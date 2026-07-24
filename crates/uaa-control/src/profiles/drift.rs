// file: crates/uaa-control/src/profiles/drift.rs
// version: 0.4.0
// guid: df5d991e-bb89-4610-ab80-458157db4e41
// last-edited: 2026-07-23

//! `content_hash` (explicit canonicalization) + `profile_versions` write capture
//! (spec `deploy-system-design.md` § Data model, Decisions 10/11, DS-REG-04),
//! plus drift **detection**, a scheduled **scan**, and the **accept** / **revert**
//! review actions (DS-REG-05).
//!
//! # Drift, and what revert actually restores (DS-REG-05)
//!
//! **Drift** = `stored.content_hash != content_hash(reconstructed_body)`. It means
//! the body was changed by something that did NOT go through `put_group` /
//! `put_profile` (which recompute the hash) — a hand-edited snapshot, say.
//!
//! [`revert_drift`] restores **the newest version whose stored body still hashes
//! to its own stored `content_hash`** ([`last_good_version`]) — the last
//! provably-untampered version, NOT a blind `N-1`. A blind `N-1` silently
//! discards the last *legitimate* change along with the drift; and had DS-REG-04
//! not captured a version on every write, version N's good body would never have
//! existed to restore. Both [`accept_drift`] and [`revert_drift`] capture the
//! drifted body as its own version row **first** (marked via the `actor` sentinel
//! [`DRIFT_SOURCE`] — `ProfileVersionRow` has no dedicated `source` column and
//! DS-REG-04's [`capture_version`] must not be modified), so neither action can
//! destroy the evidence it exists to preserve. `revert_drift` deliberately picks
//! [`last_good_version`] over the version list read **before** that evidence row
//! is appended: the evidence row hashes to its own (drifted) hash and is
//! therefore itself self-consistent, so selecting after the capture would restore
//! the drift.
//!
//! **Revert restores INTENT, not the machine.** v1 has no re-render: revert
//! appends a version row recording the restored intent and leaves the deployed
//! host exactly as drifted as it was, and does NOT rewrite the live
//! group/profile row either — so a subsequent [`scan_drift`] still surfaces the
//! same drift. That is why the scanner ([`DriftScanner`]) dedups a repeat on the
//! same `(object_id, drifted_hash)` into **one** report with a `seen_count`
//! rather than re-alerting each pass: an out-of-band editor and the revert button
//! would otherwise thrash forever. Re-deploying the host is a separate, explicit
//! operator action.
//!
//! **Threat model is inherited and bounded** (`crate::audit` Decision 21b): the
//! stored hash lives beside the body, so anyone who can edit one can edit both.
//! Drift detection's real yield is **accident and mistake detection**, not
//! defense against a root-level adversary — no comment or log here claims more.
//!
//! ## Why the audited write bridges async→sync (a deliberate deviation)
//!
//! [`AuditStore::append_in_txn`]'s `mutation` runs **synchronously** inside the
//! audit critical section, exactly like `ProfileStore::rebind`'s closure writes
//! the snapshot inside it — so the audit row can never land without the paired
//! mutation having committed first. `rebind` is a store *method* with sync access
//! to its own internals; [`accept_drift`] / [`revert_drift`] are free functions
//! holding only `&dyn ProfileStore`, whose `put_version` is `async`. The version
//! write is therefore driven to completion with `futures::executor::block_on`
//! inside the closure: `put_version` resolves without ever touching the tokio
//! reactor (it is sync work behind an async signature), so the block completes
//! immediately and cannot deadlock the `MemAuditStore` `std::Mutex` held across
//! it. `crate::db::store::guarded_mutation` is NOT used — it gates on a
//! `DbHealth` the profiles store deliberately does not have (there is no CRDB for
//! profiles to be degraded from; see `profiles::store`), exactly as
//! `allocate_index`/`rebind` already skip it.
//!
//! ## Original DS-REG-04 module notes
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

use std::collections::{BTreeMap, HashMap};

use anyhow::{anyhow, Result};
use serde::Serialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::audit::{AuditStore, NewAuditEvent};
use crate::db::{HostGroupRow, HostProfileRow, ProfileVersionRow};
use crate::profiles::store::{ensure_schema_servable, ProfileStore};

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

// ── Drift detection (DS-REG-05) ─────────────────────────────────────────────

