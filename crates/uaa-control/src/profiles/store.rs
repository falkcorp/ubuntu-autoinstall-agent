// file: crates/uaa-control/src/profiles/store.rs
// version: 0.2.0
// guid: b0c81b2d-2a46-40e3-855b-408cf3708503
// last-edited: 2026-07-17

//! `ProfileStore` — per-concern persistence trait for host groups / profiles /
//! hostname allocations / profile versions (spec `deploy-system-design.md`,
//! DS-REG-01..04). Mirrors `saga.rs`'s `SagaStore` trait+twins shape (a narrow,
//! per-concern trait + a real impl + an in-memory twin) rather than growing
//! `db::registry::RegistryStore` — see the `crate::profiles` module doc and
//! spec Decision D5. `RegistryStore` is not touched by this file.
//!
//! [`SnapshotProfileStore`] is the real impl. It reads through
//! [`read_snapshot_strict`] — fail-CLOSED — never the fail-open
//! [`read_snapshot`][crate::db::store::read_snapshot] telemetry uses. See that
//! function's doc for why: an allocator that ever saw an empty view would
//! conclude every hostname index is free and re-allocate the entire fleet from
//! 1, renaming every machine. Every read-modify-write mutation here uses the
//! same strict read (via the private `read_for_mutation` helper, which additionally
//! tolerates a genuinely MISSING file — first-ever write on a fresh install — as
//! bootstrap, while still refusing to write over a file that exists but is
//! corrupt or otherwise unreadable). Writes go through
//! [`write_snapshot`][crate::db::store::write_snapshot], the same atomic
//! tmp+rename primitive every other mutation path in this crate uses; no
//! hand-rolled file IO.
//!
//! [`MemProfileStore`] — the in-memory twin — is deliberately
//! **`#[cfg(test)]`-gated**, UNLIKE `MemRegistryStore`/`MemAuditStore`, which
//! are always compiled because `default_state()` builds them in production as
//! a "degrade rather than fail to start" fallback. That convention must NOT
//! extend here: if `MemProfileStore` were reachable from production wiring,
//! someone would eventually write
//! `SnapshotProfileStore::new(..).unwrap_or_else(|_| MemProfileStore::new())`
//! — and an empty profile store hits the exact same fleet-wide-rename failure
//! mode described above. `#[cfg(test)]` makes that wiring fail to COMPILE
//! instead of fail at 3am. This is a deliberate, documented divergence from
//! the rest of the crate's Mem* convention.
//!
//! `allocate_index`/`rebind` are DS-REG-03's job. [`SnapshotProfileStore`]'s
//! bodies are loud `unimplemented!` stubs here — never a silent `Ok` that would
//! quietly pretend allocation works. [`MemProfileStore`]'s bodies ARE real
//! (simple, test-only) implementations, because this module's own tests need a
//! working store to exercise `delete_group`'s non-cascade behavior end to end.

#[cfg(test)]
use std::sync::Mutex;

use anyhow::Result;
use async_trait::async_trait;
use uuid::Uuid;

use crate::db::store::{read_snapshot_strict, write_snapshot, SnapshotDoc, StatePaths, StoreError};
use crate::db::{HostGroupRow, HostProfileRow, HostnameAllocationRow, ProfileVersionRow};

