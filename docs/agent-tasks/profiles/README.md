<!-- file: docs/agent-tasks/profiles/README.md -->
<!-- version: 1.0.0 -->
<!-- guid: e8fa99ae-a2c0-45f7-b8de-cd3c426ce074 -->
<!-- last-edited: 2026-07-16 -->

# Workstream — profiles (pure schema, merge, validation)

The pure half: partial types, the group∪host merge engine with per-field provenance, and fail-closed validation. No I/O, no store, no async — every task here is unit-testable in isolation. From spec C1/C2, D1–D3.

Design: [`deploy-system-design.md`](../../specs/deploy-system-design.md) · Plan + authoritative wave/tier table: [`deploy-system-plan.md`](../../specs/deploy-system-plan.md) · Bucket sort: [`BREAKDOWN-2026-07-16.md`](../BREAKDOWN-2026-07-16.md)

| Task | Src id | Title | Priority | Effort | Tier | Wave |
|------|--------|-------|----------|--------|------|------|
| TASK-01 | DS-PRF-01 | `profile/` scaffold: partial types + `merge.rs`/`validate.rs` stubs | P1 | M | Sonnet-class | 2 |
| TASK-02 | DS-PRF-02 | `merge()` + provenance + 10-required-field fail-closed | P1 | L | Sonnet-class | 3 |
| TASK-03 | DS-PRF-03 | Validation: global hostname uniqueness, immutability, standalone | P1 | M | Sonnet-class | 3 |

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

**Scaffold-first resolves the only collision here.** All three tasks would otherwise collide on `profile/mod.rs`. TASK-01 (wave 2) creates `mod.rs` **with every `pub mod` declaration plus empty stub files**; TASK-02 and TASK-03 then fill **disjoint** files (`merge.rs`, `validate.rs`) in wave 3 and **must not re-edit `mod.rs`**. Because their files are disjoint, TASK-02 and TASK-03 run **concurrently**.

> **⚠ Two traps, both silent:**
>
> **TASK-01's `Option<Option<String>>`.** A plain `Option` cannot distinguish "this host doesn't override the PIN" from "this host explicitly has NO PIN" — collapse it and a host meant to have no TPM PIN **silently inherits the group's**.
>
> **TASK-02's fail-closed scope.** "Error on any unset field" **rejects configs that work today** — `len-serv-001.yaml` omits `network_renderer` entirely and relies on the serde default. Fail-closed covers exactly the 10 defaultless fields.

See [ORCHESTRATION.md](../ORCHESTRATION.md) (one level up) for the coordinator + worker protocol.