/// The `actor` written on the version row that captures a drifted body as
/// evidence. `ProfileVersionRow` has no dedicated `source` column and
/// DS-REG-04's [`capture_version`] (which must not be modified) sets only
/// `actor`, so this sentinel is how accept/revert mark "this version is the
/// out-of-band drift we found", distinct from an operator-authored version.
pub const DRIFT_SOURCE: &str = "drift";

/// One drifted object found by [`scan_drift`] / [`DriftScanner::scan`].
/// `seen_count` is how many scans (by the same scanner) have observed this exact
/// `(object_id, actual_hash)` drift, so a persistent drift is one ongoing report
/// with a rising count rather than a fresh alert every pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriftReport {
    pub object_kind: String,
    pub object_id: Uuid,
    pub stored_hash: Vec<u8>,
    pub actual_hash: Vec<u8>,
    pub seen_count: u32,
}

/// True iff the stored hash disagrees with the body's actual canonical hash —
/// i.e. the body was changed by something that bypassed the hash-recomputing
/// write path. Errors only if `body` cannot be hashed (e.g. contains a float).
pub fn is_drifted(stored_hash: &[u8], body: &serde_json::Value) -> Result<bool> {
    Ok(stored_hash != content_hash(body)?.as_slice())
}

/// The newest version whose stored `body` still hashes to its own stored
/// `content_hash` — the last provably-untampered version. **NOT** `versions[N-1]`.
///
/// Walks versions newest→oldest (by `version` number, which [`capture_version`]
/// makes monotonic with no gaps) and returns the FIRST that is self-consistent.
/// Fails loudly — never guesses, never returns "the least bad" — when the list is
/// empty or **every** version is itself inconsistent, which is exactly the case
/// where a body could not be reconstructed at any price.
pub fn last_good_version(versions: &[ProfileVersionRow]) -> Result<&ProfileVersionRow> {
    if versions.is_empty() {
        return Err(anyhow!(
            "no versions exist for this object; cannot revert to a last-good body \
             (never invent one, never fall back to the drifted body)"
        ));
    }
    let mut by_newest: Vec<&ProfileVersionRow> = versions.iter().collect();
    by_newest.sort_unstable_by_key(|v| std::cmp::Reverse(v.version));
    for candidate in by_newest {
        // Recompute the canonical hash of the STORED body and compare to the
        // STORED hash: a version tampered the same way the live row was must not
        // be selected just because it is newer.
        if !is_drifted(&candidate.content_hash, &candidate.body)? {
            return Ok(candidate);
        }
    }
    Err(anyhow!(
        "every stored version is itself inconsistent (its body no longer hashes to \
         its stored content_hash); refusing to pick a least-bad version to revert to"
    ))
}

/// Scheduled drift scanner. Holds the per-`(object_id, drifted_hash)` seen-count
/// in memory (**no new persistence** — spec D4 has no DB) so a drift that recurs
/// across scans is reported ONCE per scan with a rising `seen_count`, never
/// re-alerted as a brand-new event each pass. Intended to be constructed once and
/// [`scan`](DriftScanner::scan)ned periodically by a scheduler (out of scope for
/// this task).
#[derive(Debug, Default)]
pub struct DriftScanner {
    seen: HashMap<(Uuid, Vec<u8>), u32>,
}

impl DriftScanner {
    pub fn new() -> Self {
        Self::default()
    }

    /// Walk **every** group and profile in `store` and report each currently
    /// drifted object. Scheduled, not read-triggered: an object nobody fetches
    /// individually is still checked, so drift in it is still surfaced. Reads go
    /// through the store's fail-closed accessors (`list_groups` / `list_profiles`
    /// use `read_snapshot_strict`), never the fail-open `read_snapshot`.
    pub async fn scan(&mut self, store: &dyn ProfileStore) -> Result<Vec<DriftReport>> {
        let mut reports = Vec::new();
        let groups = store.list_groups().await?;
        for group in &groups {
            if let Some(report) =
                self.check(OBJECT_KIND_HOST_GROUP, group.id, &group.content_hash, &group_body(group))?
            {
                reports.push(report);
            }
        }
        for group in &groups {
            for profile in store.list_profiles(group.id).await? {
                if let Some(report) = self.check(
                    OBJECT_KIND_HOST_PROFILE,
                    profile.id,
                    &profile.content_hash,
                    &profile_body(&profile),
                )? {
                    reports.push(report);
                }
            }
        }
        Ok(reports)
    }

