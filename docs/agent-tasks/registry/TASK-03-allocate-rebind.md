<!-- file: docs/agent-tasks/registry/TASK-03-allocate-rebind.md -->
<!-- version: 1.0.0 -->
<!-- guid: 9f3a4cf4-4f4d-47c3-8e48-079f765c688c -->
<!-- last-edited: 2026-07-16 -->

# TASK-03 — `allocate_index` (fail-closed, allocate-once) + `rebind` ⚠ review-critical (DS-REG-03)

**Priority:** P0 · **Effort:** L · **Recommended subagent:** **Opus-class** · rust-control subagent · **Why:** irreversible — a wrong read here re-allocates every index from 1 and renames the entire fleet. Never downgrade this tier. · **Depends on:** DS-REG-02 (needs `ProfileStore` + `read_snapshot_strict`)

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/registry-allocate-rebind" -b agent/registry-allocate-rebind origin/main
cd "$REPO/.worktrees/registry-allocate-rebind"
git rebase origin/main
```

(Protocol also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

**Wave gate:** DS-REG-02 must be merged. If `grep -n "pub fn read_snapshot_strict" crates/uaa-control/src/db/store.rs` returns 0 hits, the gate is not met: STOP and report.

## Goal

Fill `crates/uaa-control/src/profiles/alloc.rs` and replace the two `unimplemented!("DS-REG-03")` stubs in `profiles/store.rs` with real `allocate_index` and `rebind`.

**This task exists to guarantee one invariant:** *a machine's hostname index is assigned once, bound to its identity, and never re-derived* — so deleting and recreating a `HostGroupProfile` never renames `len-serv-001` to `len-serv-004`, and adding a machine with a lower MAC never reshuffles anyone.

> ## ⚠ The failure this task must not have
>
> `read_snapshot` **fails OPEN** — on a missing *or corrupt* file it logs `"serving EMPTY registry (degraded)"` and returns an **empty** `SnapshotDoc`. An allocator reading through it sees zero bindings, concludes every index is free, and **re-allocates every index from 1 — renaming the entire fleet.**
>
> **Every read in this task goes through `read_snapshot_strict` (DS-REG-02), which returns `Err`.** Never `read_snapshot`, never `unwrap_or_default()`, never `.unwrap_or_else(|_| SnapshotDoc::default())` on an allocation path. If you cannot read the snapshot, you **refuse to allocate**. There is no safe fallback.

REUSE — do not invent parallels:

- **`read_snapshot_strict`** (DS-REG-02) — verify: `grep -n "pub fn read_snapshot_strict" crates/uaa-control/src/db/store.rs`. The ONLY read this task may use.
- **`guarded_mutation` + `write_snapshot`** — verify: `grep -n "pub async fn guarded_mutation\|pub fn write_snapshot" crates/uaa-control/src/db/store.rs`. `guarded_mutation` serializes writers; run the whole read-compute-write inside it so two concurrent allocators cannot both compute the same next index.
- **`AuditStore::append_in_txn`** for `rebind` — verify: `grep -n "async fn append_in_txn" crates/uaa-control/src/audit.rs`. **NOT `record()`** — `record()` passes a no-op mutation and must never be used for something that also changes state; `append_in_txn` commits the mutation and its audit row atomically.
- **`normalize_mac`** — verify: `grep -n "pub fn normalize_mac" crates/uaa-control/src/import_export.rs`. Every identity is normalized before use as a key. Do NOT hand-roll MAC normalization.
- Row types from `db/mod.rs` — import, never redefine.

## Background (verify before editing)

- **Identity is the MAC** (spec D-A / option A1), `normalize_mac`'d. Allocation is keyed `(group_id, identity)` — on the group's **immutable `Uuid`**, never its name. Keying on the mutable name would orphan every allocation on a rename and restart allocation at 1 (spec D2).
- **Allocate-once semantics** (all four spelled out here AND in acceptance):
  - **Identity already bound** → return the existing row **unchanged**; write nothing. This is what makes allocation idempotent and re-derivation impossible. A retried or crashed allocate is safe.
  - **Identity not bound** → `index = max(existing indices for this group_id) + 1`, starting at 1. Never "lowest free" — never reuse.
  - **Identity bound but `released_at.is_some()`** → **clear `released_at` and return the SAME index**. The machine comes back under its original name; that is the entire point of allocate-once. (v1 of the spec had this incoherent; it is now locked — see spec § Soft-release semantics.)
  - **Snapshot unreadable** → `Err`. **Never** allocate.
- **`rebind(group_id, old_identity, new_identity)`** is the NIC-replacement runbook, and the **one deliberate exception to append-only** (spec D18). Without it, a dead NIC means: new MAC → new identity → next free index → the machine returns as `len-serv-004`, and index 002 is burned **forever** because indices are never reused. `rebind` moves the existing index+hostname to the new identity and tombstones the old row (`rebound_to = new_identity`). It is audited via `append_in_txn` and gated at `Role::Operator` by its caller (DS-OPS-01).
  - **`old_identity` not bound** → `Err` naming it. Never silently allocate instead.
  - **`new_identity` already bound in this group** → `Err`. Never merge two machines onto one index.
- **Hostname uniqueness is global**, not per-group (spec D2): `hostname_pattern` is free-form, so group `len` with pattern `{name}-serv-{index:03}` and group `len-serv` with the default both render `len-serv-001`. Before writing an allocation, check the materialized hostname against **every** group's allocations and every `hostname_override`.
- **`index` is a plain Rust `i64` field.** (It is a reserved keyword in CockroachDB — irrelevant here; there is no SQL anywhere in this package.)

**HARD RULES (non-negotiable):**
- **NO SQL, NO migration** — `uaa-control` has no DB connection in production (spec D4).
- NO hardware actions. NEVER wipe/write/deploy on 172.16.2.30 or len-serv-003. NEVER power on unimatrixone.
- Do NOT edit `db/registry.rs`; do NOT change `read_snapshot`'s fail-open behavior (telemetry needs it).
- Stay inside your worktree; never `git push`, `gh pr`, or merge — report done and stop.

- **Re-verify these anchors before editing** — line numbers drift; zero hits = STOP and report:
  ```bash
  grep -n "pub fn read_snapshot_strict" crates/uaa-control/src/db/store.rs
  # expect: 1 hit — DS-REG-02 merged (0 hits = wave gate not met, STOP). The ONLY read you may use.
  grep -n "serving EMPTY registry" crates/uaa-control/src/db/store.rs
  # expect: 2 hits — read_snapshot's fail-open behavior. Read these to understand what you must avoid.
  grep -n "pub async fn guarded_mutation" crates/uaa-control/src/db/store.rs
  # expect: 1 hit — wrap the whole read-compute-write in this
  grep -n 'unimplemented!("DS-REG-03")' crates/uaa-control/src/profiles/store.rs
  # expect: 2 hits — the stubs you replace
  grep -n "async fn append_in_txn" crates/uaa-control/src/audit.rs
  # expect: 1 hit — rebind's audit path (NOT record())
  grep -n "pub fn normalize_mac" crates/uaa-control/src/import_export.rs
  # expect: 1 hit — identity normalization
  ```

## Step-by-step

1. Fill `crates/uaa-control/src/profiles/alloc.rs` (keep DS-REG-01's guid; bump version) with the pure, testable half:
   ```rust
   /// Next index for a group: max(existing) + 1, starting at 1. Never "lowest
   /// free" — indices are never reused, so a released index is still counted.
   pub fn next_index(existing: &[HostnameAllocationRow]) -> i64;

   /// Render a hostname from a pattern + index. "{name}-{index:03}" -> "len-serv-001".
   pub fn render_hostname(pattern: &str, name: &str, index: i64) -> Result<String>;

   /// Every materialized hostname across ALL groups, plus every hostname_override
   /// — the global-uniqueness input (spec D2).
   pub fn taken_hostnames(doc: &SnapshotDoc) -> std::collections::BTreeSet<String>;
   ```
   These are pure functions over slices — unit-testable with no store, no file, no mock.
2. In `profiles/store.rs`, replace the two stubs. `allocate_index`:
   - `let identity = normalize_mac(identity);`
   - inside `guarded_mutation`: `let doc = read_snapshot_strict(&self.paths)?;` — **strict, always**
   - if a row for `(group_id, identity)` exists: if `released_at.is_some()`, clear it and write; **return that row's existing index either way**, never a new one
   - else compute `next_index`, `render_hostname`, check against `taken_hostnames` (global), push the row, `write_snapshot`
3. Implement `rebind` in `store.rs`: inside `guarded_mutation`, strict read; `Err` if `old_identity` unbound or `new_identity` already bound; move `index`+`hostname` to a new row for `new_identity`; set the old row's `rebound_to`; audit via `append_in_txn` with the caller-supplied actor.
4. Keep purely additive — do not modify `read_snapshot`, `guarded_mutation`, `MachineRow`, or `RegistryStore`.
5. Add tests in `alloc.rs` (pure) and `store.rs` (via `MemProfileStore` + a temp-dir `SnapshotProfileStore`):
   - `test_allocate_index_is_idempotent` — second allocate for a bound identity returns the same index and **writes nothing** (assert the snapshot's mtime or row count is unchanged, not merely that the index matches).
   - **`test_group_delete_does_not_cascade_allocations`** — allocate 001/002/003, delete the group, recreate it, re-allocate the same three identities ⇒ **the same three indices**, in the same identity→index mapping. *This is the core requirement of the entire package.*
   - **`test_allocate_refuses_on_missing_snapshot`** — point `StatePaths` at a nonexistent file ⇒ `Err`, and **no allocation is written**. Not an allocate-from-1.
   - **`test_allocate_refuses_on_corrupt_snapshot`** — same for a file containing `not json`.
   - `test_allocate_never_reuses_released_index` — allocate 001/002, soft-release 002, allocate a **new** identity ⇒ gets **003**, never 002.
   - `test_allocate_returning_identity_reactivates_same_index` — soft-release 002, re-allocate the **same** identity ⇒ `released_at` cleared, index still **002**.
   - `test_allocate_lower_mac_added_later_gets_next_index` — bind aa:…:03, then aa:…:01 ⇒ the later machine gets index 2, **not** 1. (The user's original bug: no sort-order re-derivation.)
   - `test_hostname_uniqueness_is_global` — two groups whose patterns render the same hostname ⇒ the second allocate is `Err`.
   - `test_rebind_moves_index_and_tombstones_old` — rebind old→new ⇒ new identity holds the old index+hostname; old row has `rebound_to == new_identity`.
   - `test_rebind_unknown_old_identity_errors` / `test_rebind_to_bound_identity_errors`.
   - `test_rebind_is_audited` — the audit store recorded an event whose actor is the caller-supplied login (use `MemAuditStore`).
6. Bump headers on every file you touch; keep existing guids.

**Anti-over-suppression:** the fail-closed read is a guard and can over-block. `test_allocate_index_is_idempotent` and `test_allocate_returning_identity_reactivates_same_index` are the happy-path proofs that a **readable** snapshot still allocates and still re-attaches a returning machine — without them a too-strict read would refuse every legitimate allocation, which fails safe but ships a system that cannot deploy anything.

## How to test

```bash
cargo test --lib --offline
# Expected: 634+ passed, 0 failed (baseline + DS-REG-01/02's tests + your 11).
cargo build --offline
# Expected: exit 0.
cargo clippy --offline -- -D warnings
# Expected: no warnings.
```

## Acceptance criteria

- [ ] `cargo test --lib --offline` exits 0 — verify: `cargo test --lib --offline 2>&1 | grep -E "^test result"`
- [ ] `cargo build --offline` exits 0 — verify: `cargo build --offline && echo BUILD_OK`
- [ ] **No fail-open read on any allocation path** — verify: `grep -c "read_snapshot(" crates/uaa-control/src/profiles/store.rs crates/uaa-control/src/profiles/alloc.rs` returns **0** for both files (only `read_snapshot_strict` may appear)
- [ ] **No default-on-error escape hatch** — verify: `grep -c "unwrap_or_default\|SnapshotDoc::default()" crates/uaa-control/src/profiles/alloc.rs crates/uaa-control/src/profiles/store.rs` returns **0** for both
- [ ] **The core requirement** — verify: `cargo test --lib --offline test_group_delete_does_not_cascade_allocations`
- [ ] **Fail-closed on unreadable state** — verify: `cargo test --lib --offline test_allocate_refuses_on_missing_snapshot test_allocate_refuses_on_corrupt_snapshot`
- [ ] No sort-order re-derivation — verify: `cargo test --lib --offline test_allocate_lower_mac_added_later_gets_next_index`
- [ ] Anti-over-suppression: a readable snapshot still allocates — verify: `cargo test --lib --offline test_allocate_index_is_idempotent test_allocate_returning_identity_reactivates_same_index`
- [ ] `rebind` is audited via `append_in_txn`, not `record` — verify: `grep -c "record(" crates/uaa-control/src/profiles/store.rs` returns **0**, and `cargo test --lib --offline test_rebind_is_audited` passes
- [ ] Stubs are gone — verify: `grep -c 'unimplemented!("DS-REG-03")' crates/uaa-control/src/profiles/store.rs` returns **0**
- [ ] No SQL, no migration — verify: `git diff origin/main | grep -c "CREATE TABLE\|SQL_"` returns **0**
- [ ] `cargo clippy --offline -- -D warnings` clean
- [ ] File headers bumped — verify: `git diff origin/main --name-only | xargs -I{} grep -l "last-edited: 2026-07" {}`

## Coordinator review checklist (⚠ review-critical — line-by-line before merge)

- [ ] Every snapshot read on an allocation path is `read_snapshot_strict`. No `read_snapshot`, no `unwrap_or_default`, no `SnapshotDoc::default()` fallback.
- [ ] The whole read-compute-write sits inside `guarded_mutation` — not just the write.
- [ ] A bound identity returns its existing index and writes nothing.
- [ ] `next_index` is `max + 1`, never "lowest free".
- [ ] `delete_group` still leaves `hostname_allocations` untouched (DS-REG-02's behavior is not regressed).
- [ ] `rebind` uses `append_in_txn` with a real actor, never `record()` or a placeholder string.

## Commit message

```
feat(control): allocate-once hostname indices + rebind (DS-REG-03)

