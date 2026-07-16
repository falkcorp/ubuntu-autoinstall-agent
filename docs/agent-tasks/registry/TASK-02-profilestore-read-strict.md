<!-- file: docs/agent-tasks/registry/TASK-02-profilestore-read-strict.md -->
<!-- version: 1.0.0 -->
<!-- guid: 94837f18-c89f-407d-ace7-e951a019b18a -->
<!-- last-edited: 2026-07-16 -->

# TASK-02 — `ProfileStore` trait + `SnapshotProfileStore` + `read_snapshot_strict` (DS-REG-02)

**Priority:** P1 · **Effort:** M · **Recommended subagent:** Sonnet-class · rust-control subagent · **Why:** introduces the fail-closed read that DS-REG-03's allocation depends on; the trait shape binds three sibling tasks. · **Depends on:** DS-REG-01 (needs the row types + `SnapshotDoc` collections + `profiles/` stubs)

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/registry-profilestore-read-strict" -b agent/registry-profilestore-read-strict origin/main
cd "$REPO/.worktrees/registry-profilestore-read-strict"
git rebase origin/main
```

(Protocol also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

**Wave gate:** DS-REG-01 must be merged. If `grep -n "pub struct HostnameAllocationRow" crates/uaa-control/src/db/mod.rs` returns 0 hits, the gate is not met: STOP and report.

## Goal

Fill the DS-REG-01 stub `crates/uaa-control/src/profiles/store.rs` with a `ProfileStore` trait plus two impls, and add **`read_snapshot_strict`** to `crates/uaa-control/src/db/store.rs`.

**`read_snapshot_strict` is the whole point of this task.** Everything else is routine.

> ## ⚠ The single most important thing in this package
>
> `read_snapshot` **fails OPEN**. On a missing *or corrupt* snapshot it logs `"serving EMPTY registry (degraded)"` and returns `SnapshotDoc::default()` — verify: `grep -n "serving EMPTY registry" crates/uaa-control/src/db/store.rs` (2 hits).
>
> That is correct for telemetry ingest. It is **catastrophic for allocation**: an allocator reading an empty view sees zero hostname bindings, concludes every index is free, and **re-allocates every index from 1 — renaming the entire fleet.**
>
> So allocation must NOT read through it. This task adds `read_snapshot_strict(paths) -> Result<SnapshotDoc, StoreError>` which returns `Err` on missing-or-corrupt, and DS-REG-03 uses it. **Never** `unwrap_or_default()` a snapshot read on an allocation path.

REUSE — do not invent parallels:

- **Mirror `SagaStore`'s trait+twins shape** — verify: `grep -n "pub trait SagaStore: Send + Sync" crates/uaa-control/src/saga.rs`. A `#[async_trait]` trait + a real impl + an in-memory twin. This is the crate's precedent for a per-concern store, and the reason we are NOT growing `RegistryStore` (spec D5).
- **`read_snapshot` / `write_snapshot` / `guarded_mutation` / `StatePaths`** — verify: `grep -n "pub fn write_snapshot\|pub fn read_snapshot\|pub async fn guarded_mutation" crates/uaa-control/src/db/store.rs` (3 hits). Reuse them; do NOT hand-roll file IO. `read_snapshot_strict` is a **sibling** of `read_snapshot`, not a replacement — the fail-open one stays for telemetry.
- **`StoreError`** is the existing error type in `db/store.rs`. Reuse it; do NOT add a new error enum.
- Row types come from `db/mod.rs` (DS-REG-01) — import, never redefine.

## Background (verify before editing)

- **`MemProfileStore` MUST be `#[cfg(test)]`-gated.** This deliberately breaks from `MemRegistryStore`/`MemAuditStore`, which are always-compiled. Reason: `default_state()` builds `MemEnrollmentStore`/`MemAuditStore` in **production**, and the crate's convention is "degrade rather than fail to start". If `MemProfileStore` is reachable from production wiring, someone will eventually write `SnapshotProfileStore::new(..).unwrap_or_else(|_| MemProfileStore::new())` — and an empty profile store means allocation re-allocates from 1 and the fleet renames itself. `#[cfg(test)]` makes that wiring **fail to compile**. This is a deliberate, documented divergence — say so in the module doc.
- **This task adds NO allocation logic** (DS-REG-03) and **no hashing** (DS-REG-04). `allocate_index`/`rebind` are trait methods here with `unimplemented!("DS-REG-03")` bodies in `SnapshotProfileStore` — a loud stub, never `Ok(())`.
- Edge semantics (spelled out here AND in acceptance):
  - **Missing snapshot file** → `read_snapshot_strict` = `Err(StoreError::…)` naming the path. `read_snapshot` keeps returning empty (unchanged — do not touch it).
  - **Corrupt snapshot** → same: `Err`, naming the path and the parse error.
  - **Valid snapshot with no profile collections** → `Ok(doc)` with empty vecs. This is NOT an error — it is a fresh install, and DS-REG-01 made the collections `#[serde(default)]` exactly so this parses.
  - The distinction that matters: **"the file says there are no allocations" (Ok, empty) vs "I cannot read the file" (Err)**. Conflating them is the bug.