    /// Check one object; on drift, bump and stamp its cumulative `seen_count`.
    fn check(
        &mut self,
        object_kind: &str,
        object_id: Uuid,
        stored_hash: &[u8],
        body: &serde_json::Value,
    ) -> Result<Option<DriftReport>> {
        let actual = content_hash(body)?.to_vec();
        if stored_hash == actual.as_slice() {
            return Ok(None); // not drifted — the anti-over-suppression happy path
        }
        let count = self.seen.entry((object_id, actual.clone())).or_insert(0);
        *count += 1;
        Ok(Some(DriftReport {
            object_kind: object_kind.to_string(),
            object_id,
            stored_hash: stored_hash.to_vec(),
            actual_hash: actual,
            seen_count: *count,
        }))
    }
}

/// One-shot scan over EVERY group and profile (a fresh [`DriftScanner`], so every
/// `seen_count` is 1). Use a long-lived [`DriftScanner`] instead when repeat
/// drift must be deduped across scans.
pub async fn scan_drift(store: &dyn ProfileStore) -> Result<Vec<DriftReport>> {
    DriftScanner::new().scan(store).await
}

// ── Accept / revert review actions (DS-REG-05) ──────────────────────────────

/// A located live object (group or profile) whose drift is under review.
enum ReviewTarget {
    Group(HostGroupRow),
    Profile(HostProfileRow),
}

impl ReviewTarget {
    fn object_kind(&self) -> &'static str {
        match self {
            ReviewTarget::Group(_) => OBJECT_KIND_HOST_GROUP,
            ReviewTarget::Profile(_) => OBJECT_KIND_HOST_PROFILE,
        }
    }

    fn object_id(&self) -> Uuid {
        match self {
            ReviewTarget::Group(g) => g.id,
            ReviewTarget::Profile(p) => p.id,
        }
    }

    fn stored_hash(&self) -> &[u8] {
        match self {
            ReviewTarget::Group(g) => &g.content_hash,
            ReviewTarget::Profile(p) => &p.content_hash,
        }
    }

    /// The live row's on-disk envelope `schema_version` — the [`revert_drift`]
    /// gate checks this before rolling anything back (PS-SCHEMA-20).
    fn schema_version(&self) -> i64 {
        match self {
            ReviewTarget::Group(g) => g.schema_version,
            ReviewTarget::Profile(p) => p.schema_version,
        }
    }

    /// The live body, reconstructed with the SAME field selection the hash was
    /// computed over ([`group_body`] / [`profile_body`]).
    fn body(&self) -> serde_json::Value {
        match self {
            ReviewTarget::Group(g) => group_body(g),
            ReviewTarget::Profile(p) => profile_body(p),
        }
    }
}

/// Locate a group or profile by its `object_id`. Reads via the store's
/// fail-closed accessors only.
async fn find_review_target(store: &dyn ProfileStore, object_id: Uuid) -> Result<ReviewTarget> {
    let groups = store.list_groups().await?;
    if let Some(group) = groups.iter().find(|g| g.id == object_id) {
        return Ok(ReviewTarget::Group(group.clone()));
    }
    for group in &groups {
        if let Some(profile) = store
            .list_profiles(group.id)
            .await?
            .into_iter()
            .find(|p| p.id == object_id)
        {
            return Ok(ReviewTarget::Profile(profile));
        }
    }
    Err(anyhow!(
        "object {object_id} is not a known host group or profile"
    ))
}

/// Build the next version row for `object_id` adopting `body`, with a FRESHLY
/// computed `content_hash` and the given `version` number and `actor`.
fn next_version_row(
    object_kind: &str,
    object_id: Uuid,
    body: &serde_json::Value,
    version: i64,
    actor: &str,
) -> Result<ProfileVersionRow> {
    Ok(ProfileVersionRow {
        id: Uuid::new_v4(),
        object_kind: object_kind.to_string(),
        object_id,
        version,
        body: body.clone(),
        content_hash: content_hash(body)?.to_vec(),
        actor: actor.to_string(),
        created_at: Some(chrono::Utc::now().to_rfc3339()),
    })
}

