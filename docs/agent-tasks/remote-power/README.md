<!-- file: docs/agent-tasks/remote-power/README.md -->
<!-- version: 1.1.0 -->
<!-- guid: 38126f25-1166-46ae-b309-2088c2141138 -->
<!-- last-edited: 2026-07-10 -->

# Workstream — remote power control (`uaa power`)

Add one new subcommand, `uaa power <hostname> on|off|status`, with machine-class dispatch from a hardcoded host registry and an IPMI path that ALWAYS executes `ipmitool` on the server (`172.16.2.30`) over SSH — never on the local macOS machine, where `ipmitool` crashes silently against Supermicro BMCs. DASH/AMT/WoL machine classes exist as loud NotImplemented stubs pointing at `docs/agent-tasks/DEFERRED.md`. Scope, locked decisions, data model, and test matrix come from the spec: [docs/specs/remote-power-design.md](../../specs/remote-power-design.md) and [docs/specs/remote-power-plan.md](../../specs/remote-power-plan.md).

**Execution mode:** SERIAL (coordinator-driven, single task) — trigger: 1 task (below the ≥3 parallel-sweep threshold); its global wave-5 slot is gated by 5 cross-workstream collision merges on `src/cli/args.rs`, `src/main.rs`, and `src/cli/commands.rs` (see collision note below).

**Constellation continuation (2026-07-10):** TASK-01 is MERGED (`src/power/mod.rs` exists on main with IPMI implemented and `AmdDash`/`IntelAmt`/`WakeOnLan` as loud stubs). The constellation plan ([docs/specs/constellation-design.md](../../specs/constellation-design.md), Decision 15/17; plan: [constellation-plan.md](../../specs/constellation-plan.md)) adds TASK-02/TASK-03, which fill the CP-01-created stub files `crates/uaa-core/src/power/dash.rs` and `crates/uaa-core/src/power/amt_wol.rs` — disjoint files, mock-executor tests only, NO hardware. Waves in the table below are per-plan: TASK-01's wave is from the install-ops plan (complete); TASK-02/03 sit in **constellation global wave 3**.

| Task | Src id | Title | Priority | Effort | Tier | Wave |
|------|--------|-------|----------|--------|------|------|
| TASK-01 | todo:remote-power | uaa power <host> on\|off\|status: machine-class dispatch scaffolding + IPMI-via-server path (ssh 172.16.2.30 ipmitool; explicit off/on, no reset) | P2 | M | Sonnet-class | 5 (install-ops — DONE) |
| TASK-02 | ws8-power | AMD DASH power path: dashcli-deb-first with wsman fallback, executor-mocked, replacing the stub | P2 | M | Sonnet-class | 3 (constellation) |
| TASK-03 | ws8-power | Intel AMT (wsman) + Wake-on-LAN (server-side wakeonlan via ssh) replacing stubs | P2 | S | Sonnet-class | 3 (constellation) |

## Wave table