**HARD RULES (non-negotiable):**
- **NO SQL, NO migration** anywhere in this package — `uaa-control` has no DB connection in production (spec D4). If you write `CREATE TABLE` or `SQL_*`, STOP.
- NO hardware actions. NEVER wipe/write/deploy on 172.16.2.30 or len-serv-003. NEVER power on unimatrixone.
- Do NOT edit `db/registry.rs` — growing `RegistryStore` breaks `saga.rs`'s `RecordingRegistry`.
- Do NOT change `read_snapshot`'s existing fail-open behavior — telemetry depends on it.
- Stay inside your worktree; never `git push`, `gh pr`, or merge — report done and stop.

- **Re-verify these anchors before editing** — line numbers drift; zero hits = STOP and report:
  ```bash
  grep -n "serving EMPTY registry" crates/uaa-control/src/db/store.rs
  # expect: 2 hits (~lines 227, 237) — THE fail-open behavior read_snapshot_strict exists to avoid
  grep -n "pub fn read_snapshot" crates/uaa-control/src/db/store.rs
  # expect: 1 hit (~line 221) — add read_snapshot_strict next to it
  grep -n "pub async fn guarded_mutation" crates/uaa-control/src/db/store.rs
  # expect: 1 hit (~line 327) — the serialized-mutation helper
  grep -n "pub trait SagaStore: Send + Sync" crates/uaa-control/src/saga.rs
  # expect: 1 hit (~line 241) — the trait+twins shape to mirror
  grep -n "pub struct HostnameAllocationRow" crates/uaa-control/src/db/mod.rs
  # expect: 1 hit — DS-REG-01 merged (0 hits = wave gate not met, STOP)
  ```

## Step-by-step

1. In `crates/uaa-control/src/db/store.rs`, add next to `read_snapshot`:
   ```rust
   /// Fail-CLOSED snapshot read. Unlike `read_snapshot` — which serves an EMPTY
   /// doc on a missing/corrupt file, correct for telemetry — this returns Err.
   /// Allocation MUST use this: an allocator that reads an empty view concludes
   /// every index is free and re-allocates the whole fleet from 1.
   pub fn read_snapshot_strict(paths: &StatePaths) -> Result<SnapshotDoc, StoreError> { ... }
   ```
   Do **not** modify `read_snapshot`.
