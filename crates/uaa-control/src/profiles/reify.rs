// file: crates/uaa-control/src/profiles/reify.rs
// version: 0.1.1
// guid: 5e2a7c94-9d31-4b08-a6f2-3c7e1b95d840
// last-edited: 2026-07-23

//! Registry reification (DS-OPS-05) — the INVERSE of `resolve_from_registry`.
//!
//! [`register_from_config`] takes a fully-resolved [`InstallationConfig`] plus a
//! host's identity (MAC, group naming, optional pinned hostname) and writes the
//! registry rows a later [`resolve_from_registry`][crate::resolve_from_registry]
//! reconstructs it from — such that reify-then-resolve round-trips EXACTLY. It is
//! the shared core behind both the one-shot `uaa config backfill` and the
//! non-destructive `uaa config place --register` shadow write.
//!
//! **Why this lives in `uaa-control`, not `uaa-core`:** like resolution, it needs
//! [`ProfileStore`] and the [`HostGroupRow`]/[`HostProfileRow`] types, which live
//! here. `uaa-core` (the [`InstallationConfig`] owner) must not depend on this
//! crate (cycle), so the config value is handed IN.
//!
//! **The row-shaping is lifted verbatim from the M2 gate's proven fixture.** The
//! `test_resolved_equals_committed_by_struct_equality` test in the `uaa` crate
//! empirically round-trips all four committed fleet configs; its
//! `profile_row_from_cfg` helper (serialize the config, drop `hostname` — the
//! allocation supplies it — and split `applications` into the profile's own typed
//! list) IS this module's [`profile_row_from_cfg`]. That test now CALLS this core
//! (not a private copy), so the extraction is proven to preserve the round-trip.
//! Do NOT "clean up" the transform: `applications` uses
//! `skip_serializing_if = Vec::is_empty`, so an app-free config has NO key to
//! remove — hence the `unwrap_or_else(json!([]))`.
//!
//! **Idempotent throughout — re-running never duplicates or renumbers.** The
//! group is looked up by its stable name and reused (never a second group of the
//! same name); the profile row REUSES the existing row's `id` for a returning
//! identity (a fresh `Uuid` each run would create a duplicate profile carrying
//! the same MAC); and `allocate_index` is already allocate-once (a bound identity
//! returns its existing row, writing nothing). So backfill and shadow-mode
//! converge on ONE group / ONE profile / ONE allocation per host, whatever the
//! re-run count.

use anyhow::{anyhow, Result};
use serde_json::json;
use uuid::Uuid;

use uaa_core::network::InstallationConfig;

use crate::db::store::StoreError;
use crate::db::{HostGroupRow, HostProfileRow};
use crate::import_export::normalize_mac;
use crate::profiles::store::ProfileStore;

/// The registry read methods are fail-CLOSED: on a genuinely MISSING snapshot
/// (the first-ever write on a fresh install, before anything has bootstrapped the
/// file) `get_group` returns an `Err`, not `Ok(None)`. For reify's group lookup
/// that missing case is not an error — there simply is no group yet, so we create
/// one, and `put_group` bootstraps the snapshot. This tolerates ONLY a NotFound
/// missing file; a corrupt snapshot (`SnapshotCorrupt`) or a permission error
/// still propagates (and `put_group`'s own strict read would refuse a corrupt
/// file anyway), so the fail-closed guarantee is preserved.
fn tolerate_fresh_snapshot(
    res: Result<Option<HostGroupRow>>,
) -> Result<Option<HostGroupRow>> {
    match res {
        Ok(v) => Ok(v),
        Err(e) => match e.downcast_ref::<StoreError>() {
            Some(StoreError::SnapshotUnreadable { source, .. })
                if source.kind() == std::io::ErrorKind::NotFound =>
            {
                Ok(None)
            }
            _ => Err(e),
        },
    }
}