/// Write `row` as the audited review mutation: the version write and its audit
/// event commit as one unit via [`AuditStore::append_in_txn`] (NOT `record`,
/// whose no-op mutation must never accompany a state change). See the module doc
/// for why the async `put_version` is driven with `block_on` inside the sync
/// mutation closure.
async fn append_review_version(
    store: &dyn ProfileStore,
    audit: &dyn AuditStore,
    target: &ReviewTarget,
    row: ProfileVersionRow,
    actor: &str,
    action: &str,
) -> Result<()> {
    let event = NewAuditEvent {
        at: chrono::Utc::now().to_rfc3339(),
        actor: actor.to_string(),
        role: "operator".to_string(),
        action: action.to_string(),
        target: Some(format!("{}:{}", target.object_kind(), target.object_id())),
        outcome: "success".to_string(),
        detail: Some(serde_json::json!({
            "object_kind": target.object_kind(),
            "adopted_version": row.version,
        })),
    };
    audit
        .append_in_txn(
            Box::new(move || futures::executor::block_on(store.put_version(row))),
            event,
        )
        .await?;
    Ok(())
}

/// Adopt the current (drifted) body as the intended state. Captures the drifted
/// body as evidence FIRST, then appends a version adopting that same body with a
/// freshly computed hash — forward-only; nothing is destroyed. Errors (naming the
/// object) if it is not actually drifted, so an operator acting on a clean object
/// learns nothing happened rather than getting a silent no-op.
pub async fn accept_drift(
    store: &dyn ProfileStore,
    audit: &dyn AuditStore,
    object_id: Uuid,
    actor: &str,
) -> Result<ProfileVersionRow> {
    let target = find_review_target(store, object_id).await?;
    let drifted_body = target.body();
    if !is_drifted(target.stored_hash(), &drifted_body)? {
        return Err(anyhow!(
            "no drift to review: {} {object_id} is not drifted",
            target.object_kind()
        ));
    }

    // Evidence FIRST: preserve the drifted body as its own version row.
    let evidence =
        capture_version(store, target.object_kind(), object_id, &drifted_body, DRIFT_SOURCE).await?;

    // Adopt the drifted body as intended, with a fresh, correct hash.
    let adopted = next_version_row(
        target.object_kind(),
        object_id,
        &drifted_body,
        evidence.version + 1,
        actor,
    )?;
    append_review_version(store, audit, &target, adopted.clone(), actor, "registry.drift.accept")
        .await?;
    Ok(adopted)
}

