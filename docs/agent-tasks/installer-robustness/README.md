<!-- file: docs/agent-tasks/installer-robustness/README.md -->
<!-- version: 1.0.0 -->
<!-- guid: 2f6c304e-d920-48b7-a385-dde2e3ff6cc8 -->
<!-- last-edited: 2026-07-09 -->

# Workstream — installer robustness

Harden Path B (`src/network/ssh_installer/`, the PROVEN installer — 7/7 phases on unimatrixone
2026-07-09) and its CLI plumbing: fix the `/dev/sdapN` partition-suffix bug at every call site,
replace the fake disk/network autodetection with real `lsblk --json` / `ip -j` parsing, make the
netplan renderer configurable with proper `dhcp4` rendering, move the LUKS passphrase off the
process command line and environment, harden the config schema, add curtin in-target
compatibility, and document the Path A / Path B split. Scope, locked decisions, and rejected
alternatives are in the spec pair `docs/specs/installer-robustness-design.md` and
`docs/specs/installer-robustness-plan.md` — this README is the dispatch index for the 8 task
briefs in this directory.

**Execution mode:** SERIAL WAVES (coordinator-driven), parallel within a wave — trigger:
`src/cli/commands.rs` is shared by 3 tasks (TASK-02/03/07) and
`src/network/ssh_installer/installer.rs` by 3 tasks (TASK-01/05/07); see the collision note
below. Wave N+1 starts only after every wave-N PR merges and all sibling worktrees rebase.

| Task | Src id | Title | Priority | Effort | Tier | Wave |
|------|--------|-------|----------|--------|------|------|
| TASK-01 | todo:partition-suffix | Route all 11 partition-path call sites through one suffix-aware helper (fix /dev/sdapN bug) | P1 | L | Opus-class ⚠ | 1 |
| TASK-02 | todo:detect_primary_disk | detect_primary_disk: parse lsblk --json (incl. md devices) instead of fragile text matching | P2 | M | Sonnet-class | 1 |
| TASK-03 | todo:detect_network_config | detect_network_config: actually parse ip -j addr / ip -j route (stop returning hardcoded eth0/dhcp) | P2 | M | Sonnet-class | 2 |
| TASK-04 | todo:renderer | Configurable netplan renderer (networkd \| NetworkManager) + proper dhcp4 rendering for dhcp addresses | P2 | M | Sonnet-class | 2 |
| TASK-05 | todo:LUKS_KEY-env | LUKS passphrase via 0600 tempfile keyfile (kill env export + cryptsetup command-line interpolation) | P1 | M | Opus-class ⚠ | 2 |
| TASK-06 | todo:config-schema | serde deny_unknown_fields + YAML round-trip tests for InstallationConfig | P3 | S | Haiku-class | 1 |
| TASK-07 | todo:curtin | curtin in-target compatibility: skip mounts+debootstrap when already inside the target chroot | P3 | M | Sonnet-class | 3 |
| TASK-08 | todo:boot-layout-PathA | Document the Path A (subiquity render pipeline) vs Path B (ssh_installer) split + guardrails | P3 | S | Haiku-class | 1 |

Waves are GLOBAL wave numbers shared with the other install-ops workstreams (boot-prod,
phase-rerun, install-server, testing-gates, remote-power).

## Wave table

| Wave | Tasks | Prereq | Parallel-safe because |
|---|---|---|---|
| 1 | TASK-01, TASK-02, TASK-06, TASK-08 | none | disjoint file sets: T01 = `ssh_installer/{partitions,mod,disk_ops,zfs_ops,system_setup,installer}.rs`; T02 = `src/cli/commands.rs`; T06 = `ssh_installer/config.rs`; T08 = docs only |
| 2 | TASK-03, TASK-04, TASK-05 | wave 1 merged + siblings rebased | T03 shares `src/cli/commands.rs` with T02 (wave 1); T04 shares `system_setup.rs` with T01 and `config.rs` with T06; T05 shares `disk_ops.rs`/`installer.rs` with T01. Within the wave the declared file sets are disjoint — but see the T04 compiler-forced-initializer caution in the collision note |
| 3 | TASK-07 | wave 2 merged + siblings rebased | shares `installer.rs` with T01/T05 and `src/cli/commands.rs` with T02/T03 (all merged by wave 3); global wave 3 also runs boot-prod/TASK-01, which edits `system_setup.rs` after T01/T04 merged |