2. Fill `crates/uaa-control/src/profiles/store.rs` (keep DS-REG-01's guid; bump version):
   ```rust
   #[async_trait::async_trait]
   pub trait ProfileStore: Send + Sync {
       async fn list_groups(&self) -> Result<Vec<HostGroupRow>>;
       async fn get_group(&self, name: &str) -> Result<Option<HostGroupRow>>;
       async fn put_group(&self, row: HostGroupRow) -> Result<()>;
       async fn delete_group(&self, name: &str) -> Result<()>;
       async fn list_profiles(&self, group_id: Uuid) -> Result<Vec<HostProfileRow>>;
       async fn put_profile(&self, row: HostProfileRow) -> Result<()>;
       async fn list_allocations(&self, group_id: Uuid) -> Result<Vec<HostnameAllocationRow>>;
       async fn allocate_index(&self, group_id: Uuid, identity: &str) -> Result<HostnameAllocationRow>;
       async fn rebind(&self, group_id: Uuid, old_identity: &str, new_identity: &str) -> Result<HostnameAllocationRow>;
       async fn list_versions(&self, object_id: Uuid) -> Result<Vec<ProfileVersionRow>>;
       async fn put_version(&self, row: ProfileVersionRow) -> Result<()>;
   }
   ```
3. Implement `SnapshotProfileStore { paths: StatePaths }` — reads via `read_snapshot_strict`, writes via `guarded_mutation` + `write_snapshot`. `allocate_index` and `rebind` bodies are `unimplemented!("DS-REG-03")` — a loud stub, **never** a silent `Ok`.
4. Implement `MemProfileStore` **behind `#[cfg(test)]`**, backed by `Mutex<...>`, with all methods working (tests need it).
5. **`delete_group` must NOT touch `hostname_allocations`.** It removes the group row and cascades to `host_profiles` only. That asymmetry is the core mechanism — allocations outliving groups is what makes delete-and-recreate re-attach machines to their existing indices (spec D8). Comment it inline; a future reader will otherwise "fix" it.
6. Keep purely additive — do not modify `read_snapshot`, `MachineRow`, `RegistryStore`, or `registry.rs`.
7. Add tests in `store.rs`'s `mod tests` **and** `db/store.rs`'s:
   - `test_read_snapshot_strict_errors_on_missing` — a `StatePaths` pointing at a nonexistent file ⇒ `Err`, and the error names the path.
   - `test_read_snapshot_strict_errors_on_corrupt` — a file containing `not json` ⇒ `Err`.
   - `test_read_snapshot_strict_ok_on_valid_empty` — a valid snapshot with no profile collections ⇒ `Ok`, empty vecs. **This is the "no allocations" vs "cannot read" distinction; without it the fail-closed read would reject fresh installs.**
   - `test_read_snapshot_still_fails_open` — `read_snapshot` on the same missing path still returns an empty doc (proves telemetry's fail-open path is untouched).
   - `test_delete_group_leaves_allocations` — put a group + an allocation, delete the group, allocations survive.
   - `test_allocate_index_is_unimplemented_stub` — calling it panics/errors mentioning `DS-REG-03` (replaced by DS-REG-03).
8. Bump headers on every file you touch; keep existing guids.

**Anti-over-suppression:** `read_snapshot_strict` is a guard, so it can over-block. `test_read_snapshot_strict_ok_on_valid_empty` is the happy-path proof that a legitimately-empty fresh install still resolves `Ok` — without it, a too-strict read would reject every new deployment. `test_read_snapshot_still_fails_open` additionally proves the new guard did not over-reach into the telemetry path.

## How to test

```bash
cargo test --lib --offline
# Expected: 634+ passed, 0 failed (634 baseline + DS-REG-01's 3 + your 6).
cargo build --offline
# Expected: exit 0.
cargo clippy --offline -- -D warnings
# Expected: no warnings.
```

## Acceptance criteria

- [ ] `cargo test --lib --offline` exits 0 — verify: `cargo test --lib --offline 2>&1 | grep -E "^test result"`
- [ ] `cargo build --offline` exits 0 — verify: `cargo build --offline && echo BUILD_OK`
- [ ] `read_snapshot_strict` exists and returns a Result — verify: `grep -c "pub fn read_snapshot_strict" crates/uaa-control/src/db/store.rs` returns 1
- [ ] `read_snapshot`'s fail-open behavior is untouched — verify: `cargo test --lib --offline test_read_snapshot_still_fails_open`
- [ ] **`MemProfileStore` is test-gated** — verify: `grep -B2 "pub struct MemProfileStore" crates/uaa-control/src/profiles/store.rs | grep -c "cfg(test)"` returns 1
- [ ] Allocation is a loud stub, not a silent Ok — verify: `grep -c 'unimplemented!("DS-REG-03")' crates/uaa-control/src/profiles/store.rs` returns 2
- [ ] Delete does not cascade allocations — verify: `cargo test --lib --offline test_delete_group_leaves_allocations`
- [ ] Fresh install still resolves — verify: `cargo test --lib --offline test_read_snapshot_strict_ok_on_valid_empty`
- [ ] No SQL, no migration — verify: `git diff origin/main | grep -c "CREATE TABLE\|SQL_"` returns **0**
- [ ] `RegistryStore` untouched — verify: `git diff origin/main --name-only | grep -c "db/registry.rs"` returns **0**
- [ ] `cargo clippy --offline -- -D warnings` clean
- [ ] File headers bumped — verify: `git diff origin/main --name-only | xargs -I{} grep -l "last-edited: 2026-07" {}`

## Commit message

```
feat(control): add ProfileStore, SnapshotProfileStore, read_snapshot_strict (DS-REG-02)

Adds a sibling ProfileStore trait + snapshot-backed impl, following saga.rs's
SagaStore precedent rather than growing RegistryStore (whose RecordingRegistry
in saga.rs hand-forwards every method).

The load-bearing part is read_snapshot_strict. read_snapshot fails OPEN — on a
missing or corrupt file it logs "serving EMPTY registry (degraded)" and returns
an empty doc. That is right for telemetry and catastrophic for allocation: an
allocator reading an empty view concludes every index is free and re-allocates
the whole fleet from 1. read_snapshot_strict returns Err instead, and DS-REG-03
uses it. read_snapshot itself is unchanged.

MemProfileStore is #[cfg(test)]-gated — deliberately unlike its always-compiled
neighbours — so a production fallback to an empty profile store cannot compile.

delete_group deliberately does NOT touch hostname_allocations: allocations
outliving groups is what makes delete-and-recreate re-attach machines to their
existing indices.

allocate_index/rebind are loud unimplemented!() stubs (DS-REG-03).

Co-Authored-By: Claude <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

**Polarity: additive.** If `grep -n "pub fn read_snapshot_strict" crates/uaa-control/src/db/store.rs` hits AND `grep -n "pub trait ProfileStore" crates/uaa-control/src/profiles/store.rs` hits, this task is already applied — run the acceptance checks instead of re-applying. Rollback = revert the single commit; `read_snapshot`'s fail-open path and every existing consumer are unaffected, and nothing calls `ProfileStore` yet. DS-REG-03 fills the allocation stubs in this same file and must rebase after this merges — see the collision table in `../BREAKDOWN-2026-07-16.md`.