/// Persistence for host groups / profiles / hostname allocations / profile
/// versions. Mirrors `saga.rs`'s `SagaStore` — a narrow, per-concern trait, not
/// a growth of `RegistryStore` (whose `RecordingRegistry` test double in
/// `saga.rs` hand-forwards every method; adding profile methods there would
/// break it).
#[async_trait]
pub trait ProfileStore: Send + Sync {
    /// Every host group. Fail-closed: see module doc.
    async fn list_groups(&self) -> Result<Vec<HostGroupRow>>;
    /// A single group by its immutable `name`, or `None` if absent.
    async fn get_group(&self, name: &str) -> Result<Option<HostGroupRow>>;
    /// Upsert a group row, keyed by `id` (never `name` — see [`HostGroupRow`] doc).
    async fn put_group(&self, row: HostGroupRow) -> Result<()>;
    /// Removes the group row and cascades to `host_profiles` ONLY.
    /// `hostname_allocations` MUST survive this call — see module doc / spec
    /// D8: allocations outliving groups is what makes delete-and-recreate
    /// re-attach machines to the index they already had.
    async fn delete_group(&self, name: &str) -> Result<()>;
    /// Every profile belonging to `group_id`.
    async fn list_profiles(&self, group_id: Uuid) -> Result<Vec<HostProfileRow>>;
    /// Upsert a profile row, keyed by `id`.
    async fn put_profile(&self, row: HostProfileRow) -> Result<()>;
    /// Every hostname allocation belonging to `group_id`, including released /
    /// rebound-away rows (append-only history) — the fail-closed read
    /// DS-REG-03's allocator depends on.
    async fn list_allocations(&self, group_id: Uuid) -> Result<Vec<HostnameAllocationRow>>;
    /// DS-REG-03. A loud stub in [`SnapshotProfileStore`] — never a silent `Ok`.
    async fn allocate_index(&self, group_id: Uuid, identity: &str) -> Result<HostnameAllocationRow>;
    /// DS-REG-03. A loud stub in [`SnapshotProfileStore`] — never a silent `Ok`.
    async fn rebind(
        &self,
        group_id: Uuid,
        old_identity: &str,
        new_identity: &str,
    ) -> Result<HostnameAllocationRow>;
    /// Every prior version of `object_id` (a group or profile), oldest first.
    async fn list_versions(&self, object_id: Uuid) -> Result<Vec<ProfileVersionRow>>;
    /// Upsert a version row, keyed by `id`. Callers must never mutate a
    /// version already written — this trait does not enforce that; it is a
    /// caller-side invariant (spec Decisions 10/11).
    async fn put_version(&self, row: ProfileVersionRow) -> Result<()>;
}

// ── SnapshotProfileStore — real impl, backed by the StatePaths snapshot ────

/// Real [`ProfileStore`]: reads/writes `SnapshotDoc.host_groups` /
/// `host_profiles` / `hostname_allocations` / `profile_versions` in the
/// `StatePaths` JSON snapshot. `uaa-control` has no database connection in
/// production (spec D4) — this snapshot IS the system of record for profiles,
/// not a degraded-mode cache of one, which is why (unlike `db::store`'s
/// `guarded_mutation`) writes here are never gated behind a CRDB health check:
/// there is no CRDB for profiles to be degraded from.
pub struct SnapshotProfileStore {
    paths: StatePaths,
}

impl SnapshotProfileStore {
    pub fn new(paths: StatePaths) -> Self {
        Self { paths }
    }

    /// Strict read for a mutation's read-modify-write cycle. Like
    /// [`read_snapshot_strict`], EXCEPT a genuinely missing file — the
    /// first-ever write on a fresh install, before any snapshot has been
    /// written by anything in the process — is tolerated as an empty doc to
    /// bootstrap from. Every OTHER failure (corrupt file, permission error)
    /// still aborts: a mutation must never clobber an existing-but-currently-
    /// unreadable snapshot with near-empty data, which is exactly the
    /// fail-open hazard this whole task exists to avoid.
    fn read_for_mutation(&self) -> Result<SnapshotDoc> {
        match read_snapshot_strict(&self.paths) {
            Ok(doc) => Ok(doc),
            Err(StoreError::SnapshotUnreadable { source, .. })
                if source.kind() == std::io::ErrorKind::NotFound =>
            {
                Ok(SnapshotDoc::default())
            }
            Err(err) => Err(err.into()),
        }
    }
}

#[async_trait]
impl ProfileStore for SnapshotProfileStore {
    async fn list_groups(&self) -> Result<Vec<HostGroupRow>> {
        Ok(read_snapshot_strict(&self.paths)?.host_groups)
    }

    async fn get_group(&self, name: &str) -> Result<Option<HostGroupRow>> {
        Ok(read_snapshot_strict(&self.paths)?
            .host_groups
            .into_iter()
            .find(|g| g.name == name))
    }

    async fn put_group(&self, row: HostGroupRow) -> Result<()> {
        let mut doc = self.read_for_mutation()?;
        match doc.host_groups.iter_mut().find(|g| g.id == row.id) {
            Some(existing) => *existing = row,
            None => doc.host_groups.push(row),
        }
        write_snapshot(&self.paths, &doc)?;
        Ok(())
    }