Cross-workstream constraints touching this WS: `testing-gates/TASK-01` (global wave 2, the
QEMU+swtpm gate) **depends on TASK-01's merge** — the /dev/vda suffix fix is what makes the
virtio VM gate passable. Later global waves reuse this WS's files after it completes:
`phase-rerun/TASK-01` + `remote-power/TASK-01` (`src/cli/commands.rs`, waves 4–5),
`phase-rerun/TASK-02` (`disk_ops.rs`/`zfs_ops.rs`/`installer.rs`, wave 5), `boot-prod/TASK-02`
(`mod.rs`/`installer.rs`, wave 6).

## Ground rules

- Rust only, in the files named by each task brief; changes are additive or the exact transform
  the brief names — no drive-by refactors, no signature changes beyond those a brief declares.
- Build + test gate for every task in this workstream (all 8 are code or docs; code briefs add
  clippy):
  ```bash
  cargo test --lib --offline    # Expected: 237+ passed; 0 failed (baseline 237)
  cargo build --offline         # Expected: exit 0
  cargo clippy --offline        # code briefs: no new warnings
  ```
- **Verify every file:line anchor with `grep` before editing** — line numbers in each brief are
  a starting point, not a guarantee. Zero hits = STOP and report.
- File headers are MANDATORY: bump `version` + `last-edited` (keep `guid`) on every file touched.
- HARD RULES (restated in every brief): NEVER wipe/reimage/touch 172.16.2.30 ("the server") or
  len-serv-003 — all tasks here are code/docs only, validated by unit tests and later in
  VM/QEMU, never on live servers. `disk_device` is READ from the live target (`lsblk`/`fdisk`),
  NEVER guessed from /dev/sd* conventions (U1's /dev/sda is an IMSM RAID *member*; the real
  volume is /dev/md126). No real `luks_key`/`root_password`/`tpm2_pin` ever enters git —
  committed configs carry `REPLACE_AT_PLACE_TIME`. Workers stay in their worktree and NEVER
  push/PR/merge — the coordinator owns all git.

## Collision / wave note

Shared files (from the operation collision matrix) and their serialization:

| Shared file | Tasks that touch it | Resolution |
|---|---|---|
| `src/cli/commands.rs` | TASK-02, TASK-03, TASK-07 (+ phase-rerun/TASK-01, remote-power/TASK-01 later) | serialize: wave1=T02, wave2=T03, wave3=T07 |
| `src/network/ssh_installer/installer.rs` | TASK-01, TASK-05, TASK-07 (+ phase-rerun/TASK-01/02, boot-prod/TASK-02 later) | serialize: wave1=T01, wave2=T05, wave3=T07 |
| `src/network/ssh_installer/system_setup.rs` | TASK-01, TASK-04 (+ boot-prod/TASK-01 in wave 3) | serialize: wave1=T01, wave2=T04 |
| `src/network/ssh_installer/config.rs` | TASK-04, TASK-06 | serialize: wave1=T06, wave2=T04 |
| `src/network/ssh_installer/disk_ops.rs` | TASK-01, TASK-05 (+ phase-rerun/TASK-02 later) | serialize: wave1=T01, wave2=T05 |
| `src/network/ssh_installer/zfs_ops.rs` | TASK-01 (+ phase-rerun/TASK-02 later) | no intra-WS collision |
| `src/network/ssh_installer/mod.rs` | TASK-01 (+ boot-prod/TASK-02 later) | no intra-WS collision |

**Wave-2 caution:** TASK-04 adds a field to `InstallationConfig`, which forces a one-line
initializer in every exhaustive struct literal — the compiler will flag
`src/cli/commands.rs` (TASK-03's file) and a `#[cfg(test)]` helper in
`src/network/ssh_installer/installer.rs` (TASK-05's file). The coordinator must serialize the
wave-2 MERGES (any order), rebasing the remaining wave-2 worktrees after each merge, instead of
merging all three blindly.

See [ORCHESTRATION.md](../ORCHESTRATION.md) (one level up) for the coordinator + worker protocol.
