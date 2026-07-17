<!-- file: docs/agent-tasks/checkin/README.md -->
<!-- version: 1.0.0 -->
<!-- guid: e3dcaaf4-350c-46a8-96b7-9d6e5d86881a -->
<!-- last-edited: 2026-07-16 -->

# Workstream — checkin (application health reporting)

Report and surface whether an installed machine's applications are actually running — which `MachineStatus` (a *registration* lifecycle) does not cover — and make sure silence never reads as health. From spec C6, D-A.

Design: [`deploy-system-design.md`](../../specs/deploy-system-design.md) · Plan + authoritative wave/tier table: [`deploy-system-plan.md`](../../specs/deploy-system-plan.md) · Bucket sort: [`BREAKDOWN-2026-07-16.md`](../BREAKDOWN-2026-07-16.md)

| Task | Src id | Title | Priority | Effort | Tier | Wave |
|------|--------|-------|----------|--------|------|------|
| TASK-01 | DS-CHK-01 | `app_status.rs` client reporter (mirror `luks_sync`) | P2 | S | Haiku-class | 1 |
| TASK-02 | DS-CHK-02 | Machine-plane ingest + snapshot fields | P2 | M | Sonnet-class | 4 |
| TASK-03 | DS-CHK-03 | Read-time staleness (`Stale` ≠ healthy) | P2 | M | Sonnet-class | 5 |

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

No same-file collisions **within** this workstream — the three tasks own `app_status.rs`, `lifecycle.rs`, and `staleness.rs` respectively. They serialize only on their data dependency (TASK-02 needs TASK-01's payload; TASK-03 needs TASK-02's fields). **TASK-02 edits `db/mod.rs`**, which the registry workstream's TASK-01/04 also touch — check the package collision table before scheduling.

> **⚠ Do NOT extend `MachineStatus`.** It is the registration lifecycle (`Seen|Pending|Approved|Revoked|Unknown`), and its `Unknown(String)` variant exists to round-trip dirty Python parity data. Application health is a **separate, additive** field.

> **⚠ The bug TASK-03 fixes.** The machine plane writes health **only on check-in** and nothing flips a status on absence — no reaper, no TTL. So a machine whose service died, or whose NIC died, keeps its last-known-good health **forever** and the dashboard renders it **green**. That is worse than showing nothing. Staleness is computed at **read** time — no background job, ingest stays fail-open.

See [ORCHESTRATION.md](../ORCHESTRATION.md) (one level up) for the coordinator + worker protocol.