/// A freshly-minted group row for `name`. `defaults`/`applications` are empty:
/// reification carries the ENTIRE config as the host profile's overrides (exactly
/// as the M2 fixture does), so the group is a pure naming/allocation scope. The
/// `content_hash` is (re)computed by `put_group`; the value here is ignored.
fn group_row(id: Uuid, name: &str, hostname_pattern: &str, is_standalone: bool) -> HostGroupRow {
    HostGroupRow {
        id,
        name: name.to_string(),
        hostname_pattern: hostname_pattern.to_string(),
        is_standalone,
        defaults: json!({}),
        applications: json!([]),
        content_hash: vec![],
        version: 1,
        // Value ignored: `put_group` stamps the current SCHEMA_VERSION_MAX
        // (like it recomputes content_hash). See profiles::store.
        schema_version: 0,
        created_at: None,
        updated_at: None,
    }
}

/// A `HostProfileRow` whose `overrides` reproduce `cfg` EXACTLY, lifted verbatim
/// from the M2 gate's proven fixture: round-trip the config to a JSON object,
/// drop `hostname` (the allocation — or `hostname_override` for a standalone —
/// supplies it at resolve time), and split `applications` out into the profile's
/// own typed list (empty when the config omitted the key). `id` is supplied by
/// the caller so a re-run REUSES the returning identity's existing row rather
/// than minting a duplicate.
fn profile_row_from_cfg(
    id: Uuid,
    group_id: Uuid,
    identity: &str,
    hostname_override: Option<&str>,
    cfg: &InstallationConfig,
) -> Result<HostProfileRow> {
    let mut v = serde_json::to_value(cfg)?;
    let obj = v
        .as_object_mut()
        .ok_or_else(|| anyhow!("InstallationConfig did not serialize to a JSON object"))?;
    obj.remove("hostname");
    let applications = obj.remove("applications").unwrap_or_else(|| json!([]));
    Ok(HostProfileRow {
        id,
        group_id,
        identity: identity.to_string(),
        hostname_override: hostname_override.map(str::to_string),
        overrides: v,
        applications,
        content_hash: vec![],
        version: 1,
        // Value ignored: `put_profile` stamps SCHEMA_VERSION_MAX on write.
        schema_version: 0,
        created_at: None,
        updated_at: None,
    })
}

