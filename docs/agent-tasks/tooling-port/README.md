<!-- file: docs/agent-tasks/tooling-port/README.md -->
<!-- version: 1.0.0 -->
<!-- guid: 135c297b-223f-4617-8eea-061b0f335ad8 -->
<!-- last-edited: 2026-07-10 -->

# Workstream — tooling port (`uaa iso/config/image/vm-validate` + script retirement)

Port the four battle-tested shell pipelines into `crates/uaa-core` behind the CP-01 pre-wired `uaa` subcommands — ISO remaster (xorriso), server-local config placement with place-time secret injection, squashfs installer-image build, and the 8-stage QEMU+swtpm VM validation gate — then, as the plan's final gated step, delete the Python agent and all four shell scripts. Every external tool (xorriso, unsquashfs/mksquashfs, qemu/swtpm) is invoked through `CommandExecutor` so tests run against mocks; the shell originals stay authoritative and untouched until TASK-05. From ws9-tooling; spec: [constellation-design.md](../../specs/constellation-design.md) (Goals "retire the ported shell tools only after their Rust replacement passes its gate", Decisions 16/17).

| Task | Src id | Title | Priority | Effort | Tier | Wave |
|------|--------|-------|----------|--------|------|------|
| TASK-01 | ws9-tooling | `uaa iso remaster`: port make-ssh-ready-iso.sh (xorriso extract/patch/repack, idempotent cmdline tokens, --autoinstall opt-in) | P1 | L | Sonnet-class | 2 |
| TASK-02 | ws9-tooling | `uaa config place/inject`: port deploy-usb-configs.sh incl. --inject-from (0600 staging, git-tree refusal, perms check, placeholder hard-gate) | P1 | M | Opus-class ⚠ | 2 |
| TASK-03 | ws9-tooling | `uaa image build`: port build-installer-image.sh (unsquashfs overlay, agent install, subiquity mask, mksquashfs zstd) | P2 | M | Sonnet-class | 3 |
| TASK-04 | ws9-tooling | `uaa vm-validate`: port the 8-stage QEMU+swtpm harness reusing utils/qemu.rs+vm.rs | P2 | L | Sonnet-class | 2 |
| TASK-05 | ws9-tooling | Retire scripts/autoinstall-agent.py + the four ported shell scripts (deletion, hard-gated) | P3 | S | Haiku-class | 9 |

Waves are GLOBAL across the constellation plan (skeleton `.global_waves`): TP-01/02/04 sit in wave 2 (prereq: wave 1 `CP-01` merged + siblings rebased — each fills its own CP-01 stub); TP-03 sits in wave 3 (prereq: wave 2 merged — TP-01 for the shared `iso/` module); TP-05 sits in wave 9 (prereq: TG-03 merged + ⛔ OPERATOR-CONFIRMED M6 cutover + 2-week window — Bucket-3 gate).

## Ground rules

- Rust only, one uaa-core stub file per task (`crates/uaa-core/src/iso/remaster.rs`, `crates/uaa-core/src/config_place.rs`, `crates/uaa-core/src/iso/image_build.rs`, `crates/uaa-core/src/vm_validate.rs`) plus that task's pre-wired CLI arm in `crates/uaa/src/`; TASK-05 is deletions only. TP-01..04 are purely additive; the shell scripts are NEVER edited by this workstream — TASK-05 deletes them.
- Build + test gate for every task:
  ```bash
  cargo test --lib --offline && cargo build --offline
  # Expected: all tests pass (baseline 311 + this workstream's new tests), build clean
  cargo clippy --offline -- -D warnings
  # Expected: no warnings
  ```
- **Verify every file:line anchor with `grep` before editing** — these tasks run in waves 2–9, AFTER the CP-01 workspace transform; every brief carries the path map (`src/**` → `crates/uaa-core/src/**`, CLI → `crates/uaa/src/**`). Grep the old path, then the mapped path; zero hits at BOTH = STOP and report.
- File headers MANDATORY: new files get a fresh 4-line header with a new uuid4; every edited file gets version bumped + `last-edited` updated, guid preserved.
- HARD RULES (operation contract, restated verbatim in every brief): NO hardware actions — mock executors + cargo only (plus the QEMU harness where a brief says so); NEVER wipe/write 172.16.2.30 or len-serv-003; `disk_device` read from the live target, never guessed; ipmitool via `ssh 172.16.2.30`; NEVER power on unimatrixone; no real secret committed — `REPLACE_AT_PLACE_TIME` placeholders stay placeholders (TASK-02 additionally: secret values never in argv/logs/Debug output); workers never push/PR/merge.
- TASK-04 scope rule: `scripts/vm-validate.sh` stays the authoritative gate until `testing-gates/TASK-03` proves the port — the port ships alongside it, never replaces it early.

## Collision / wave note

From the skeleton collision matrix, this workstream's only collision surface is the stub pattern: "stub-pattern (uaa-core stubs by CP-01; …) — serialize: dependency-ordered (stub wave precedes fill wave); each stub file has EXACTLY ONE filling task". TP-01/02/04 each fill a distinct CP-01 stub (disjoint files → parallel-safe in global wave 2); TP-03 waits for TP-01 because both live under `crates/uaa-core/src/iso/` (shared `iso/mod.rs`); TP-05 collides with nothing ("pure deletions, nothing else open" — wave 9).

**Execution mode: PARALLEL DISPATCH within wave — TP-01/02/04 disjoint; TP-03 after TP-01; TP-05 hard-gated removal — trigger: 3 parallel-safe tasks in local wave 1 (meets the ≥3 parallel-sweep threshold).**

⛔ TASK-05 additionally carries the Bucket-3 operator gate: **DO NOT DISPATCH until the operator confirms M6 cutover complete + 2-week window elapsed.** No wave-9 dispatch on green CI alone.

Link: See [ORCHESTRATION.md](../ORCHESTRATION.md) for the coordinator + worker protocol.
