// file: crates/uaa-control/src/profiles/drift.rs
// version: 0.2.0
// guid: df5d991e-bb89-4610-ab80-458157db4e41
// last-edited: 2026-07-17

//! `content_hash` (explicit canonicalization) + `profile_versions` write capture
//! (spec `deploy-system-design.md` § Data model, Decisions 10/11, DS-REG-04).
//!
//! This module ships hashing + version capture **only**. Drift *detection* and
//! accept/revert are DS-REG-05 (which fills the rest of this file).
//!
//! # Why `content_hash` cannot lean on `serde_json`'s internal ordering
//!
//! `serde_json`'s `preserve_order` feature is **off** in this workspace today
//! (verify: `grep -n "preserve_order" Cargo.toml crates/uaa-control/Cargo.toml`
//! — zero hits), which makes `serde_json::Value::Object` an alias for
//! `BTreeMap<String, Value>` and therefore already key-sorted on parse. That
//! makes a naive `SHA-256(serde_json::to_vec(body))` *look* deterministic
//! today — and makes the obvious "shuffle the keys, hash should match" test
//! **vacuous**: it would pass whether or not this module canonicalizes
//! anything, because `serde_json` already did the sorting for free. Two
//! unpinned assumptions break that naive approach later: (a) any dependency in
//! the workspace enabling `preserve_order` (a global feature-unification
//! hazard — the `crate::audit` module's own hand-built-`BTreeMap` helper
//! explicitly defends only its own **top level** against this, rather than
//! trusting `serde_json::Value`), and (b) a float entering a body (`1.0` and
//! `1` are the same JSON number and serialize to different bytes).
//!
//! So [`canonical_body_bytes`] recursively re-sorts object keys into
//! `std::collections::BTreeMap`s **itself**, independent of `serde_json`'s
//! internal representation, and [`content_hash`] rejects any float outright
//! rather than hash it silently. This is a **different function** from the
//! hashing helpers in `crate::audit`: those hash an *audit event* (was the
//! log tampered with); this hashes an arbitrary *object body* (did this
//! group/profile change out-of-band) — spec D10. Conflating the two would
//! misstate what the audit chain proves.
//!
//! # Why every write captures a version, not only a changed one
//!
//! [`capture_version`] is called on **every** `put_group` / `put_profile`,
//! regardless of whether the body actually changed. DS-REG-05's revert
//! restores "the newest version whose body still hashes to its own stored
//! `content_hash`" — which only works if version N was captured **before** an
//! out-of-band edit could ever overwrite the live row. A version row written
//! only on *detected* drift is too late: by the time drift is detected, the
//! out-of-band write has already destroyed the only copy of the good body
//! (spec D11).

use std::collections::BTreeMap;

use anyhow::{anyhow, Result};
use serde::Serialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::db::{HostGroupRow, HostProfileRow, ProfileVersionRow};
use crate::profiles::store::ProfileStore;

// ── Canonicalization ────────────────────────────────────────────────────────

/// An arbitrary JSON body, re-shaped so that serialization order is
/// guaranteed by `BTreeMap`'s own `Serialize` impl (which always iterates in
/// key order) rather than by `serde_json::Value`'s internal representation.
/// Floats are excluded from this type entirely — [`canonicalize`] rejects
/// them before a [`Canonical`] value naming a float can ever be constructed.
#[derive(Serialize)]
#[serde(untagged)]
enum Canonical {
    Null,
    Bool(bool),
    /// Integers only — [`canonicalize`] refuses any `serde_json::Number` for
    /// which `is_f64()` is true before this variant is ever constructed.
    Number(serde_json::Number),
    String(String),
    /// Array order is significant and preserved verbatim; only object *keys*
    /// are sorted (spec: "Array order is significant").
    Array(Vec<Canonical>),
    Object(BTreeMap<String, Canonical>),
}

/// Recursively re-sorts `value` into [`Canonical`], erroring with the exact
/// `path` (e.g. `$.applications[2].weight`) of the first float encountered.
/// `path` starts at `"$"` for the root and is extended with `.key` / `[idx]`
/// exactly like a jq/JSONPath expression, so the error message is directly
/// actionable against the offending body.
fn canonicalize(value: &serde_json::Value, path: &str) -> Result<Canonical> {
    Ok(match value {
        serde_json::Value::Null => Canonical::Null,
        serde_json::Value::Bool(b) => Canonical::Bool(*b),
        serde_json::Value::Number(n) => {
            if n.is_f64() {
                return Err(anyhow!(
                    "content_hash: refusing to hash float at {path}: JSON numbers `1.0` and `1` \
                     round-trip to different bytes, so a float body would make content_hash \
                     change for no reason; use an integer or store the value as a string"
                ));
            }
            Canonical::Number(n.clone())
        }
        serde_json::Value::String(s) => Canonical::String(s.clone()),
        serde_json::Value::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for (i, item) in items.iter().enumerate() {
                out.push(canonicalize(item, &format!("{path}[{i}]"))?);
            }
            Canonical::Array(out)
        }
        serde_json::Value::Object(map) => {
            let mut out = BTreeMap::new();
            for (k, v) in map {
                out.insert(k.clone(), canonicalize(v, &format!("{path}.{k}"))?);
            }
            Canonical::Object(out)
        }
    })
}