/// Reify one host's resolved [`InstallationConfig`] into the profile registry, so
/// that a later `resolve_from_registry(store, cfg.hostname)` reconstructs `cfg`
/// exactly. Idempotent: safe to re-run (backfill re-run / shadow-write on every
/// place) without creating duplicate groups, profiles, or allocations.
///
/// - `group_name` / `hostname_pattern` / `is_standalone` describe the host's
///   group. The group is looked up by `group_name` and REUSED if present
///   (never a second group of the same name); only created if absent.
/// - `mac` is the host's identity; it is `normalize_mac`'d ONCE and that single
///   value is used both as the stored profile identity and for the returning-row
///   lookup, so it always matches the (also-normalized) allocation identity
///   `resolve_from_registry` keys on.
/// - Indexed hosts (`is_standalone == false`) allocate a hostname index. Because
///   the rendered name depends on ALLOCATION ORDER (index 1→001, 2→002, …),
///   callers MUST reify a group's indexed hosts in hostname-sorted order; this
///   function GUARDS that by asserting the allocated hostname equals
///   `cfg.hostname` and failing loudly otherwise, rather than silently binding a
///   machine to the wrong name.
/// - Standalone hosts pin `hostname_override` and take NO allocation (resolution
///   falls back to the override).
// Eight parameters by deliberate design: this is the flat, explicit reify API
// (group descriptor + host identity + config + actor) shared verbatim by the
// backfill, shadow-registration, and M2 round-trip call sites. Bundling them into
// a struct would only move the same fields behind one more indirection at every
// call site without adding an invariant, so the flat signature is kept.
#[allow(clippy::too_many_arguments)]
pub async fn register_from_config(
    store: &dyn ProfileStore,
    group_name: &str,
    hostname_pattern: &str,
    is_standalone: bool,
    mac: &str,
    hostname_override: Option<&str>,
    cfg: &InstallationConfig,
    actor: &str,
) -> Result<()> {
    // 1. Group: look up by stable name and reuse; create only if absent. Never
    //    mint a second group of the same name (backfill + shadow converge here).
    let group_id = match tolerate_fresh_snapshot(store.get_group(group_name).await)? {
        Some(existing) => existing.id,
        None => {
            let id = Uuid::new_v4();
            store
                .put_group(group_row(id, group_name, hostname_pattern, is_standalone), actor)
                .await?;
            id
        }
    };

    // 2. Profile: normalize the identity ONCE, reuse the existing row's id for a
    //    returning identity (a fresh Uuid each run would duplicate the profile),
    //    and store the normalized identity so it matches the allocation identity.
    let identity = normalize_mac(mac);
    let profile_id = store
        .list_profiles(group_id)
        .await?
        .into_iter()
        .find(|p| p.identity == identity)
        .map(|p| p.id)
        .unwrap_or_else(Uuid::new_v4);
    store
        .put_profile(
            profile_row_from_cfg(profile_id, group_id, &identity, hostname_override, cfg)?,
            actor,
        )
        .await?;

    // 3. Allocation (indexed hosts only). allocate_index is allocate-once, so a
    //    re-run returns the same row. Guard the allocation-order trap: the
    //    rendered hostname MUST match the config's own hostname, else a host was
    //    reified out of order and would silently bind to the wrong name.
    if !is_standalone {
        let alloc = store.allocate_index(group_id, &identity).await?;
        if alloc.hostname != cfg.hostname {
            return Err(anyhow!(
                "reify {identity}: allocated hostname {:?} != expected {:?} — indexed hosts \
                 must be reified in hostname-sorted order (index 1→001, 2→002, …)",
                alloc.hostname,
                cfg.hostname
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::store::StatePaths;
    use crate::profiles::store::SnapshotProfileStore;
    use crate::resolve_from_registry;

    // The full four-host round-trip against the COMMITTED fleet YAML is the M2
    // gate in the `uaa` crate (it needs serde_yaml + the committed files, neither
    // a uaa-control dependency); that test now calls this real reify core. These
    // tests cover what M2 does not: the reify core in isolation, and — the point
    // of the whole entrypoint — its idempotency under re-runs.

    /// Minimal valid `InstallationConfig` for `hostname`. Built via serde_json so
    /// this test needs neither serde_yaml nor a committed file: only the
    /// no-default fields are supplied; `deny_unknown_fields` still accepts it and
    /// serde defaults fill the rest.
    fn cfg_for(hostname: &str) -> InstallationConfig {
        serde_json::from_value(json!({
            "hostname": hostname,
            "disk_device": "/dev/nvme0n1",
            "timezone": "America/New_York",
            "luks_key": "changeme",
            "root_password": "changeme",
            "network_interface": "enp1s0f0",
            "network_address": "172.16.3.96/23",
            "network_gateway": "172.16.2.1",
            "network_search": "jf.local",
            "network_nameservers": ["172.16.2.1"],
        }))
        .expect("minimal InstallationConfig must deserialize")
    }

    fn canonical(cfg: &InstallationConfig) -> serde_json::Value {
        serde_json::to_value(cfg).unwrap()
    }

    #[tokio::test]
    async fn test_reify_then_resolve_round_trips_indexed_and_standalone() {
        let dir = tempfile::tempdir().unwrap();
        let store = SnapshotProfileStore::new(StatePaths::under(dir.path()));

        // Indexed group: reify in hostname-sorted order so allocation renders
        // 001/002/003 (the allocation-order contract).
        let indexed = [
            ("len-serv-001", "6c:4b:90:bc:39:b3"),
            ("len-serv-002", "6c:4b:90:bc:f8:a3"),
            ("len-serv-003", "6c:4b:90:bc:f7:f4"),
        ];
        for (host, mac) in indexed {
            let cfg = cfg_for(host);
            register_from_config(
                &store,
                "len-serv",
                "{name}-{index:03}",
                false,
                mac,
                None,
                &cfg,
                "op",
            )
            .await
            .unwrap_or_else(|e| panic!("reify {host}: {e}"));
        }

        // Standalone: pinned hostname_override, no allocation.
        let uni_cfg = cfg_for("unimatrixone");
        register_from_config(
            &store,
            "unimatrixone",
            "unimatrixone",
            true,
            "ac:1f:6b:40:fc:e2",
            Some("unimatrixone"),
            &uni_cfg,
            "op",
        )
        .await
        .unwrap();

        for host in ["len-serv-001", "len-serv-002", "len-serv-003", "unimatrixone"] {
            let resolved = resolve_from_registry(&store, host)
                .await
                .unwrap_or_else(|e| panic!("resolving {host}: {e}"));
            assert_eq!(
                canonical(&resolved),
                canonical(&cfg_for(host)),
                "reify→resolve must round-trip {host} exactly"
            );
        }
    }

    #[tokio::test]
    async fn test_reify_is_idempotent_across_reruns() {
        // The defining property of a shadow write on EVERY place / a re-run
        // backfill: converge on ONE group, ONE profile, ONE allocation per host,
        // with indices unchanged — never a duplicate or a renumber.
        let dir = tempfile::tempdir().unwrap();
        let store = SnapshotProfileStore::new(StatePaths::under(dir.path()));

        let indexed = [
            ("len-serv-001", "6c:4b:90:bc:39:b3"),
            ("len-serv-002", "6c:4b:90:bc:f8:a3"),
            ("len-serv-003", "6c:4b:90:bc:f7:f4"),
        ];

        let reify_all = || async {
            for (host, mac) in indexed {
                register_from_config(
                    &store,
                    "len-serv",
                    "{name}-{index:03}",
                    false,
                    mac,
                    None,
                    &cfg_for(host),
                    "op",
                )
                .await
                .unwrap();
            }
        };

        reify_all().await;
        let groups = store.list_groups().await.unwrap();
        assert_eq!(groups.len(), 1, "one len-serv group after first pass");
        let gid = groups[0].id;
        let indices_first: Vec<i64> = {
            let mut a = store.list_allocations(gid).await.unwrap();
            a.sort_by_key(|r| r.index);
            a.iter().map(|r| r.index).collect()
        };
        assert_eq!(indices_first, vec![1, 2, 3]);

        // Re-run: must NOT create a second group, duplicate any profile, or bump
        // any index.
        reify_all().await;
        assert_eq!(
            store.list_groups().await.unwrap().len(),
            1,
            "re-run must not mint a second group of the same name"
        );
        assert_eq!(
            store.list_profiles(gid).await.unwrap().len(),
            3,
            "re-run must not duplicate profiles (existing row id reused)"
        );
        let indices_second: Vec<i64> = {
            let mut a = store.list_allocations(gid).await.unwrap();
            a.sort_by_key(|r| r.index);
            a.iter().map(|r| r.index).collect()
        };
        assert_eq!(
            indices_second, indices_first,
            "re-run must not renumber (allocate-once)"
        );
    }

    #[tokio::test]
    async fn test_reify_out_of_order_indexed_host_fails_loud() {
        // Reifying an indexed host whose config hostname cannot be the next
        // rendered name (here: 002 into an empty group renders 001) must Err,
        // never silently bind the machine to the wrong hostname.
        let dir = tempfile::tempdir().unwrap();
        let store = SnapshotProfileStore::new(StatePaths::under(dir.path()));
        let err = register_from_config(
            &store,
            "len-serv",
            "{name}-{index:03}",
            false,
            "6c:4b:90:bc:f8:a3",
            None,
            &cfg_for("len-serv-002"),
            "op",
        )
        .await
        .unwrap_err();
        assert!(
            err.to_string().contains("hostname-sorted order"),
            "expected an allocation-order error, got: {err}"
        );
    }
}