Waves are GLOBAL across the install-ops plan (this workstream owns only wave 5's `remote-power/TASK-01`):

| Wave | Tasks | Prereq | Parallel-safe because |
|---|---|---|---|
| 5 | TASK-01 (+ cross-WS peer `phase-rerun/TASK-02`) | waves 1–4 merged + siblings rebased — specifically `installer-robustness/TASK-02` (wave 1), `installer-robustness/TASK-03` (wave 2), `installer-robustness/TASK-07` (wave 3), `phase-rerun/TASK-01` (wave 4) | wave-5 peer `phase-rerun/TASK-02` shares NO files with TASK-01 (it edits `src/network/ssh_installer/*`; TASK-01 edits `src/power/`, `src/lib.rs`, and CLI wiring) |

Constellation-plan waves (GLOBAL numbering from the constellation skeleton; this workstream owns two wave-3 slots):

| Wave | Tasks | Prereq | Parallel-safe because |
|---|---|---|---|
| 3 (constellation) | TASK-02 (RP-02), TASK-03 (RP-03) | constellation wave 2 merged — specifically CP-01 (wave 1: workspace transform + stub files) and CP-03 (wave 2: fleet config) both on `origin/main`, siblings rebased | disjoint stub fills: TASK-02 edits only `crates/uaa-core/src/power/dash.rs`, TASK-03 edits only `crates/uaa-core/src/power/amt_wol.rs`; `power/mod.rs` dispatch was pre-wired by CP-01, so neither task touches a shared file |

## Ground rules

- Rust only, in exactly the five files the brief names (`src/power/mod.rs` NEW, `src/lib.rs`, `src/cli/args.rs`, `src/main.rs`, `src/cli/commands.rs`); purely additive — no existing match arm, variant, or handler is modified beyond the file-header bump.
- Build + test gate for every task in this workstream:
  ```bash
  cargo test --lib --offline    # Expected: 245+ passed; 0 failed (baseline 237 + >=8 new power tests)
  cargo build --offline
  cargo clippy --offline
  ```
- **Verify every file:line anchor with `grep` before editing** — this task runs in wave 5, AFTER four other tasks have edited the same CLI files, so line numbers in the brief WILL have drifted; the grep hits are authoritative, the line numbers are not.
- File headers MANDATORY: new file gets a fresh 4-line `// file: / // version: / // guid: / // last-edited:` header; every edited file gets version bumped and `last-edited: 2026-07-09` (or the actual edit date), guid preserved.
- HARD RULES (from the operation contract, restated in the brief):
  - `ipmitool` runs FROM THE SERVER over SSH (`ssh 172.16.2.30 'ipmitool -I lanplus -H <bmc> ...'`), never on macOS — the local execution path must not exist in code.
  - Explicit `power off` / `power on` / `status` only; NO `reset`/`cycle` anywhere (unreliable on the X10DSC+) — unrepresentable in the action enum, not merely rejected.
  - SECRETS: no real BMC password in source, tests, fixtures, or committed docs; password arrives at runtime via `UAA_IPMI_PASSWORD` / `--ipmi-password` and is never logged (redacted command form only).
  - Code/docs only — no live command against any BMC, the server (`172.16.2.30`), or any host during implementation; validation is unit tests + build.
  - Workers stay in their worktree and NEVER push/PR/merge — the coordinator owns all git.

## Collision / wave note

TASK-01 shares three files with earlier-wave tasks (from the operation collision matrix) — it MUST NOT start until all of these are merged to `main` and its worktree is rebased:

| Shared file | Colliding tasks (all earlier waves) |
|---|---|
| `src/cli/args.rs` | `phase-rerun/TASK-01` (wave 4) |
| `src/main.rs` | `phase-rerun/TASK-01` (wave 4) |
| `src/cli/commands.rs` | `installer-robustness/TASK-02` (wave 1), `installer-robustness/TASK-03` (wave 2), `installer-robustness/TASK-07` (wave 3), `phase-rerun/TASK-01` (wave 4) |

`src/power/mod.rs` (new) and `src/lib.rs` are uncontested. The wave-5 peer `phase-rerun/TASK-02` shares no files with TASK-01, so both wave-5 tasks may run concurrently.

### Constellation continuation (TASK-02/TASK-03)

Execution mode: PARALLEL DISPATCH — RP-02/RP-03 fill disjoint CP-01 stubs — trigger: 2 tasks in one wave, zero shared files.

TASK-02/03 have NO file collisions with each other or with any constellation peer: each fills exactly one CP-01-created stub (`crates/uaa-core/src/power/dash.rs` vs `crates/uaa-core/src/power/amt_wol.rs`), and the stub-creator collision (`crates/uaa-core/src/power/mod.rs`: CP-01 → CP-03) is resolved by wave order (both merged before wave 3 starts). Additional ground rules for the continuation:

- Each brief's diff is exactly ONE source file (`git diff origin/main --stat` gated in acceptance); `power/mod.rs`, the CLI, and all sibling stubs stay untouched.
- Gate baseline is now **311 passing tests** (`cargo test --lib --offline && cargo build --offline`, clippy `-D warnings`).
- NO hardware validation: len-serv-002 lacks the Linux DASH/WSMAN service and no Intel-AMT host is registered — all protocol commands are built as strings, executed only through `CommandExecutor` mocks in tests, and carry `VERIFY-ON-HW` markers. WoL magic packets are sent FROM the server (`172.16.2.30`) via the executor seam, never from the Mac. NEVER power on unimatrixone — both new briefs enforce the deny-list fail-closed.
- Anchors in the new briefs cite pre-move `src/**` paths; after CP-01 they map to `crates/uaa-core/src/**` — grep old path then mapped path, zero hits at both = STOP.

See [ORCHESTRATION.md](../ORCHESTRATION.md) (one level up) for the coordinator + worker protocol.
