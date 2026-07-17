// file: crates/uaa-control/src/profiles/store.rs
// version: 0.4.0
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

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use uuid::Uuid;

use crate::audit::{AuditStore, NewAuditEvent};
use crate::db::store::{read_snapshot_strict, write_snapshot, SnapshotDoc, StatePaths, StoreError};
use crate::db::{HostGroupRow, HostProfileRow, HostnameAllocationRow, ProfileVersionRow};
use crate::import_export::normalize_mac;
use crate::profiles::alloc::{next_index, render_hostname, taken_hostnames};

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
    /// `content_hash` is (re)computed from the row's content on every call —
    /// a caller-supplied `content_hash` is ignored — and a
    /// [`ProfileVersionRow`] is captured for `actor` on EVERY call, not only
    /// when the body actually changed (DS-REG-04; see `profiles::drift`
    /// module doc for why capture-on-change is too late for revert).
    async fn put_group(&self, row: HostGroupRow, actor: &str) -> Result<()>;
    /// Removes the group row and cascades to `host_profiles` ONLY.
    /// `hostname_allocations` MUST survive this call — see module doc / spec
    /// D8: allocations outliving groups is what makes delete-and-recreate
    /// re-attach machines to the index they already had.
    async fn delete_group(&self, name: &str) -> Result<()>;
    /// Every profile belonging to `group_id`.
    async fn list_profiles(&self, group_id: Uuid) -> Result<Vec<HostProfileRow>>;
    /// Upsert a profile row, keyed by `id`. Same `content_hash` /
    /// `capture_version` contract as [`put_group`][ProfileStore::put_group].
    async fn put_profile(&self, row: HostProfileRow, actor: &str) -> Result<()>;
    /// Every hostname allocation belonging to `group_id`, including released /
    /// rebound-away rows (append-only history) — the fail-closed read
    /// DS-REG-03's allocator depends on.
    async fn list_allocations(&self, group_id: Uuid) -> Result<Vec<HostnameAllocationRow>>;
    /// DS-REG-03. Allocate-once: a bound identity returns its existing row
    /// unchanged (writing nothing); an unbound one gets `max(index)+1`; a
    /// released one is reactivated at the SAME index. Reads fail CLOSED via
    /// `read_snapshot_strict` — an unreadable snapshot is an `Err`, never an
    /// allocate-from-1.
    async fn allocate_index(&self, group_id: Uuid, identity: &str) -> Result<HostnameAllocationRow>;
    /// DS-REG-03 / spec D18 — the NIC-replacement runbook and the one deliberate
    /// exception to append-only. Moves the existing index+hostname to
    /// `new_identity` and tombstones the old row, audited via
    /// [`AuditStore::append_in_txn`] with the caller-supplied `actor`.
    ///
    /// NOTE (deliberate deviation from the DS-REG-03 brief's 3-arg sketch): the
    /// `audit` store and `actor` are threaded as parameters, mirroring the
    /// crate's audited-mutation convention (`enroll::approve(..., audit, &session.login)`).
    /// The actor is the per-request operator login; it cannot be a store field.
    /// Operator-gating (`Role::Operator`) is the caller's job (DS-OPS-01).
    async fn rebind(
        &self,
        audit: &dyn AuditStore,
        actor: &str,
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

/// Current wall-clock time as an RFC3339 string, for `allocated_at` audit
/// stamps. Ordering/debug only — never a key.
fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
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

    /// **Write-path only; allocation must use `read_snapshot_strict` directly.**
    /// This helper fails OPEN on a missing file (returns an empty doc to
    /// bootstrap a first write) — CORRECT for `put_*`, CATASTROPHIC for
    /// `allocate_index`/`rebind`, where a missing snapshot read as empty would
    /// conclude every index is free and re-allocate the whole fleet from 1.
    ///
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

    async fn put_group(&self, mut row: HostGroupRow, actor: &str) -> Result<()> {
        let body = crate::profiles::drift::group_body(&row);
        row.content_hash = crate::profiles::drift::content_hash(&body)?.to_vec();
        let group_id = row.id;

        // Write the live row FIRST: capture_version's list_versions() reads
        // via read_snapshot_strict (fail-closed), which — unlike
        // read_for_mutation below — does not tolerate a not-yet-existing
        // snapshot file. On a fresh install this write is what creates the
        // file capture_version needs to read.
        let mut doc = self.read_for_mutation()?;
        match doc.host_groups.iter_mut().find(|g| g.id == row.id) {
            Some(existing) => *existing = row,
            None => doc.host_groups.push(row),
        }
        write_snapshot(&self.paths, &doc)?;

        // Capture the version on EVERY write, unconditionally (DS-REG-04;
        // see profiles::drift module doc for why capture-on-change is too
        // late for DS-REG-05's revert).
        crate::profiles::drift::capture_version(
            self,
            crate::profiles::drift::OBJECT_KIND_HOST_GROUP,
            group_id,
            &body,
            actor,
        )
        .await?;
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

    async fn put_profile(&self, mut row: HostProfileRow, actor: &str) -> Result<()> {
        let body = crate::profiles::drift::profile_body(&row);
        row.content_hash = crate::profiles::drift::content_hash(&body)?.to_vec();
        let profile_id = row.id;

        // Write the live row first — see put_group's comment on why this
        // must precede capture_version (fail-closed list_versions read).
        let mut doc = self.read_for_mutation()?;
        match doc.host_profiles.iter_mut().find(|p| p.id == row.id) {
            Some(existing) => *existing = row,
            None => doc.host_profiles.push(row),
        }
        write_snapshot(&self.paths, &doc)?;

        crate::profiles::drift::capture_version(
            self,
            crate::profiles::drift::OBJECT_KIND_HOST_PROFILE,
            profile_id,
            &body,
            actor,
        )
        .await?;
        Ok(())
    }

    async fn list_allocations(&self, group_id: Uuid) -> Result<Vec<HostnameAllocationRow>> {
        Ok(read_snapshot_strict(&self.paths)?
            .hostname_allocations
            .into_iter()
            .filter(|a| a.group_id == group_id)
            .collect())
    }

    /// Allocate-once hostname index for `(group_id, normalize_mac(identity))`.
    ///
    /// Reads via [`read_snapshot_strict`] — **never** `read_for_mutation` (which
    /// fails open) — so an unreadable snapshot is refused, never treated as an
    /// empty registry that would re-allocate the fleet from 1. Follows
    /// DS-REG-02's lock-free read-compute-write pattern (`guarded_mutation` is
    /// not usable here: it gates on a `DbHealth` this store does not have and
    /// holds no lock). There is therefore NO in-process serialization; under
    /// spec D4 `uaa-control` is single-writer, exactly as every other mutation
    /// in this store already assumes.
    async fn allocate_index(&self, group_id: Uuid, identity: &str) -> Result<HostnameAllocationRow> {
        let identity = normalize_mac(identity);
        let mut doc = read_snapshot_strict(&self.paths)?;

        // Allocate-once: at most one non-tombstoned row per (group, identity).
        if let Some(pos) = doc.hostname_allocations.iter().position(|a| {
            a.group_id == group_id && a.identity == identity && a.rebound_to.is_none()
        }) {
            if doc.hostname_allocations[pos].released_at.is_some() {
                // Returning machine: reactivate at the SAME index.
                doc.hostname_allocations[pos].released_at = None;
                let row = doc.hostname_allocations[pos].clone();
                write_snapshot(&self.paths, &doc)?;
                return Ok(row);
            }
            // Already bound and active: idempotent no-op, write NOTHING.
            return Ok(doc.hostname_allocations[pos].clone());
        }

        // Unbound: max(index)+1 over EVERY row in the group (never lowest-free).
        let group_rows: Vec<HostnameAllocationRow> = doc
            .hostname_allocations
            .iter()
            .filter(|a| a.group_id == group_id)
            .cloned()
            .collect();
        let index = next_index(&group_rows);

        let group = doc
            .host_groups
            .iter()
            .find(|g| g.id == group_id)
            .ok_or_else(|| anyhow!("no host group with id {group_id}"))?;
        let hostname = render_hostname(&group.hostname_pattern, &group.name, index)?;

        // Global uniqueness (spec D2): the materialized name must not collide with
        // ANY group's allocation or ANY hostname_override.
        if taken_hostnames(&doc).contains(&hostname) {
            return Err(anyhow!(
                "hostname {hostname:?} is already allocated (global uniqueness, spec D2)"
            ));
        }

        let row = HostnameAllocationRow {
            group_id,
            identity,
            index,
            hostname,
            allocated_at: Some(now_rfc3339()),
            released_at: None,
            rebound_to: None,
        };
        doc.hostname_allocations.push(row.clone());
        write_snapshot(&self.paths, &doc)?;
        Ok(row)
    }

    async fn rebind(
        &self,
        audit: &dyn AuditStore,
        actor: &str,
        group_id: Uuid,
        old_identity: &str,
        new_identity: &str,
    ) -> Result<HostnameAllocationRow> {
        let old_identity = normalize_mac(old_identity);
        let new_identity = normalize_mac(new_identity);
        let mut doc = read_snapshot_strict(&self.paths)?;

        let old_pos = doc
            .hostname_allocations
            .iter()
            .position(|a| {
                a.group_id == group_id && a.identity == old_identity && a.rebound_to.is_none()
            })
            .ok_or_else(|| {
                anyhow!("rebind: old identity {old_identity} is not bound in group {group_id}")
            })?;

        if doc.hostname_allocations.iter().any(|a| {
            a.group_id == group_id && a.identity == new_identity && a.rebound_to.is_none()
        }) {
            return Err(anyhow!(
                "rebind: new identity {new_identity} is already bound in group {group_id}"
            ));
        }

        // Tombstone the old row; the index + hostname move to the new identity.
        doc.hostname_allocations[old_pos].rebound_to = Some(new_identity.clone());
        let index = doc.hostname_allocations[old_pos].index;
        let hostname = doc.hostname_allocations[old_pos].hostname.clone();

        let new_row = HostnameAllocationRow {
            group_id,
            identity: new_identity.clone(),
            index,
            hostname,
            allocated_at: Some(now_rfc3339()),
            released_at: None,
            rebound_to: None,
        };
        doc.hostname_allocations.push(new_row.clone());

        // Commit the snapshot write INSIDE the audit's critical section so the
        // append-only exception and its audit row land atomically (spec D18 /
        // Decision 21). Never the no-op-mutation `append` helper — this changes
        // state, so the mutation IS the snapshot write.
        let event = NewAuditEvent {
            at: now_rfc3339(),
            actor: actor.to_string(),
            role: "operator".to_string(),
            action: "registry.rebind".to_string(),
            target: Some(format!("group:{group_id}")),
            outcome: "success".to_string(),
            detail: Some(serde_json::json!({
                "old_identity": old_identity,
                "new_identity": new_identity,
                "index": index,
            })),
        };
        let paths = self.paths.clone();
        audit
            .append_in_txn(
                Box::new(move || write_snapshot(&paths, &doc).map_err(anyhow::Error::from)),
                event,
            )
            .await?;
        Ok(new_row)
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

    async fn put_group(&self, mut row: HostGroupRow, actor: &str) -> Result<()> {
        let body = crate::profiles::drift::group_body(&row);
        row.content_hash = crate::profiles::drift::content_hash(&body)?.to_vec();

        // Scope the lock so it is dropped BEFORE the capture_version().await
        // below — capture_version -> put_version re-locks the same
        // std::sync::Mutex, and it is not reentrant.
        {
            let mut state = self.state.lock().expect("MemProfileStore state poisoned");
            match state.host_groups.iter_mut().find(|g| g.id == row.id) {
                Some(existing) => *existing = row.clone(),
                None => state.host_groups.push(row.clone()),
            }
        }

        crate::profiles::drift::capture_version(
            self,
            crate::profiles::drift::OBJECT_KIND_HOST_GROUP,
            row.id,
            &body,
            actor,
        )
        .await?;
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

    async fn put_profile(&self, mut row: HostProfileRow, actor: &str) -> Result<()> {
        let body = crate::profiles::drift::profile_body(&row);
        row.content_hash = crate::profiles::drift::content_hash(&body)?.to_vec();

        // See put_group's comment: the lock must be dropped before the
        // capture_version().await below.
        {
            let mut state = self.state.lock().expect("MemProfileStore state poisoned");
            match state.host_profiles.iter_mut().find(|p| p.id == row.id) {
                Some(existing) => *existing = row.clone(),
                None => state.host_profiles.push(row.clone()),
            }
        }

        crate::profiles::drift::capture_version(
            self,
            crate::profiles::drift::OBJECT_KIND_HOST_PROFILE,
            row.id,
            &body,
            actor,
        )
        .await?;
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
        audit: &dyn AuditStore,
        actor: &str,
        group_id: Uuid,
        old_identity: &str,
        new_identity: &str,
    ) -> Result<HostnameAllocationRow> {
        // Phase 1: validate + compute the new row WITHOUT committing (guard
        // dropped before the await below — no std Mutex held across `.await`).
        let (index, hostname) = {
            let state = self.state.lock().expect("MemProfileStore state poisoned");
            let existing = state
                .hostname_allocations
                .iter()
                .find(|a| {
                    a.group_id == group_id && a.identity == old_identity && a.rebound_to.is_none()
                })
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "rebind: old identity {old_identity} is not bound in group {group_id}"
                    )
                })?;
            if state.hostname_allocations.iter().any(|a| {
                a.group_id == group_id && a.identity == new_identity && a.rebound_to.is_none()
            }) {
                return Err(anyhow::anyhow!(
                    "rebind: new identity {new_identity} is already bound in group {group_id}"
                ));
            }
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

        // Phase 2: commit the tombstone + new row INSIDE the audit append — the
        // same atomic pattern the real store uses (never the no-op-mutation
        // helper, which must never accompany a state change).
        let event = NewAuditEvent {
            at: now_rfc3339(),
            actor: actor.to_string(),
            role: "operator".to_string(),
            action: "registry.rebind".to_string(),
            target: Some(format!("group:{group_id}")),
            outcome: "success".to_string(),
            detail: None,
        };
        let row_for_closure = row.clone();
        let new_identity = new_identity.to_string();
        audit
            .append_in_txn(
                Box::new(move || {
                    let mut state = self.state.lock().expect("MemProfileStore state poisoned");
                    if let Some(old) = state.hostname_allocations.iter_mut().find(|a| {
                        a.group_id == group_id
                            && a.identity == old_identity
                            && a.rebound_to.is_none()
                    }) {
                        old.rebound_to = Some(new_identity.clone());
                    }
                    state.hostname_allocations.push(row_for_closure);
                    Ok(())
                }),
                event,
            )
            .await?;
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
        store
            .put_group(sample_group(group_id, "len-serv"), "alice")
            .await
            .unwrap();

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
            .put_group(sample_group(group_id, "bootstrap"), "alice")
            .await
            .expect("first write on a fresh install must succeed");
    }

    #[tokio::test]
    async fn test_delete_group_leaves_allocations() {
        let store = MemProfileStore::new();
        let group_id = Uuid::new_v4();
        store
            .put_group(sample_group(group_id, "len-serv"), "alice")
            .await
            .unwrap();
        store
            .put_profile(
                sample_profile(Uuid::new_v4(), group_id, "aa:bb:cc:dd:ee:ff"),
                "alice",
            )
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
        let audit = MemAuditStore::new();
        let group_id = Uuid::new_v4();
        let first = store.allocate_index(group_id, "old-identity").await.unwrap();

        let rebound = store
            .rebind(&audit, "operator-login", group_id, "old-identity", "new-identity")
            .await
            .unwrap();

        assert_eq!(rebound.index, first.index, "rebind must keep the same index");
        assert_eq!(rebound.identity, "new-identity");

        let all = store.list_allocations(group_id).await.unwrap();
        assert_eq!(all.len(), 2, "old row is tombstoned, not removed");
        let old_row = all.iter().find(|a| a.identity == "old-identity").unwrap();
        assert_eq!(old_row.rebound_to.as_deref(), Some("new-identity"));
    }

    // ── DS-REG-03: allocate-once + rebind on the REAL SnapshotProfileStore ──
    //
    // These exercise the fail-CLOSED production impl (`read_snapshot_strict`),
    // not the in-memory twin, because the whole point of this task is the
    // strict-read behavior a temp-dir snapshot can prove.
    // (`AuditStore`, `read_snapshot_strict`, `write_snapshot` come via `super::*`.)

    use crate::audit::MemAuditStore;

    /// Seed a group into a fresh temp-dir store (via `put_group`, which
    /// bootstraps the snapshot on a missing file) so allocation has a pattern to
    /// render against.
    async fn snapshot_store_with_group(
        dir: &std::path::Path,
        group_id: Uuid,
        name: &str,
    ) -> SnapshotProfileStore {
        let store = SnapshotProfileStore::new(StatePaths::under(dir));
        store.put_group(sample_group(group_id, name), "alice").await.unwrap();
        store
    }

    /// Simulate a decommission: set `released_at` on the row for `identity`.
    fn soft_release(paths: &StatePaths, group_id: Uuid, identity: &str) {
        let mut doc = read_snapshot_strict(paths).unwrap();
        let row = doc
            .hostname_allocations
            .iter_mut()
            .find(|a| a.group_id == group_id && a.identity == identity && a.rebound_to.is_none())
            .expect("identity must be bound to release it");
        row.released_at = Some("2026-01-01T00:00:00Z".into());
        write_snapshot(paths, &doc).unwrap();
    }

    #[tokio::test]
    async fn test_allocate_index_is_idempotent() {
        let dir = tempdir().unwrap();
        let paths = StatePaths::under(dir.path());
        let group_id = Uuid::new_v4();
        let store = snapshot_store_with_group(dir.path(), group_id, "len-serv").await;

        let first = store.allocate_index(group_id, "aa:bb:cc:dd:ee:01").await.unwrap();
        let rows_after_first = read_snapshot_strict(&paths).unwrap().hostname_allocations.len();

        let second = store.allocate_index(group_id, "aa:bb:cc:dd:ee:01").await.unwrap();
        let rows_after_second = read_snapshot_strict(&paths).unwrap().hostname_allocations.len();

        assert_eq!(first.index, second.index, "same identity -> same index");
        assert_eq!(
            rows_after_first, rows_after_second,
            "a second allocate for a bound identity must write NOTHING (no new row)"
        );
        assert_eq!(rows_after_second, 1, "still exactly one allocation row");
    }

    #[tokio::test]
    async fn test_group_delete_does_not_cascade_allocations() {
        // THE core requirement of the whole package: delete + recreate a group
        // (same immutable id) re-attaches every machine to the index it had.
        let dir = tempdir().unwrap();
        let group_id = Uuid::new_v4();
        let store = snapshot_store_with_group(dir.path(), group_id, "len-serv").await;

        let ids = ["aa:bb:cc:dd:ee:01", "aa:bb:cc:dd:ee:02", "aa:bb:cc:dd:ee:03"];
        let mut original = Vec::new();
        for id in ids {
            original.push(store.allocate_index(group_id, id).await.unwrap().index);
        }
        assert_eq!(original, vec![1, 2, 3]);

        store.delete_group("len-serv").await.unwrap();
        // Recreate with the SAME immutable id (allocations key on group_id).
        store
            .put_group(sample_group(group_id, "len-serv"), "alice")
            .await
            .unwrap();

        for (id, expected) in ids.iter().zip(original.iter()) {
            let row = store.allocate_index(group_id, id).await.unwrap();
            assert_eq!(
                row.index, *expected,
                "identity {id} must re-attach to its ORIGINAL index, never a new one"
            );
        }
    }

    #[tokio::test]
    async fn test_allocate_refuses_on_missing_snapshot() {
        // Bare tempdir: NO put_group (that would bootstrap the file). The
        // snapshot genuinely does not exist.
        let dir = tempdir().unwrap();
        let paths = StatePaths::under(dir.path());
        let store = SnapshotProfileStore::new(StatePaths::under(dir.path()));

        let result = store.allocate_index(Uuid::new_v4(), "aa:bb:cc:dd:ee:01").await;
        assert!(
            result.is_err(),
            "a missing snapshot must refuse to allocate, never allocate-from-1"
        );
        assert!(
            !paths.snapshot.exists(),
            "the refusal must not create/clobber the snapshot"
        );
    }

    #[tokio::test]
    async fn test_allocate_refuses_on_corrupt_snapshot() {
        let dir = tempdir().unwrap();
        let paths = StatePaths::under(dir.path());
        std::fs::write(&paths.snapshot, b"not json").unwrap();
        let store = SnapshotProfileStore::new(StatePaths::under(dir.path()));

        let result = store.allocate_index(Uuid::new_v4(), "aa:bb:cc:dd:ee:01").await;
        assert!(
            result.is_err(),
            "a corrupt snapshot must refuse to allocate, never allocate-from-1"
        );
        assert_eq!(
            std::fs::read(&paths.snapshot).unwrap(),
            b"not json",
            "the refusal must not overwrite the corrupt file with an empty registry"
        );
    }

    #[tokio::test]
    async fn test_allocate_never_reuses_released_index() {
        let dir = tempdir().unwrap();
        let paths = StatePaths::under(dir.path());
        let group_id = Uuid::new_v4();
        let store = snapshot_store_with_group(dir.path(), group_id, "len-serv").await;

        store.allocate_index(group_id, "aa:bb:cc:dd:ee:01").await.unwrap();
        store.allocate_index(group_id, "aa:bb:cc:dd:ee:02").await.unwrap();
        soft_release(&paths, group_id, "aa:bb:cc:dd:ee:02");

        let fresh = store.allocate_index(group_id, "aa:bb:cc:dd:ee:03").await.unwrap();
        assert_eq!(
            fresh.index, 3,
            "a NEW identity must get 3, never reuse the released index 2"
        );
    }

    #[tokio::test]
    async fn test_allocate_returning_identity_reactivates_same_index() {
        let dir = tempdir().unwrap();
        let paths = StatePaths::under(dir.path());
        let group_id = Uuid::new_v4();
        let store = snapshot_store_with_group(dir.path(), group_id, "len-serv").await;

        store.allocate_index(group_id, "aa:bb:cc:dd:ee:01").await.unwrap();
        let second = store.allocate_index(group_id, "aa:bb:cc:dd:ee:02").await.unwrap();
        soft_release(&paths, group_id, "aa:bb:cc:dd:ee:02");

        let returned = store.allocate_index(group_id, "aa:bb:cc:dd:ee:02").await.unwrap();
        assert_eq!(returned.index, second.index, "returning machine keeps index 2");

        let doc = read_snapshot_strict(&paths).unwrap();
        let row = doc
            .hostname_allocations
            .iter()
            .find(|a| a.identity == "aa:bb:cc:dd:ee:02" && a.rebound_to.is_none())
            .unwrap();
        assert!(row.released_at.is_none(), "released_at must be cleared on return");
    }

    #[tokio::test]
    async fn test_allocate_lower_mac_added_later_gets_next_index() {
        // The operator's original bug: a machine added LATER with a numerically
        // LOWER MAC must never reshuffle an existing binding.
        let dir = tempdir().unwrap();
        let group_id = Uuid::new_v4();
        let store = snapshot_store_with_group(dir.path(), group_id, "len-serv").await;

        let first = store.allocate_index(group_id, "6c:4b:90:bc:f7:f4").await.unwrap();
        assert_eq!(first.index, 1, "first bound MAC gets index 1");

        let lower = store.allocate_index(group_id, "6c:4b:90:bc:39:b3").await.unwrap();
        assert_eq!(
            lower.index, 2,
            "the lower MAC added later gets the NEXT index, never index 1"
        );
    }

    #[tokio::test]
    async fn test_hostname_uniqueness_is_global() {
        let dir = tempdir().unwrap();
        // Two groups whose patterns render the SAME hostname from different
        // prefixes: `len` + "{name}-serv-{index:03}" and `len-serv` + default.
        let g1 = Uuid::new_v4();
        let g2 = Uuid::new_v4();
        let store = SnapshotProfileStore::new(StatePaths::under(dir.path()));
        let mut group1 = sample_group(g1, "len");
        group1.hostname_pattern = "{name}-serv-{index:03}".into();
        store.put_group(group1, "alice").await.unwrap();
        store
            .put_group(sample_group(g2, "len-serv"), "alice")
            .await
            .unwrap();

        let a = store.allocate_index(g1, "aa:bb:cc:dd:ee:01").await.unwrap();
        assert_eq!(a.hostname, "len-serv-001");

        let collision = store.allocate_index(g2, "aa:bb:cc:dd:ee:02").await;
        assert!(
            collision.is_err(),
            "a second group rendering the same hostname must be refused (global uniqueness)"
        );
    }

    #[tokio::test]
    async fn test_rebind_moves_index_and_tombstones_old() {
        let dir = tempdir().unwrap();
        let paths = StatePaths::under(dir.path());
        let audit = MemAuditStore::new();
        let group_id = Uuid::new_v4();
        let store = snapshot_store_with_group(dir.path(), group_id, "len-serv").await;

        let original = store.allocate_index(group_id, "6c:4b:90:bc:f7:f4").await.unwrap();
        let new_row = store
            .rebind(&audit, "op", group_id, "6c:4b:90:bc:f7:f4", "6c:4b:90:bc:39:b3")
            .await
            .unwrap();

        assert_eq!(new_row.index, original.index, "index moves to the new identity");
        assert_eq!(new_row.hostname, original.hostname, "hostname moves too");
        assert_eq!(new_row.identity, "6c:4b:90:bc:39:b3");

        let doc = read_snapshot_strict(&paths).unwrap();
        let old_row = doc
            .hostname_allocations
            .iter()
            .find(|a| a.identity == "6c:4b:90:bc:f7:f4")
            .unwrap();
        assert_eq!(
            old_row.rebound_to.as_deref(),
            Some("6c:4b:90:bc:39:b3"),
            "old row must be tombstoned with rebound_to"
        );
    }

    #[tokio::test]
    async fn test_rebind_unknown_old_identity_errors() {
        let dir = tempdir().unwrap();
        let audit = MemAuditStore::new();
        let group_id = Uuid::new_v4();
        let store = snapshot_store_with_group(dir.path(), group_id, "len-serv").await;

        let result = store
            .rebind(&audit, "op", group_id, "6c:4b:90:bc:f7:f4", "6c:4b:90:bc:39:b3")
            .await;
        assert!(
            result.is_err(),
            "rebind of an unbound old identity must Err, never silently allocate"
        );
    }

    #[tokio::test]
    async fn test_rebind_to_bound_identity_errors() {
        let dir = tempdir().unwrap();
        let audit = MemAuditStore::new();
        let group_id = Uuid::new_v4();
        let store = snapshot_store_with_group(dir.path(), group_id, "len-serv").await;

        store.allocate_index(group_id, "6c:4b:90:bc:f7:f4").await.unwrap();
        store.allocate_index(group_id, "6c:4b:90:bc:39:b3").await.unwrap();

        let result = store
            .rebind(&audit, "op", group_id, "6c:4b:90:bc:f7:f4", "6c:4b:90:bc:39:b3")
            .await;
        assert!(
            result.is_err(),
            "rebind onto an already-bound identity must Err, never merge two machines"
        );
    }

    #[tokio::test]
    async fn test_rebind_is_audited() {
        let dir = tempdir().unwrap();
        let audit = MemAuditStore::new();
        let group_id = Uuid::new_v4();
        let store = snapshot_store_with_group(dir.path(), group_id, "len-serv").await;

        store.allocate_index(group_id, "6c:4b:90:bc:f7:f4").await.unwrap();
        store
            .rebind(&audit, "operator-jdfalk", group_id, "6c:4b:90:bc:f7:f4", "6c:4b:90:bc:39:b3")
            .await
            .unwrap();

        let events = audit.list_events(0).await.unwrap();
        assert_eq!(events.len(), 1, "rebind must append exactly one audit event");
        assert_eq!(
            events[0].actor, "operator-jdfalk",
            "the audit actor must be the caller-supplied login"
        );
        assert_eq!(events[0].action, "registry.rebind");
    }
}