    async fn delete_group(&self, name: &str) -> Result<()> {
        let mut doc = self.read_for_mutation()?;
        let group_id = doc
            .host_groups
            .iter()
            .find(|g| g.name == name)
            .map(|g| g.id);
        doc.host_groups.retain(|g| g.name != name);
        if let Some(group_id) = group_id {
            // Cascade to host_profiles ONLY. hostname_allocations is
            // deliberately left untouched — see trait doc / module doc / spec
            // D8. Do NOT "fix" this to also filter hostname_allocations.
            doc.host_profiles.retain(|p| p.group_id != group_id);
        }
        write_snapshot(&self.paths, &doc)?;
        Ok(())
    }

    async fn list_profiles(&self, group_id: Uuid) -> Result<Vec<HostProfileRow>> {
        Ok(read_snapshot_strict(&self.paths)?
            .host_profiles
            .into_iter()
            .filter(|p| p.group_id == group_id)
            .collect())
    }

    async fn put_profile(&self, row: HostProfileRow) -> Result<()> {
        let mut doc = self.read_for_mutation()?;
        match doc.host_profiles.iter_mut().find(|p| p.id == row.id) {
            Some(existing) => *existing = row,
            None => doc.host_profiles.push(row),
        }
        write_snapshot(&self.paths, &doc)?;
        Ok(())
    }

    async fn list_allocations(&self, group_id: Uuid) -> Result<Vec<HostnameAllocationRow>> {
        Ok(read_snapshot_strict(&self.paths)?
            .hostname_allocations
            .into_iter()
            .filter(|a| a.group_id == group_id)
            .collect())
    }

    async fn allocate_index(&self, _group_id: Uuid, _identity: &str) -> Result<HostnameAllocationRow> {
        unimplemented!("DS-REG-03")
    }

    async fn rebind(
        &self,
        _group_id: Uuid,
        _old_identity: &str,
        _new_identity: &str,
    ) -> Result<HostnameAllocationRow> {
        unimplemented!("DS-REG-03")
    }

    async fn list_versions(&self, object_id: Uuid) -> Result<Vec<ProfileVersionRow>> {
        Ok(read_snapshot_strict(&self.paths)?
            .profile_versions
            .into_iter()
            .filter(|v| v.object_id == object_id)
            .collect())
    }

    async fn put_version(&self, row: ProfileVersionRow) -> Result<()> {
        let mut doc = self.read_for_mutation()?;
        match doc.profile_versions.iter_mut().find(|v| v.id == row.id) {
            Some(existing) => *existing = row,
            None => doc.profile_versions.push(row),
        }
        write_snapshot(&self.paths, &doc)?;
        Ok(())
    }
}

// ── MemProfileStore — in-memory twin, tests ONLY ───────────────────────────

/// In-memory state backing [`MemProfileStore`]. Test-only, gated alongside it.
#[cfg(test)]
#[derive(Debug, Default)]
struct MemProfileState {
    host_groups: Vec<HostGroupRow>,
    host_profiles: Vec<HostProfileRow>,
    hostname_allocations: Vec<HostnameAllocationRow>,
    profile_versions: Vec<ProfileVersionRow>,
}

/// In-memory [`ProfileStore`] twin for tests (zero filesystem IO). Deliberately
/// `#[cfg(test)]`-gated — see the module doc for why this breaks from
/// `MemRegistryStore`/`MemAuditStore`'s always-compiled convention: a
/// production-reachable empty profile store is a fleet-wide-rename bug waiting
/// to happen, so this type must not exist outside `cargo test`.
///
/// Unlike [`SnapshotProfileStore`], `allocate_index`/`rebind` here are REAL
/// (simple, test-only) implementations, not stubs — this module's own tests
/// need a working store end to end (e.g. to seed an allocation before
/// asserting `delete_group` leaves it alone).
#[cfg(test)]
#[derive(Debug, Default)]
pub struct MemProfileStore {
    state: Mutex<MemProfileState>,
}

#[cfg(test)]
impl MemProfileStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[cfg(test)]
#[async_trait]
impl ProfileStore for MemProfileStore {
    async fn list_groups(&self) -> Result<Vec<HostGroupRow>> {
        Ok(self
            .state
            .lock()
            .expect("MemProfileStore state poisoned")
            .host_groups
            .clone())
    }

    async fn get_group(&self, name: &str) -> Result<Option<HostGroupRow>> {
        Ok(self
            .state
            .lock()
            .expect("MemProfileStore state poisoned")
            .host_groups
            .iter()
            .find(|g| g.name == name)
            .cloned())
    }

