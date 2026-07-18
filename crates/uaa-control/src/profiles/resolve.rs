// file: crates/uaa-control/src/profiles/resolve.rs
// version: 0.1.0
// guid: 3f9c2b18-6d54-4a7e-b0c2-1e8d9a4f6572
// last-edited: 2026-07-18

//! Registry resolution for `uaa config place --from-registry` (DS-OPS-03).
//!
//! [`resolve_from_registry`] turns a target hostname into a fully-resolved
//! [`InstallationConfig`] by reading the profile store — the group defaults,
//! the host profile's overrides, and the host's EXISTING hostname allocation —
//! and running `uaa_core::profile::merge`. This is the RESOLVE half of the
//! task; the PLACE half (dry-run default, `.bak`-before-overwrite, the
//! `REPLACE_AT_PLACE_TIME` hard gate) stays in `uaa_core::config_place` and
//! operates on the [`InstallationConfig`] this function hands back.
//!
//! **Why resolution lives here, not in `uaa-core`:** it needs [`ProfileStore`],
//! the row→profile converters, and the allocation types — all of which live in
//! `uaa-control`, which already depends on `uaa-core`. Putting resolution in
//! `uaa-core` would be a dependency cycle. The `uaa` binary wires the two
//! halves together (nothing depends on the binary, so that is cycle-free).
//!
//! **Read-only and all-or-nothing.** Resolution looks up an ALREADY-bound
//! host; it never calls `allocate_index` (that would mutate the store and would
//! be wrong — it computes the *next* index, not this host's existing one) and
//! never falls back to a hand-authored `<host>.yaml`. Any missing group,
//! profile, or allocation is a loud `Err`, never a partial config.

use anyhow::{anyhow, Result};

use uaa_core::network::InstallationConfig;
use uaa_core::profile::merge::merge;

use crate::profiles::convert::{group_row_to_profile, profile_row_to_profile};
use crate::profiles::store::ProfileStore;

/// Resolve `host` (a target HOSTNAME, e.g. `"len-serv-001"`) into its full
/// [`InstallationConfig`] from the profile registry.
///
/// The host is located by its ACTIVE hostname allocation first (the normal
/// indexed case), falling back to a profile `hostname_override` (a pinned host
/// with no index allocation, e.g. a standalone box). The allocation is the
/// authority for the resolved hostname, so it is threaded into the host profile
/// before merge. Fails closed: an unreadable store, a missing profile, or a
/// merge that lacks a required field all return `Err` — never a half-built
/// config.
pub async fn resolve_from_registry(
    store: &dyn ProfileStore,
    host: &str,
) -> Result<InstallationConfig> {
    // Fail-closed read: `?` propagates an unreadable store rather than treating
    // it as an empty registry (which would make every host "not found").
    let groups = store.list_groups().await?;

    // 1. Locate the host by its active allocation (indexed hosts).
    for group in &groups {
        let allocations = store.list_allocations(group.id).await?;
        let Some(alloc) = allocations
            .iter()
            .find(|a| a.hostname == host && a.released_at.is_none() && a.rebound_to.is_none())
        else {
            continue;
        };

        let group_profile = group_row_to_profile(group).map_err(|e| anyhow!(e))?;
        let profiles = store.list_profiles(group.id).await?;
        let prow = profiles
            .iter()
            .find(|p| p.identity == alloc.identity)
            .ok_or_else(|| {
                anyhow!(
                    "host {host:?}: allocation identity {} has no host profile in group {:?}",
                    alloc.identity,
                    group.name
                )
            })?;
        let mut host_profile = profile_row_to_profile(prow, &group.name).map_err(|e| anyhow!(e))?;
        // The allocation is the source of truth for the hostname — merge keys
        // the hostname off `hostname_override`, so thread the allocated name in.
        host_profile.hostname_override = Some(alloc.hostname.clone());
        let (config, _provenance) =
            merge(&group_profile, &host_profile).map_err(|e| anyhow!(e.to_string()))?;
        return Ok(config);
    }

    // 2. Fall back to a pinned `hostname_override` (a host with no index
    //    allocation — e.g. a standalone group whose pattern is a fixed name).
    for group in &groups {
        let profiles = store.list_profiles(group.id).await?;
        let Some(prow) = profiles
            .iter()
            .find(|p| p.hostname_override.as_deref() == Some(host))
        else {
            continue;
        };
        let group_profile = group_row_to_profile(group).map_err(|e| anyhow!(e))?;
        let host_profile = profile_row_to_profile(prow, &group.name).map_err(|e| anyhow!(e))?;
        let (config, _provenance) =
            merge(&group_profile, &host_profile).map_err(|e| anyhow!(e.to_string()))?;
        return Ok(config);
    }

    Err(anyhow!(
        "host {host:?} is not in the profile registry: no active hostname allocation \
         and no matching hostname_override. Refusing rather than placing a stale config."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    use crate::db::store::StatePaths;
    use crate::db::HostGroupRow;
    use crate::profiles::store::{ProfileStore, SnapshotProfileStore};

    // The resolved-vs-committed M2 gate (`test_resolved_equals_committed_by_
    // struct_equality`) lives in the `uaa` crate, not here: it must read the
    // committed `examples/configs/install/*.yaml` (serde_yaml, not a uaa-control
    // dependency) and compare full configs (`InstallationConfig` deliberately
    // has no `PartialEq` — `TangServer` blocks it), which the `uaa` crate does
    // by canonical-serialization equality. These two tests cover resolution's
    // fail-loud behavior, which needs neither.

    fn group_row(id: Uuid, name: &str, pattern: &str) -> HostGroupRow {
        HostGroupRow {
            id,
            name: name.to_string(),
            hostname_pattern: pattern.to_string(),
            is_standalone: false,
            defaults: serde_json::json!({}),
            applications: serde_json::json!([]),
            content_hash: vec![],
            version: 1,
            created_at: None,
            updated_at: None,
        }
    }

    #[tokio::test]
    async fn test_known_host_missing_from_registry_errors() {
        // A host absent from the registry is a named Err, never a silent
        // empty/partial config.
        let dir = tempfile::tempdir().unwrap();
        let store = SnapshotProfileStore::new(StatePaths::under(dir.path()));
        // Bootstrap the snapshot with one unrelated group so the store is
        // readable (a genuinely missing snapshot is a different failure).
        store
            .put_group(group_row(Uuid::new_v4(), "len-serv", "{name}-{index:03}"), "op")
            .await
            .unwrap();

        let err = resolve_from_registry(&store, "no-such-host").await.unwrap_err();
        assert!(
            err.to_string().contains("not in the profile registry"),
            "expected a named not-in-registry error, got: {err}"
        );
    }

    #[tokio::test]
    async fn test_resolve_missing_profile_for_allocation_errors() {
        // An allocation exists but no profile carries its identity → loud Err,
        // never a partial config.
        let dir = tempfile::tempdir().unwrap();
        let store = SnapshotProfileStore::new(StatePaths::under(dir.path()));
        let gid = Uuid::new_v4();
        store
            .put_group(group_row(gid, "len-serv", "{name}-{index:03}"), "op")
            .await
            .unwrap();
        // Allocate a hostname WITHOUT ever creating a matching profile row.
        let alloc = store.allocate_index(gid, "aa:bb:cc:dd:ee:01").await.unwrap();
        assert_eq!(alloc.hostname, "len-serv-001");

        let err = resolve_from_registry(&store, "len-serv-001").await.unwrap_err();
        assert!(
            err.to_string().contains("has no host profile"),
            "expected a missing-profile error, got: {err}"
        );
    }
}
