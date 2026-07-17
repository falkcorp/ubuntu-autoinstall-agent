<!-- file: docs/agent-tasks/registry/README.md -->
<!-- version: 1.0.0 -->
<!-- guid: 447b2246-3247-4b4f-aa17-3feb89a41c65 -->
<!-- last-edited: 2026-07-16 -->

# Workstream — registry (profile store, allocate-once indices, drift)

The package's safety core. Persists host groups/profiles in the existing `StatePaths` snapshot, allocates hostname indices **once** per machine identity so deleting and recreating a group never renames a machine, and detects out-of-band edits. From spec C3/C5, D4–D12, D18.

Design: [`deploy-system-design.md`](../../specs/deploy-system-design.md) · Plan + authoritative wave/tier table: [`deploy-system-plan.md`](../../specs/deploy-system-plan.md) · Bucket sort: [`BREAKDOWN-2026-07-16.md`](../BREAKDOWN-2026-07-16.md)

| Task | Src id | Title | Priority | Effort | Tier | Wave |
|------|--------|-------|----------|--------|------|------|
| TASK-01 | DS-REG-01 | Snapshot row types + `SnapshotDoc` collections + `profiles/` scaffold | P1 | M | Sonnet-class | 1 |
| TASK-02 | DS-REG-02 | `ProfileStore` + `SnapshotProfileStore` + **`read_snapshot_strict`** | P1 | M | Sonnet-class | 2 |
| TASK-03 ⚠ | DS-REG-03 | `allocate_index` (fail-closed) + `rebind` | **P0** | L | **Opus-class** | 3 |
| TASK-04 | DS-REG-04 | `content_hash` (explicit canonicalization) + `profile_versions` | P1 | M | Sonnet-class | 4 |
| TASK-05 ⚠ | DS-REG-05 | Drift scan + accept/revert (last-good-version) | P1 | L | **Opus-class** | 5 |

**Waves are GLOBAL across the deploy-system package** (not per-workstream) — see [`../../specs/deploy-system-plan.md`](../../specs/deploy-system-plan.md) § Parallel execution groups. A task does not start until every task it depends on has MERGED.

## Ground rules

- Rust only (except the SPA workstream), in exactly the files each brief names. Purely additive — no existing signature changes.
- Build + test gate for every task in this workstream:
  ```bash
  cargo test --lib --offline && cargo build --offline
  # Expected: >=634 passed (planning baseline), build exit 0
  cargo clippy --offline -- -D warnings
  # Expected: no warnings
  ```
- **Verify every file:line anchor with `grep` before editing** — line numbers in each brief are a starting point, not a guarantee. Zero hits = STOP and report.
- File headers MANDATORY: new files get a fresh 4-line header with a new uuid4; edited files keep their guid and bump `version` + `last-edited`.
- **NO SQL, NO migration anywhere in this package.** `uaa-control` has no database connection in production (`default_state()` builds `FileRegistry` + `Mem*Store`; `db::migrations::apply` has no caller). Profiles persist in the `StatePaths` snapshot — spec [D4](../../specs/deploy-system-design.md).
- HARD RULES (restated in every brief): NO hardware actions — all commands go through `CommandExecutor` mocks; NEVER wipe/write/deploy on 172.16.2.30 or len-serv-003; NEVER power on unimatrixone; `disk_device` read from the live target, never guessed; no real secret anywhere (`REPLACE_AT_PLACE_TIME` stays a placeholder); workers stay in their worktree and never push/PR/merge.

## Collision / wave note

**TASK-01 and TASK-02 both edit `db/store.rs`**; **TASK-01 and TASK-04 both edit `db/mod.rs`**; **TASK-02 and TASK-03 both edit `profiles/store.rs`**; **TASK-04 and TASK-05 both edit `profiles/drift.rs`**. Each pair MUST run in different waves — the collision table in [`BREAKDOWN-2026-07-16.md`](../BREAKDOWN-2026-07-16.md) is authoritative.

> **⚠ TASK-03 and TASK-05 are Opus-class and review-critical. Never downgrade them.**
>
> **TASK-03** owns the design's core safety property. `read_snapshot` **fails OPEN** — on a missing *or corrupt* snapshot it logs `"serving EMPTY registry (degraded)"` and returns an empty doc. An allocator reading through it sees zero bindings, concludes every index is free, and **re-allocates every index from 1 — renaming the entire fleet**. Every allocation read must use `read_snapshot_strict` (TASK-02). There is no safe fallback.
>
> **TASK-05** owns revert semantics. Revert restores **the newest version whose body still hashes to its own stored hash**, never a blind `N−1` — a blind N−1 silently discards the last legitimate change along with the drift.

See [ORCHESTRATION.md](../ORCHESTRATION.md) (one level up) for the coordinator + worker protocol.