    async fn put_group(&self, row: HostGroupRow) -> Result<()> {
        let mut state = self.state.lock().expect("MemProfileStore state poisoned");
        match state.host_groups.iter_mut().find(|g| g.id == row.id) {
            Some(existing) => *existing = row,
            None => state.host_groups.push(row),
        }
        Ok(())
    }

    async fn delete_group(&self, name: &str) -> Result<()> {
        let mut state = self.state.lock().expect("MemProfileStore state poisoned");
        let group_id = state
            .host_groups
            .iter()
            .find(|g| g.name == name)
            .map(|g| g.id);
        state.host_groups.retain(|g| g.name != name);
        if let Some(group_id) = group_id {
            // Cascade to host_profiles ONLY — hostname_allocations survives.
            // Same asymmetry as SnapshotProfileStore; see module/trait doc.
            state.host_profiles.retain(|p| p.group_id != group_id);
        }
        Ok(())
    }

    async fn list_profiles(&self, group_id: Uuid) -> Result<Vec<HostProfileRow>> {
        Ok(self
            .state
            .lock()
            .expect("MemProfileStore state poisoned")
            .host_profiles
            .iter()
            .filter(|p| p.group_id == group_id)
            .cloned()
            .collect())
    }

    async fn put_profile(&self, row: HostProfileRow) -> Result<()> {
        let mut state = self.state.lock().expect("MemProfileStore state poisoned");
        match state.host_profiles.iter_mut().find(|p| p.id == row.id) {
            Some(existing) => *existing = row,
            None => state.host_profiles.push(row),
        }
        Ok(())
    }

    async fn list_allocations(&self, group_id: Uuid) -> Result<Vec<HostnameAllocationRow>> {
        Ok(self
            .state
            .lock()
            .expect("MemProfileStore state poisoned")
            .hostname_allocations
            .iter()
            .filter(|a| a.group_id == group_id)
            .cloned()
            .collect())
    }

    async fn allocate_index(&self, group_id: Uuid, identity: &str) -> Result<HostnameAllocationRow> {
        let mut state = self.state.lock().expect("MemProfileStore state poisoned");

        // Idempotent: an identity that already holds an active allocation in
        // this group gets the same row back rather than a second index.
        if let Some(existing) = state.hostname_allocations.iter().find(|a| {
            a.group_id == group_id
                && a.identity == identity
                && a.released_at.is_none()
                && a.rebound_to.is_none()
        }) {
            return Ok(existing.clone());
        }

        let next_index = state
            .hostname_allocations
            .iter()
            .filter(|a| a.group_id == group_id)
            .map(|a| a.index)
            .max()
            .map(|max| max + 1)
            .unwrap_or(1);
        let hostname = format!("mem-group-{group_id}-{next_index:03}");

        let row = HostnameAllocationRow {
            group_id,
            identity: identity.to_string(),
            index: next_index,
            hostname,
            allocated_at: None,
            released_at: None,
            rebound_to: None,
        };
        state.hostname_allocations.push(row.clone());
        Ok(row)
    }

