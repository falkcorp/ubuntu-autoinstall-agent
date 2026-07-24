// file: crates/uaa-control/src/profiles/resolve.rs
// version: 0.3.0
// guid: 3f9c2b18-6d54-4a7e-b0c2-1e8d9a4f6572
// last-edited: 2026-07-24

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

use uaa_core::autoinstall::host_spec::HostSpec;
use uaa_core::network::ssh_installer::config::ApplicationSpec;
use uaa_core::network::InstallationConfig;
use uaa_core::profile::merge::merge;
use uaa_core::profile::HostGroupProfile;

use crate::db::HostGroupRow;
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
        let (mut config, _provenance) =
            merge(&group_profile, &host_profile).map_err(|e| anyhow!(e.to_string()))?;
        if has_cockroach_application(&config) {
            config.cockroach_members = cockroach_member_ips(store, group, &group_profile).await?;
        }
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
        let (mut config, _provenance) =
            merge(&group_profile, &host_profile).map_err(|e| anyhow!(e.to_string()))?;
        if has_cockroach_application(&config) {
            config.cockroach_members = cockroach_member_ips(store, group, &group_profile).await?;
        }
        return Ok(config);
    }

    Err(anyhow!(
        "host {host:?} is not in the profile registry: no active hostname allocation \
         and no matching hostname_override. Refusing rather than placing a stale config."
    ))
}

/// `true` if `config` carries a Cockroach application — the only case
/// [`cockroach_member_ips`] needs to run. Every other host's
/// `cockroach_members` stays empty (and, via `skip_serializing_if`, out of
/// the serialized config entirely) so this is a no-op for the rest of the
/// fleet.
fn has_cockroach_application(config: &InstallationConfig) -> bool {
    config
        .applications
        .iter()
        .any(|a| matches!(a, ApplicationSpec::Cockroach(_)))
}