/// Restore [`last_good_version`]. Captures the drifted body as evidence FIRST,
/// then appends a version carrying the last-good body.
///
/// Restores INTENT, not the machine — the deployed host stays exactly as drifted
/// as it was, and re-deploying it is a separate operator action. Errors (naming
/// the object) if it is not drifted, if it has no versions, or if every stored
/// version is itself inconsistent — never a blind `N-1`, never a least-bad guess.
pub async fn revert_drift(
    store: &dyn ProfileStore,
    audit: &dyn AuditStore,
    object_id: Uuid,
    actor: &str,
) -> Result<ProfileVersionRow> {
    let target = find_review_target(store, object_id).await?;
    // Envelope version gate (PS-SCHEMA-20): refuse to roll back a row written by
    // a newer binary rather than restore a body against a shape this binary
    // cannot fully recognize. Fail-loud, keyed off schema_version alone — no
    // blob deserialization. Symmetric with the serve gate in `convert`.
    ensure_schema_servable(target.schema_version())
        .map_err(|e| anyhow!("cannot revert {} {object_id}: {e}", target.object_kind()))?;
    let drifted_body = target.body();
    if !is_drifted(target.stored_hash(), &drifted_body)? {
        return Err(anyhow!(
            "no drift to review: {} {object_id} is not drifted",
            target.object_kind()
        ));
    }

    // Select the restore target BEFORE capturing the drift evidence: the evidence
    // row hashes to its own (drifted) hash and is therefore self-consistent, so
    // selecting last_good afterwards would restore the drift itself.
    let versions = store.list_versions(object_id).await?;
    let good_body = last_good_version(&versions)
        .map_err(|source| {
            anyhow!(
                "cannot revert {} {object_id}: {source}",
                target.object_kind()
            )
        })?
        .body
        .clone();

    // Evidence FIRST: preserve the drifted body as its own version row.
    let evidence =
        capture_version(store, target.object_kind(), object_id, &drifted_body, DRIFT_SOURCE).await?;

    // Restore the last-good body, with a fresh, correct hash.
    let restored = next_version_row(
        target.object_kind(),
        object_id,
        &good_body,
        evidence.version + 1,
        actor,
    )?;
    append_review_version(store, audit, &target, restored.clone(), actor, "registry.drift.revert")
        .await?;
    Ok(restored)
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
            schema_version: 0,
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

    // ── DS-REG-05: drift scan + accept/revert ──────────────────────────────

    use crate::audit::MemAuditStore;
    use crate::db::HostProfileRow;

    /// A group row with the given `applications` body and stored `content_hash`,
    /// every other field fixed — so two `group_row`s differ in body iff their
    /// `applications` differ.
    fn group_row(id: Uuid, apps: serde_json::Value, stored_hash: Vec<u8>) -> HostGroupRow {
        HostGroupRow {
            id,
            name: "len-serv".into(),
            hostname_pattern: "{name}-{index:03}".into(),
            is_standalone: false,
            defaults: serde_json::json!({}),
            applications: apps,
            content_hash: stored_hash,
            version: 1,
            schema_version: 0,
            created_at: None,
            updated_at: None,
        }
    }

    fn profile_row(id: Uuid, group_id: Uuid, apps: serde_json::Value, stored_hash: Vec<u8>) -> HostProfileRow {
        HostProfileRow {
            id,
            group_id,
            identity: "aa:bb:cc:dd:ee:ff".into(),
            hostname_override: None,
            overrides: serde_json::json!({}),
            applications: apps,
            content_hash: stored_hash,
            version: 1,
            schema_version: 0,
            created_at: None,
            updated_at: None,
        }
    }

    /// The canonical body a `group_row` with these `applications` hashes over.
    fn group_body_for(apps: serde_json::Value) -> serde_json::Value {
        group_body(&group_row(Uuid::nil(), apps, vec![]))
    }

    fn hash_of(body: &serde_json::Value) -> Vec<u8> {
        content_hash(body).unwrap().to_vec()
    }

    /// Inject a drifted live group: `applications = apps`, but stored hash is the
    /// hash of a DIFFERENT ("original") body — the out-of-band-edit shape.
    fn inject_drifted_group(store: &MemProfileStore, id: Uuid, apps: serde_json::Value) {
        let stale = hash_of(&group_body_for(serde_json::json!(["original"])));
        store.inject_group_raw(group_row(id, apps, stale));
    }

    #[tokio::test]
    async fn test_drift_detected_on_out_of_band_edit() {
        let store = MemProfileStore::new();
        let id = Uuid::new_v4();
        let drifted = group_row(id, serde_json::json!(["tampered"]), hash_of(&group_body_for(serde_json::json!(["original"]))));
        let body = group_body(&drifted);

        assert!(
            is_drifted(&drifted.content_hash, &body).unwrap(),
            "a body whose stored hash was computed over a different body is drifted"
        );

        store.inject_group_raw(drifted);
        let reports = scan_drift(&store).await.unwrap();
        assert!(
            reports.iter().any(|r| r.object_id == id),
            "scan_drift must report the out-of-band-edited object"
        );
    }

    #[tokio::test]
    async fn test_scan_finds_drift_in_unread_object() {
        // A drifted PROFILE that is never fetched individually (no get/list on it)
        // is still surfaced, because the scan walks every group's profiles.
        let store = MemProfileStore::new();
        let group_id = Uuid::new_v4();
        let profile_id = Uuid::new_v4();
        // A clean group so the scan iterates into its profiles.
        store.put_group(group_row(group_id, serde_json::json!([]), vec![]), "op").await.unwrap();

        let clean_profile_body = profile_body(&profile_row(profile_id, group_id, serde_json::json!(["orig"]), vec![]));
        let stale = hash_of(&clean_profile_body);
        store.inject_profile_raw(profile_row(profile_id, group_id, serde_json::json!(["tampered"]), stale));

        let reports = scan_drift(&store).await.unwrap();
        assert!(
            reports.iter().any(|r| r.object_id == profile_id && r.object_kind == OBJECT_KIND_HOST_PROFILE),
            "a drifted profile nobody read individually must still be reported"
        );
    }

    #[tokio::test]
    async fn test_revert_restores_last_good_not_n_minus_1() {
        // v1 good, v2 good (a legitimate change), then v2's body is tampered
        // out-of-band. Revert must restore v2's body — a blind N-1 gives v1.
        let store = MemProfileStore::new();
        let audit = MemAuditStore::new();
        let id = Uuid::new_v4();

        let v1_body = group_body_for(serde_json::json!(["v1"]));
        let v2_body = group_body_for(serde_json::json!(["v2"]));
        capture_version(&store, OBJECT_KIND_HOST_GROUP, id, &v1_body, "op").await.unwrap();
        capture_version(&store, OBJECT_KIND_HOST_GROUP, id, &v2_body, "op").await.unwrap();

        // Live row: v2's good hash, but a tampered body.
        store.inject_group_raw(group_row(id, serde_json::json!(["TAMPERED"]), hash_of(&v2_body)));

        let restored = revert_drift(&store, &audit, id, "operator").await.unwrap();
        assert_eq!(restored.body, v2_body, "revert must restore the LAST-good body (v2)");
        assert_ne!(restored.body, v1_body, "a blind N-1 would have restored v1 — that is the bug");
        assert!(
            !is_drifted(&restored.content_hash, &restored.body).unwrap(),
            "the restored version must carry a fresh, correct hash"
        );
    }

    #[tokio::test]
    async fn test_revert_captures_drifted_body_first() {
        let store = MemProfileStore::new();
        let audit = MemAuditStore::new();
        let id = Uuid::new_v4();

        let good_body = group_body_for(serde_json::json!(["good"]));
        capture_version(&store, OBJECT_KIND_HOST_GROUP, id, &good_body, "op").await.unwrap();
        store.inject_group_raw(group_row(id, serde_json::json!(["TAMPERED"]), hash_of(&good_body)));
        let tampered_body = group_body_for(serde_json::json!(["TAMPERED"]));

        revert_drift(&store, &audit, id, "operator").await.unwrap();

        let versions = store.list_versions(id).await.unwrap();
        let evidence = versions
            .iter()
            .find(|v| v.actor == DRIFT_SOURCE)
            .expect("a drift-source evidence version must exist after revert");
        assert_eq!(evidence.body, tampered_body, "the evidence row must hold the tampered body");
    }

    #[tokio::test]
    async fn test_accept_captures_drifted_body_and_adopts() {
        let store = MemProfileStore::new();
        let audit = MemAuditStore::new();
        let id = Uuid::new_v4();
        let drifted_body = group_body_for(serde_json::json!(["drifted"]));
        inject_drifted_group(&store, id, serde_json::json!(["drifted"]));

        let adopted = accept_drift(&store, &audit, id, "operator").await.unwrap();

        assert_eq!(adopted.body, drifted_body, "accept adopts the drifted body as intended");
        assert_eq!(
            adopted.content_hash,
            hash_of(&drifted_body),
            "the adopted version must carry a fresh, correct hash"
        );
        assert!(!is_drifted(&adopted.content_hash, &adopted.body).unwrap());

        let versions = store.list_versions(id).await.unwrap();
        assert!(
            versions.iter().any(|v| v.actor == DRIFT_SOURCE && v.body == drifted_body),
            "the drifted body must survive as a drift-source evidence version"
        );
    }

    #[tokio::test]
    async fn test_revert_errors_when_no_good_version() {
        let store = MemProfileStore::new();
        let audit = MemAuditStore::new();
        let id = Uuid::new_v4();
        inject_drifted_group(&store, id, serde_json::json!(["drifted"]));

        // Every stored version is itself inconsistent (bogus stored hash).
        store
            .put_version(ProfileVersionRow {
                id: Uuid::new_v4(),
                object_kind: OBJECT_KIND_HOST_GROUP.into(),
                object_id: id,
                version: 1,
                body: serde_json::json!({"x": 1}),
                content_hash: vec![0xde, 0xad, 0xbe, 0xef],
                actor: "op".into(),
                created_at: None,
            })
            .await
            .unwrap();

        assert!(
            revert_drift(&store, &audit, id, "operator").await.is_err(),
            "revert must Err when no version is self-consistent — never pick a least-bad one"
        );
    }

    #[tokio::test]
    async fn test_revert_refuses_future_schema_version() {
        // PS-SCHEMA-20 roll-back gate: a live row whose schema_version exceeds
        // this binary's max must be refused fail-loud, BEFORE any body work —
        // symmetric with the serve-path gate in `convert`.
        let store = MemProfileStore::new();
        let audit = MemAuditStore::new();
        let id = Uuid::new_v4();
        let mut row = group_row(id, serde_json::json!(["future"]), vec![0x01]);
        row.schema_version = 2;
        store.inject_group_raw(row);

        let err = revert_drift(&store, &audit, id, "op").await.unwrap_err();
        assert!(
            err.to_string().contains("schema version 2 exceeds binary max 1"),
            "expected the version-gate message, got: {err}"
        );
    }

    #[tokio::test]
    async fn test_review_on_clean_object_errors() {
        let store = MemProfileStore::new();
        let audit = MemAuditStore::new();
        let id = Uuid::new_v4();
        // put_group recomputes the hash, so this live row is NOT drifted.
        store.put_group(group_row(id, serde_json::json!(["clean"]), vec![]), "op").await.unwrap();

        assert!(
            accept_drift(&store, &audit, id, "operator").await.is_err(),
            "accept on a clean object must Err, not silently no-op"
        );
        assert!(
            revert_drift(&store, &audit, id, "operator").await.is_err(),
            "revert on a clean object must Err, not silently no-op"
        );
    }

    #[tokio::test]
    async fn test_repeat_drift_reported_once_with_count() {
        let store = MemProfileStore::new();
        let id = Uuid::new_v4();
        inject_drifted_group(&store, id, serde_json::json!(["drifted"]));

        let mut scanner = DriftScanner::new();
        let _ = scanner.scan(&store).await.unwrap();
        let _ = scanner.scan(&store).await.unwrap();
        let third = scanner.scan(&store).await.unwrap();

        assert_eq!(third.len(), 1, "the same drift is one report per scan, not a growing list");
        assert_eq!(third[0].seen_count, 3, "a repeat is reported once with a rising count");
    }

    #[tokio::test]
    async fn test_review_actions_are_audited() {
        // Accept on one object, revert on another; both audited with the caller.
        let audit = MemAuditStore::new();

        let accept_store = MemProfileStore::new();
        let accept_id = Uuid::new_v4();
        inject_drifted_group(&accept_store, accept_id, serde_json::json!(["drifted"]));
        accept_drift(&accept_store, &audit, accept_id, "op-accept").await.unwrap();

        let revert_store = MemProfileStore::new();
        let revert_id = Uuid::new_v4();
        let good_body = group_body_for(serde_json::json!(["good"]));
        capture_version(&revert_store, OBJECT_KIND_HOST_GROUP, revert_id, &good_body, "op").await.unwrap();
        revert_store.inject_group_raw(group_row(revert_id, serde_json::json!(["TAMPERED"]), hash_of(&good_body)));
        revert_drift(&revert_store, &audit, revert_id, "op-revert").await.unwrap();

        let events = audit.list_events(0).await.unwrap();
        assert!(
            events.iter().any(|e| e.actor == "op-accept" && e.action == "registry.drift.accept"),
            "accept must append an audit event with the caller's actor"
        );
        assert!(
            events.iter().any(|e| e.actor == "op-revert" && e.action == "registry.drift.revert"),
            "revert must append an audit event with the caller's actor"
        );
    }

    #[tokio::test]
    async fn test_clean_object_is_not_reported() {
        // A fleet-shaped set of un-drifted objects (written through put_*, which
        // recompute the hash) must scan to EMPTY — the anti-over-suppression guard.
        let store = MemProfileStore::new();
        for i in 0..3 {
            let gid = Uuid::new_v4();
            let mut row = group_row(gid, serde_json::json!([{"app": i}]), vec![]);
            row.name = format!("group-{i}");
            store.put_group(row, "op").await.unwrap();
            store
                .put_profile(profile_row(Uuid::new_v4(), gid, serde_json::json!([{"p": i}]), vec![]), "op")
                .await
                .unwrap();
        }

        let reports = scan_drift(&store).await.unwrap();
        assert!(reports.is_empty(), "a scan over untouched objects must return empty, not flag everything");
    }
}