    async fn rebind(
        &self,
        group_id: Uuid,
        old_identity: &str,
        new_identity: &str,
    ) -> Result<HostnameAllocationRow> {
        let mut state = self.state.lock().expect("MemProfileStore state poisoned");

        let (index, hostname) = {
            let existing = state
                .hostname_allocations
                .iter_mut()
                .find(|a| {
                    a.group_id == group_id && a.identity == old_identity && a.rebound_to.is_none()
                })
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "no active allocation for identity {old_identity} in group {group_id}"
                    )
                })?;
            existing.rebound_to = Some(new_identity.to_string());
            (existing.index, existing.hostname.clone())
        };

        let row = HostnameAllocationRow {
            group_id,
            identity: new_identity.to_string(),
            index,
            hostname,
            allocated_at: None,
            released_at: None,
            rebound_to: None,
        };
        state.hostname_allocations.push(row.clone());
        Ok(row)
    }

    async fn list_versions(&self, object_id: Uuid) -> Result<Vec<ProfileVersionRow>> {
        Ok(self
            .state
            .lock()
            .expect("MemProfileStore state poisoned")
            .profile_versions
            .iter()
            .filter(|v| v.object_id == object_id)
            .cloned()
            .collect())
    }

    async fn put_version(&self, row: ProfileVersionRow) -> Result<()> {
        let mut state = self.state.lock().expect("MemProfileStore state poisoned");
        match state.profile_versions.iter_mut().find(|v| v.id == row.id) {
            Some(existing) => *existing = row,
            None => state.profile_versions.push(row),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn sample_group(id: Uuid, name: &str) -> HostGroupRow {
        HostGroupRow {
            id,
            name: name.to_string(),
            hostname_pattern: "{name}-{index:03}".into(),
            is_standalone: false,
            defaults: serde_json::json!({}),
            applications: serde_json::json!([]),
            content_hash: vec![0xab],
            version: 1,
            created_at: None,
            updated_at: None,
        }
    }

    fn sample_profile(id: Uuid, group_id: Uuid, identity: &str) -> HostProfileRow {
        HostProfileRow {
            id,
            group_id,
            identity: identity.to_string(),
            hostname_override: None,
            overrides: serde_json::json!({}),
            applications: serde_json::json!([]),
            content_hash: vec![0x12],
            version: 1,
            created_at: None,
            updated_at: None,
        }
    }

    #[tokio::test]
    async fn test_snapshot_profile_store_put_and_list_groups() {
        let dir = tempdir().unwrap();
        let store = SnapshotProfileStore::new(StatePaths::under(dir.path()));
        let group_id = Uuid::new_v4();
        store.put_group(sample_group(group_id, "len-serv")).await.unwrap();

        let groups = store.list_groups().await.unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].id, group_id);

        let found = store.get_group("len-serv").await.unwrap();
        assert!(found.is_some(), "get_group must find the row by name");
    }

    #[tokio::test]
    async fn test_snapshot_profile_store_bootstraps_on_missing_file() {
        // No snapshot file exists yet anywhere under this tempdir: the very
        // first write must still succeed (fresh install), not error out
        // because read_for_mutation's strict read found nothing.
        let dir = tempdir().unwrap();
        let store = SnapshotProfileStore::new(StatePaths::under(dir.path()));
        let group_id = Uuid::new_v4();
        store
            .put_group(sample_group(group_id, "bootstrap"))
            .await
            .expect("first write on a fresh install must succeed");
    }

    #[tokio::test]
    #[should_panic(expected = "DS-REG-03")]
    async fn test_allocate_index_is_unimplemented_stub() {
        let dir = tempdir().unwrap();
        let store = SnapshotProfileStore::new(StatePaths::under(dir.path()));
        let _ = store.allocate_index(Uuid::new_v4(), "aa:bb:cc:dd:ee:ff").await;
    }

    #[tokio::test]
    async fn test_delete_group_leaves_allocations() {
        let store = MemProfileStore::new();
        let group_id = Uuid::new_v4();
        store.put_group(sample_group(group_id, "len-serv")).await.unwrap();
        store
            .put_profile(sample_profile(Uuid::new_v4(), group_id, "aa:bb:cc:dd:ee:ff"))
            .await
            .unwrap();
        store
            .allocate_index(group_id, "aa:bb:cc:dd:ee:ff")
            .await
            .unwrap();

        store.delete_group("len-serv").await.unwrap();

        assert!(
            store.get_group("len-serv").await.unwrap().is_none(),
            "group must be gone"
        );
        assert!(
            store.list_profiles(group_id).await.unwrap().is_empty(),
            "profiles must cascade-delete with the group"
        );
        let allocations = store.list_allocations(group_id).await.unwrap();
        assert_eq!(
            allocations.len(),
            1,
            "hostname_allocations must survive group deletion"
        );
        assert_eq!(allocations[0].identity, "aa:bb:cc:dd:ee:ff");
    }

    #[tokio::test]
    async fn test_mem_profile_store_rebind_tombstones_old_row() {
        let store = MemProfileStore::new();
        let group_id = Uuid::new_v4();
        let first = store.allocate_index(group_id, "old-identity").await.unwrap();

        let rebound = store
            .rebind(group_id, "old-identity", "new-identity")
            .await
            .unwrap();

        assert_eq!(rebound.index, first.index, "rebind must keep the same index");
        assert_eq!(rebound.identity, "new-identity");

        let all = store.list_allocations(group_id).await.unwrap();
        assert_eq!(all.len(), 2, "old row is tombstoned, not removed");
        let old_row = all.iter().find(|a| a.identity == "old-identity").unwrap();
        assert_eq!(old_row.rebound_to.as_deref(), Some("new-identity"));
    }
}
