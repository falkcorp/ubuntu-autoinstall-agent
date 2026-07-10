<!-- file: docs/agent-tasks/phase-rerun/README.md -->
<!-- version: 1.0.0 -->
<!-- guid: dac48ff2-eef1-47fb-928f-7af8ba41e470 -->
<!-- last-edited: 2026-07-09 -->

# Workstream — phase-rerun (phase-selective re-run of the Path B installer)

Give the Path B installer (`src/network/ssh_installer/`) the ability to re-run only selected phases (`uaa install --phases 5`, `--from-phase 4`) against an EXISTING on-disk system, non-destructively — today the only retry after a Phase-5 GRUB failure is a full re-run whose `preflight_checks` treats residual state (imported pools, open LUKS mapper, `/mnt/targetos` mounts) as damage and WIPES the disk. Scope, locked decisions (flag grammar, default inertness, `WipeAuthorization` guard, load-bearing mount order), data model, and test matrix are in the spec: `docs/specs/phase-selective-rerun-design.md` and `docs/specs/phase-selective-rerun-plan.md`. A flagless run must stay byte-identical to today's full 0–6 run.

**Execution mode:** SERIAL WAVES (coordinator-driven) — trigger: TASK-02 `Depends on: TASK-01` and both tasks edit `src/network/ssh_installer/installer.rs` (collision matrix); 2 tasks is below the ≥3 mechanically-similar threshold for /parallel-sweep. Tasks sit in GLOBAL waves 4 and 5 of the install-ops plan — each starts only after every prior global wave's PRs are merged and all sibling worktrees are rebased.

| Task | Src id | Title | Priority | Effort | Tier | Wave |
|------|--------|-------|----------|--------|------|------|
| [TASK-01](TASK-01-phase-spec-cli.md) | todo:phase-selective | `--phases <spec>` / `--from-phase <n>`: CLI parsing + phase-selection plumbing (full run unchanged by default) | P1 | L | Opus-class ⚠ review-critical | 4 (global) |
| [TASK-02](TASK-02-mount-existing-target.md) | todo:phase-selective | Non-destructive mount-existing-target prep (assemble md, open LUKS, import rpool then bpool, mount `/` then `/boot` then ESP, binds) + NEVER-wipe guard when Phase 2/3 skipped | P1 | L | Opus-class ⚠ review-critical | 5 (global) |

## Wave table

Waves below are GLOBAL waves from the install-ops skeleton — this workstream intentionally runs late so it rebases onto the installer-robustness merges that rework the same files.

| Wave | Tasks | Prereq | Parallel-safe because |
|---|---|---|---|
| 4 | TASK-01 | global waves 1–3 merged + siblings rebased (installer-robustness/TASK-01 w1, TASK-02 w1, TASK-03 w2, TASK-05 w2, TASK-07 w3 all touch `installer.rs`/`commands.rs`/`disk_ops.rs` first) | wave-4 sibling install-server/TASK-05 touches only `scripts/autoinstall-agent.py` — disjoint |
| 5 | TASK-02 | wave 4 merged (TASK-01 is a hard dependency: `PhaseSelection`/`WipeAuthorization` types) + siblings rebased | wave-5 sibling remote-power/TASK-01 touches `src/power/`, `src/lib.rs`, `args.rs`, `main.rs`, `commands.rs` — disjoint from TASK-02's `installer.rs`/`zfs_ops.rs`/`disk_ops.rs` |

## Ground rules

- Rust only, in exactly the files each brief names; every change is additive (new flags, new types, new prep path) — the flagless default run stays byte-identical to today.
- Build + test gate for every task in this workstream:
  ```bash
  cargo test --lib --offline   # baseline 237 passed — must not regress
  cargo build --offline
  cargo clippy --offline
  ```
- **Verify every file:line anchor with `grep` before editing** — line numbers in each brief are a starting point, not a guarantee. Zero-hit at execution time means STOP and report.
- File headers are MANDATORY: bump `// version:` and `// last-edited:` on every touched file; keep existing `// guid:` values.
- HARD RULES that bind this workstream (restated in each brief):
  1. NEVER wipe/reimage/touch 172.16.2.30 ("the server") or len-serv-003. These tasks are code-only, validated by unit tests and later in VM/QEMU (`scripts/vm-validate.sh`, testing-gates/TASK-01) — never on live servers.
  2. `disk_device` is READ from the live target (`lsblk`/`fdisk`), never guessed from `/dev/sd*` conventions (U1's `/dev/sda` is an IMSM RAID member; the real volume is `/dev/md126`).
  3. SECRETS: no brief may introduce a real `luks_key`/`root_password`/`tpm2_pin` anywhere in git; committed configs carry `REPLACE_AT_PLACE_TIME`. TASK-02's LUKS re-open must use the 0600-tempfile keyfile pattern, never command-line interpolation.
  4. Workers stay in their worktree and NEVER push/PR/merge — the coordinator owns all git.
- This is the wipe-adjacent workstream of the plan: both tasks are Opus-class ⚠ review-critical. Every guard added must ship an anti-over-suppression test proving the DEFAULT full install still wipes + installs exactly as today.

## Collision / wave note

Exact shared files from the install-ops collision matrix:

| Shared file | Tasks that touch it | Resolution |
|---|---|---|
| `src/network/ssh_installer/installer.rs` | installer-robustness/TASK-01 (w1), TASK-05 (w2), TASK-07 (w3), **phase-rerun/TASK-01 (w4), phase-rerun/TASK-02 (w5)**, boot-prod/TASK-02 (w6) | serialize by global wave; TASK-01 and TASK-02 must be in different waves |
| `src/cli/commands.rs` | installer-robustness/TASK-02 (w1), TASK-03 (w2), TASK-07 (w3), **phase-rerun/TASK-01 (w4)**, remote-power/TASK-01 (w5) | serialize by global wave |
| `src/cli/args.rs` | **phase-rerun/TASK-01 (w4)**, remote-power/TASK-01 (w5) | serialize: wave4=phase-rerun/TASK-01, wave5=remote-power/TASK-01 |
| `src/main.rs` | **phase-rerun/TASK-01 (w4)**, remote-power/TASK-01 (w5) | serialize: wave4=phase-rerun/TASK-01, wave5=remote-power/TASK-01 |
| `src/network/ssh_installer/disk_ops.rs` | installer-robustness/TASK-01 (w1), TASK-05 (w2), **phase-rerun/TASK-02 (w5)** | serialize by global wave; TASK-02 consumes TASK-05's keyfile helper |
| `src/network/ssh_installer/zfs_ops.rs` | installer-robustness/TASK-01 (w1), **phase-rerun/TASK-02 (w5)** | serialize by global wave |

TASK-01 (wave 4) and TASK-02 (wave 5) MUST NOT run concurrently — they share `installer.rs`, and TASK-02 consumes TASK-01's types. Within their waves, both are parallel-safe with their listed siblings (disjoint file sets).

See [ORCHESTRATION.md](../ORCHESTRATION.md) (one level up) for the coordinator + worker protocol.