/// Canonicalize `body`: recursively sort object keys into a `BTreeMap`
/// itself, preserve array order, and reject any float value outright. This
/// deliberately does NOT rely on `serde_json`'s internal ordering — see the
/// module doc for why a naive `serde_json::to_vec(body)` would look correct
/// today and break later. An empty object `{}` is legal and hashes to a
/// stable value; it is NOT an error.
pub fn canonical_body_bytes(body: &serde_json::Value) -> Result<Vec<u8>> {
    let canonical = canonicalize(body, "$")?;
    Ok(serde_json::to_vec(&canonical).expect("Canonical always serializes"))
}

/// SHA-256 over [`canonical_body_bytes`]. A **separate** function from the
/// `crate::audit` module's own event-hashing helper — this hashes an object
/// *body*, not an audit *event* — see the module doc (spec D10). Do NOT call
/// `crate::audit`'s hashing helpers from here.
pub fn content_hash(body: &serde_json::Value) -> Result<[u8; 32]> {
    let bytes = canonical_body_bytes(body)?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    Ok(hasher.finalize().into())
}

// ── Row content bodies ──────────────────────────────────────────────────────

/// `object_kind` for a [`HostGroupRow`] (spec `deploy-system-design.md` §
/// Data model: `ProfileVersionRow.object_kind` is `"host_group"` |
/// `"host_profile"`).
pub const OBJECT_KIND_HOST_GROUP: &str = "host_group";
/// `object_kind` for a [`HostProfileRow`].
pub const OBJECT_KIND_HOST_PROFILE: &str = "host_profile";

/// The content fields of a host group that participate in `content_hash` /
/// drift detection. Excludes `id` (identity, not content), `content_hash`
/// itself (hashing it would be circular), `version` (a write counter, not
/// content), and the timestamps (mutated by every write, never human-authored
/// content). **Exported** so DS-REG-05's `scan_drift` reconstructs the
/// IDENTICAL body when checking a live row's `content_hash` for drift — a
/// silent divergence here would make every group read as drifted.
pub fn group_body(row: &HostGroupRow) -> serde_json::Value {
    serde_json::json!({
        "name": row.name,
        "hostname_pattern": row.hostname_pattern,
        "is_standalone": row.is_standalone,
        "defaults": row.defaults,
        "applications": row.applications,
    })
}

/// The content fields of a host profile that participate in `content_hash` /
/// drift detection. Same exclusions as [`group_body`]; see its doc.
pub fn profile_body(row: &HostProfileRow) -> serde_json::Value {
    serde_json::json!({
        "group_id": row.group_id,
        "identity": row.identity,
        "hostname_override": row.hostname_override,
        "overrides": row.overrides,
        "applications": row.applications,
    })
}

// ── Version capture ─────────────────────────────────────────────────────────

/// Append `body` as the next version for `object_id`. Called on **EVERY**
/// write (`put_group` / `put_profile`), not only on change — see the module
/// doc for why capture-only-on-drift is too late. Version numbers are
/// monotonic per `object_id`, starting at 1, with no gaps: computed as
/// `max(existing versions) + 1`. A duplicate `(object_id, version)` — which
/// single-writer `uaa-control` (spec D4) should never produce — is refused
/// rather than silently overwritten.
pub async fn capture_version(
    store: &dyn ProfileStore,
    object_kind: &str,
    object_id: Uuid,
    body: &serde_json::Value,
    actor: &str,
) -> Result<ProfileVersionRow> {
    let hash = content_hash(body)?;
    let existing = store.list_versions(object_id).await?;
    let next_version = existing.iter().map(|v| v.version).max().unwrap_or(0) + 1;
    if existing.iter().any(|v| v.version == next_version) {
        return Err(anyhow!(
            "capture_version: version {next_version} already exists for object {object_id}"
        ));
    }

    let row = ProfileVersionRow {
        id: Uuid::new_v4(),
        object_kind: object_kind.to_string(),
        object_id,
        version: next_version,
        body: body.clone(),
        content_hash: hash.to_vec(),
        actor: actor.to_string(),
        created_at: Some(chrono::Utc::now().to_rfc3339()),
    };
    store.put_version(row.clone()).await?;
    Ok(row)
}

