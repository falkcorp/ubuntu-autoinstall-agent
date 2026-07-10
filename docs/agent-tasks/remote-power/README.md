<!-- file: docs/agent-tasks/remote-power/README.md -->
<!-- version: 1.0.0 -->
<!-- guid: 38126f25-1166-46ae-b309-2088c2141138 -->
<!-- last-edited: 2026-07-09 -->

# Workstream — remote power control (`uaa power`)

Add one new subcommand, `uaa power <hostname> on|off|status`, with machine-class dispatch from a hardcoded host registry and an IPMI path that ALWAYS executes `ipmitool` on the server (`172.16.2.30`) over SSH — never on the local macOS machine, where `ipmitool` crashes silently against Supermicro BMCs. DASH/AMT/WoL machine classes exist as loud NotImplemented stubs pointing at `docs/agent-tasks/DEFERRED.md`. Scope, locked decisions, data model, and test matrix come from the spec: [docs/specs/remote-power-design.md](../../specs/remote-power-design.md) and [docs/specs/remote-power-plan.md](../../specs/remote-power-plan.md).

**Execution mode:** SERIAL (coordinator-driven, single task) — trigger: 1 task (below the ≥3 parallel-sweep threshold); its global wave-5 slot is gated by 5 cross-workstream collision merges on `src/cli/args.rs`, `src/main.rs`, and `src/cli/commands.rs` (see collision note below).

| Task | Src id | Title | Priority | Effort | Tier | Wave |
|------|--------|-------|----------|--------|------|------|
| TASK-01 | todo:remote-power | uaa power <host> on\|off\|status: machine-class dispatch scaffolding + IPMI-via-server path (ssh 172.16.2.30 ipmitool; explicit off/on, no reset) | P2 | M | Sonnet-class | 5 |

## Wave table

Waves are GLOBAL across the install-ops plan (this workstream owns only wave 5's `remote-power/TASK-01`):

| Wave | Tasks | Prereq | Parallel-safe because |
|---|---|---|---|
| 5 | TASK-01 (+ cross-WS peer `phase-rerun/TASK-02`) | waves 1–4 merged + siblings rebased — specifically `installer-robustness/TASK-02` (wave 1), `installer-robustness/TASK-03` (wave 2), `installer-robustness/TASK-07` (wave 3), `phase-rerun/TASK-01` (wave 4) | wave-5 peer `phase-rerun/TASK-02` shares NO files with TASK-01 (it edits `src/network/ssh_installer/*`; TASK-01 edits `src/power/`, `src/lib.rs`, and CLI wiring) |

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

See [ORCHESTRATION.md](../ORCHESTRATION.md) (one level up) for the coordinator + worker protocol.