Implements allocate_index and rebind on SnapshotProfileStore, guaranteeing
that a machine's index is assigned once, bound to its MAC, and never
re-derived: deleting and recreating a group re-attaches every machine to the
index it already had, and a machine with a lower MAC added later gets the
next free index rather than reshuffling anyone.

Every read goes through read_snapshot_strict. read_snapshot fails OPEN — an
allocator reading its empty doc would conclude every index is free and
re-allocate the whole fleet from 1. There is no safe fallback: an unreadable
snapshot means refuse to allocate.

rebind is the one deliberate exception to append-only (audited via
append_in_txn): a replaced NIC means a new MAC, and without rebind the
machine returns as len-serv-004 while index 002 is burned forever.

Hostname uniqueness is checked globally, not per-group: hostname_pattern is
free-form, so two groups can render the same hostname from different prefixes.

Co-Authored-By: Claude <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge. This task is ⚠ review-critical: expect line-by-line review against the checklist above.

## Idempotency / Rollback

**Polarity: transform** (replaces `unimplemented!` stubs with real bodies). If `grep -c 'unimplemented!("DS-REG-03")' crates/uaa-control/src/profiles/store.rs` returns **0** AND `grep -n "pub fn next_index" crates/uaa-control/src/profiles/alloc.rs` hits, this task is already applied — run the acceptance checks instead of re-applying. Rollback = revert the single commit, restoring the loud stubs; **no allocation data can exist yet** because nothing calls these methods until DS-OPS-01/03, so reverting strands nothing. Once allocations DO exist in a production snapshot, this table is roll-forward-only — a bad binding is corrected with `rebind`, never by deleting rows (spec § Rollback).