// ── Unit tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profiles::store::MemProfileStore;

    #[test]
    fn test_content_hash_is_canonical() {
        // Two JSON texts with the SAME keys, inserted in different order, at
        // BOTH the top level and a nested object, must hash identically.
        //
        // This test would FAIL if canonical_body_bytes were replaced by
        // serde_json::to_vec: do not "simplify" it back to that. It passes
        // today only because canonicalize() explicitly re-sorts every object
        // level into its own BTreeMap, independent of serde_json's
        // preserve_order feature (which happens to be off today but is a
        // global feature-unification hazard — see the module doc).
        let a: serde_json::Value = serde_json::from_str(
            r#"{"name":"len-serv","nested":{"z":1,"a":2},"active":true}"#,
        )
        .unwrap();
        let b: serde_json::Value = serde_json::from_str(
            r#"{"active":true,"nested":{"a":2,"z":1},"name":"len-serv"}"#,
        )
        .unwrap();

        let hash_a = content_hash(&a).unwrap();
        let hash_b = content_hash(&b).unwrap();
        assert_eq!(hash_a, hash_b, "key order must not change the hash");
    }

    #[test]
    fn test_content_hash_rejects_float() {
        let body = serde_json::json!({"weight": 1.5});
        let err = content_hash(&body).unwrap_err();
        assert!(
            err.to_string().contains("$.weight"),
            "error must name the path to the offending value, got: {err}"
        );
    }

    #[test]
    fn test_content_hash_empty_body_is_stable() {
        let body = serde_json::json!({});
        let first = content_hash(&body).unwrap();
        let second = content_hash(&body).unwrap();
        assert_eq!(first, second, "an empty body is legal and must hash stably");
    }

    #[test]
    fn test_content_hash_array_order_is_significant() {
        let a = serde_json::json!({"items": [1, 2]});
        let b = serde_json::json!({"items": [2, 1]});
        assert_ne!(
            content_hash(&a).unwrap(),
            content_hash(&b).unwrap(),
            "only object keys sort; array order must remain significant"
        );
    }

    #[tokio::test]
    async fn test_capture_version_on_every_write() {
        // Exercises the WIRING (put_group -> capture_version), not
        // capture_version directly — a regression that dropped the
        // capture_version call from put_group would leave a
        // capture_version-only test green while breaking the actual write
        // path this task exists to fix.
        let store = MemProfileStore::new();
        let group_id = Uuid::new_v4();
        let row = HostGroupRow {
            id: group_id,
            name: "len-serv".to_string(),
            hostname_pattern: "{name}-{index:03}".to_string(),
            is_standalone: false,
            defaults: serde_json::json!({"note": "same-body-both-times"}),
            applications: serde_json::json!([]),
            content_hash: vec![],
            version: 1,
            created_at: None,
            updated_at: None,
        };

        store.put_group(row.clone(), "alice").await.unwrap();
        store.put_group(row, "alice").await.unwrap();

        let versions = store.list_versions(group_id).await.unwrap();
        assert_eq!(
            versions.len(),
            2,
            "capture is unconditional: two put_group calls with the SAME body must still \
             produce two versions"
        );
    }

    #[tokio::test]
    async fn test_capture_version_is_monotonic() {
        let store = MemProfileStore::new();
        let object_id = Uuid::new_v4();

        for i in 0..3 {
            capture_version(
                &store,
                OBJECT_KIND_HOST_GROUP,
                object_id,
                &serde_json::json!({"n": i}),
                "alice",
            )
            .await
            .unwrap();
        }

        let mut versions: Vec<i64> = store
            .list_versions(object_id)
            .await
            .unwrap()
            .into_iter()
            .map(|v| v.version)
            .collect();
        versions.sort_unstable();
        assert_eq!(versions, vec![1, 2, 3], "versions must be 1, 2, 3 with no gaps");
    }

    #[tokio::test]
    async fn test_version_body_hash_matches_stored() {
        let store = MemProfileStore::new();
        let object_id = Uuid::new_v4();
        let bodies = [
            serde_json::json!({"n": 1}),
            serde_json::json!({"n": 2, "nested": {"b": 1, "a": 2}}),
        ];
        for body in &bodies {
            capture_version(&store, OBJECT_KIND_HOST_GROUP, object_id, body, "alice")
                .await
                .unwrap();
        }

        for row in store.list_versions(object_id).await.unwrap() {
            let recomputed = content_hash(&row.body).unwrap();
            assert_eq!(
                row.content_hash,
                recomputed.to_vec(),
                "every captured row's content_hash must equal content_hash(row.body) — \
                 the self-consistency property DS-REG-05's revert selects on"
            );
        }
    }
}