/// Populate `InstallationConfig::cockroach_members` (PS-COCKROACH-16): the
/// bare (no-CIDR) IPs of `group`'s currently-ACTIVE hostname allocations
/// (never released, never rebound-away), in ascending allocation-index
/// order — the roster `applications.rs::derive_cockroach_endpoints`
/// consumes to build this host's advertise/join strings.
///
/// Each member's `network_address` is resolved by running `merge()` against
/// ITS OWN host profile (not copied from the allocation, which carries no IP)
/// so a per-host static-address override is respected exactly as it would be
/// if that host were the one being resolved directly.
///
/// A member whose active allocation has no matching host profile row is
/// SKIPPED rather than failing this (unrelated) host's resolve — a
/// deliberately weaker guarantee than `resolve_from_registry`'s own
/// fail-closed lookup, which errors loudly for the TARGET host precisely
/// because a caller is resolving that host by name and must not silently get
/// a placeholder config. Here, the target already resolved successfully, so
/// letting one broken sibling's MISSING PROFILE drop it from the roster
/// (rather than failing every other node's re-resolve) is the more useful
/// failure mode. A sibling that HAS a profile row but whose own `merge()`
/// fails still propagates via `?` and fails this call — only the
/// missing-row case is soft. Untriggered by any config committed today (no
/// host authors a Cockroach application yet).
async fn cockroach_member_ips(
    store: &dyn ProfileStore,
    group: &HostGroupRow,
    group_profile: &HostGroupProfile,
) -> Result<Vec<String>> {
    let mut allocations: Vec<_> = store
        .list_allocations(group.id)
        .await?
        .into_iter()
        .filter(|a| a.released_at.is_none() && a.rebound_to.is_none())
        .collect();
    allocations.sort_by_key(|a| a.index);

    let profiles = store.list_profiles(group.id).await?;
    let mut members = Vec::with_capacity(allocations.len());
    for alloc in &allocations {
        let Some(prow) = profiles.iter().find(|p| p.identity == alloc.identity) else {
            continue;
        };
        let mut member_profile = profile_row_to_profile(prow, &group.name).map_err(|e| anyhow!(e))?;
        member_profile.hostname_override = Some(alloc.hostname.clone());
        let (member_config, _provenance) =
            merge(group_profile, &member_profile).map_err(|e| anyhow!(e.to_string()))?;
        members.push(HostSpec::ip_without_cidr(&member_config.network_address).to_string());
    }
    Ok(members)
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
            schema_version: 0,
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

    /// PS-COCKROACH-16 gate: `cockroach_members` must be populated from the
    /// group's ACTIVE allocation roster, in ascending index order, and the
    /// resulting derived (advertise, join) for len-serv-001 must equal the
    /// exact literal strings the retired hardcoded fleet-member-IPs constant
    /// produced for that host (see `applications.rs`'s
    /// `test_cockroach_join_matches_former_lenserv_member_ips_constant` and
    /// `host_spec.rs`'s `for_lenserv_matches_known_hosts`).
    #[tokio::test]
    async fn test_cockroach_members_populated_from_group_roster() {
        use crate::db::HostProfileRow;
        use uaa_core::network::ssh_installer::applications::derive_cockroach_endpoints;
        use uaa_core::network::ssh_installer::config::CockroachSpec;

        let dir = tempfile::tempdir().unwrap();
        let store = SnapshotProfileStore::new(StatePaths::under(dir.path()));
        let gid = Uuid::new_v4();

        let cockroach = ApplicationSpec::Cockroach(CockroachSpec {
            version: "v25.3.0".to_string(),
            port: uaa_core::autoinstall::host_spec::COCKROACH_PORT,
            sql_port: 36257,
            http_addr: ":38080".to_string(),
            seed_ip: uaa_core::autoinstall::host_spec::COCKROACH_SERVER_IP.to_string(),
            cache: ".25".to_string(),
            max_sql_memory: ".25".to_string(),
            locality: "region=us,cluster-unit=lenovo".to_string(),
        });

        let mut group = group_row(gid, "len-serv", "{name}-{index:03}");
        // Every host in the group runs Cockroach — authored once on the
        // group, inherited by each host via `union_applications`.
        group.applications = serde_json::to_value(vec![cockroach]).unwrap();
        group.defaults = serde_json::json!({
            "disk_device": "/dev/nvme0n1",
            "timezone": "America/New_York",
            "luks_key": "REPLACE_AT_PLACE_TIME",
            "root_password": "REPLACE_AT_PLACE_TIME",
            "network_interface": "enp1s0f0",
            "network_gateway": "172.16.2.1",
            "network_search": "jf.local",
            "network_nameservers": ["172.16.2.1"],
        });
        store.put_group(group, "op").await.unwrap();

        let member_ips = ["172.16.3.92", "172.16.3.94", "172.16.3.96"];
        for (i, ip) in member_ips.iter().enumerate() {
            let mac = format!("aa:bb:cc:dd:ee:{:02x}", i);
            let alloc = store.allocate_index(gid, &mac).await.unwrap();
            assert_eq!(alloc.hostname, format!("len-serv-{:03}", i + 1));
            let profile = HostProfileRow {
                id: Uuid::new_v4(),
                group_id: gid,
                identity: mac,
                hostname_override: None,
                overrides: serde_json::json!({ "network_address": format!("{ip}/23") }),
                applications: serde_json::json!([]),
                content_hash: vec![],
                version: 1,
                schema_version: 0,
                created_at: None,
                updated_at: None,
            };
            store.put_profile(profile, "op").await.unwrap();
        }

        let config = resolve_from_registry(&store, "len-serv-001").await.unwrap();
        assert_eq!(
            config.cockroach_members,
            vec![
                "172.16.3.92".to_string(),
                "172.16.3.94".to_string(),
                "172.16.3.96".to_string(),
            ],
            "cockroach_members must be the group's active roster in ascending index order"
        );

        let cockroach_spec = match &config.applications[..] {
            [ApplicationSpec::Cockroach(c)] => c.clone(),
            other => panic!("expected exactly one Cockroach application, got {other:?}"),
        };
        let (advertise, join) = derive_cockroach_endpoints(
            &config.network_address,
            &config.cockroach_members,
            &cockroach_spec,
        );
        assert_eq!(advertise, "172.16.3.92:36357");
        assert_eq!(
            join, "172.16.2.30:36357,172.16.3.94:36357,172.16.3.96:36357",
            "must equal the former hardcoded-constant-derived join for len-serv-001"
        );
    }
}
