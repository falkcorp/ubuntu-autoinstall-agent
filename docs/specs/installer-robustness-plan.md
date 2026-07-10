<!-- file: docs/specs/installer-robustness-plan.md -->
<!-- version: 1.0.0 -->
<!-- guid: c2cdda90-9497-4a36-9820-5819e56d4a48 -->
<!-- last-edited: 2026-07-09 -->

# Installer Robustness — Implementation Plan

**Design spec:** [installer-robustness-design.md](installer-robustness-design.md) (decisions LOCKED there; this plan sequences them — it does not reopen them).
**Workstream:** `installer-robustness` — 8 tasks, briefs in `docs/agent-tasks/installer-robustness/`.
**Repo:** `/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent` · default branch `main` · workers in `$REPO/.worktrees/<ws>-<slug>` on `agent/<ws>-<slug>` off `origin/main`; workers NEVER push/PR/merge — the coordinator owns all git.

**Gate (every step):**

- Run: `cargo test --lib --offline`
  Expected: `237+ passed; 0 failed` (baseline 237; steps add tests)
- Run: `cargo build --offline`
  Expected: exit 0
- Run: `cargo clippy --offline` (code steps, i.e. all but Step 8)
  Expected: no new warnings

**Hard rules in force for every step:** no touching 172.16.2.30 or len-serv-003 (code/docs only, VM-validated later by `testing-gates`); `disk_device` is read from the live target, never guessed; no real `luks_key`/`root_password`/`tpm2_pin` in git (`REPLACE_AT_PLACE_TIME` placeholders only); file headers bumped on every touched file.

---

## Wave order

Waves are GLOBAL across the operation (see `docs/agent-tasks/ORCHESTRATION.md`); this workstream occupies waves 1–3. Within a wave, tasks run in parallel (no shared files inside a wave for this WS); across waves, later tasks rebase onto merged earlier waves.

| Global wave | This WS's tasks | Cross-WS notes |
|---|---|---|
| 1 | TASK-01, TASK-02, TASK-06, TASK-08 | `testing-gates/TASK-01` (wave 2) depends on our TASK-01 merging |
| 2 | TASK-03, TASK-04, TASK-05 | rebase onto wave-1 merges (helper, deny_unknown_fields) |
| 3 | TASK-07 | shares `system_setup.rs`-adjacent turf with `boot-prod/TASK-01` (different files ok); shares `installer.rs`/`commands.rs` history with waves 1–2 — rebase first |

**Collision matrix (from the skeleton; exact shared files):**

- `src/network/ssh_installer/installer.rs`: TASK-01 (w1) → TASK-05 (w2) → TASK-07 (w3), later also `phase-rerun/TASK-01` (w4), `phase-rerun/TASK-02` (w5), `boot-prod/TASK-02` (w6)
- `src/network/ssh_installer/disk_ops.rs`: TASK-01 (w1) → TASK-05 (w2), later `phase-rerun/TASK-02` (w5)
- `src/network/ssh_installer/system_setup.rs`: TASK-01 (w1) → TASK-04 (w2), later `boot-prod/TASK-01` (w3)
- `src/network/ssh_installer/config.rs`: TASK-06 (w1) → TASK-04 (w2)
- `src/network/ssh_installer/zfs_ops.rs`: TASK-01 (w1), later `phase-rerun/TASK-02` (w5)
- `src/network/ssh_installer/mod.rs`: TASK-01 (w1), later `boot-prod/TASK-02` (w6)
- `src/cli/commands.rs`: TASK-02 (w1) → TASK-03 (w2) → TASK-07 (w3), later `phase-rerun/TASK-01` (w4), `remote-power/TASK-01` (w5)

Every wave-2+ worker's first action after `worktree add` is `git rebase origin/main` (the ⛔ START HERE block in each brief).

---

## Step 1 — Partition-path helper sweep (wave 1)

**Brief:** `docs/agent-tasks/installer-robustness/TASK-01-partition-suffix-helper.md` · P1 · L · Opus-class · ⚠ review-critical · depends on: none

Create `src/network/ssh_installer/partitions.rs` with `partition_path(disk: &str, n: u32) -> String` (append `p` only when the device basename ends in an ASCII digit — LOCKED) + unit-test matrix (`nvme0n1`, `md126`, `sda`, `vda`, `loop0`, bare name). Register `pub mod partitions;` in `mod.rs`. Route ALL 11 production call sites AND the 2 `#[cfg(test)]` builders (`build_mkfs_esp`/`build_mkfs_reset`) through it: `disk_ops.rs` (~320/~325/~340/~348 + ~390/~395), `zfs_ops.rs` (~200), `system_setup.rs` (~46/~51/~60/~573/~884, plus the `${DISK}p1` doc comment at ~66), `installer.rs` (~566, and ~605 where `{d}p4` appears TWICE in one shell string — sweep both). Correct the 4 tests asserting `sdapN` to `sdaN` (`zfs_ops.rs:381`, `system_setup.rs:1001/1026/1032`) in the same commit. EXCLUDE `src/utils/qemu.rs:118` (loop device — already correct; LOCKED).

- Run: `grep -rn '}p[1-4]' src --include='*.rs'`
  Expected: only `src/utils/qemu.rs` remains (pre-change: 15 code hits + 1 doc comment across 5 files)
- Run: `grep -rn 'sdap' src --include='*.rs'`
  Expected: 0 hits (pre-change: 4)
- Run + Expected: the standard gate (tests/build/clippy above)

## Step 2 — detect_primary_disk via lsblk --json (wave 1)

**Brief:** `docs/agent-tasks/installer-robustness/TASK-02-detect-primary-disk-json.md` · P2 · M · Sonnet-class · depends on: none

Rewrite `detect_primary_disk` (`src/cli/commands.rs:~635` — anchor: `grep -n 'fn detect_primary_disk' src/cli/commands.rs`) to run `lsblk --json --bytes -o NAME,TYPE,SIZE`, deserialize with `serde_json` (already a dependency), and select per the spec's C2 rule: include md (`raid*`) devices, exclude loop/rom AND disks that are RAID members, prefer largest; hard-error on zero candidates (hard rule 2 — never guess `/dev/sd*`; on unimatrixone `/dev/sda` is an IMSM member, the real volume is `/dev/md126`). Fixture tests for nvme-only, md-over-sda (must return `/dev/md126`), loop/rom-only (error).

- Run: `cargo test --lib --offline detect_primary_disk`
  Expected: new fixture tests pass, incl. the md-over-sda case
- Run + Expected: the standard gate

## Step 3 — Schema hardening: deny_unknown_fields + round-trip tests (wave 1)

**Brief:** `docs/agent-tasks/installer-robustness/TASK-06-config-schema-hardening.md` · P3 · S · Haiku-class · depends on: none

Add `#[serde(deny_unknown_fields)]` to `InstallationConfig` (`src/network/ssh_installer/config.rs` — anchor: `grep -n 'deny_unknown_fields' src/network/ssh_installer/config.rs` expects 0 hits pre-change). Add tests: round-trip `examples/configs/install/unimatrixone.yaml` (struct fields already match its keys 1:1 — scout-verified, LOCKED-safe), plus an unknown-key YAML string that MUST fail to deserialize. Committed example configs keep `REPLACE_AT_PLACE_TIME` placeholders (hard rule 4).

- Run: `grep -n 'deny_unknown_fields' src/network/ssh_installer/config.rs`
  Expected: 1 hit on `InstallationConfig`
- Run + Expected: the standard gate

## Step 4 — Path A/B split documentation (wave 1)

**Brief:** `docs/agent-tasks/installer-robustness/TASK-08-path-a-b-split-doc.md` · P3 · S · Haiku-class · depends on: none

Write NEW `docs/architecture-path-split.md` (Path A = `src/autoinstall/` subiquity render/place/verify + golden tests, STILL LIVE — removal explicitly NOT tasked, LOCKED; Path B = `src/network/ssh_installer/`, proven 7/7 phases on unimatrixone 2026-07-09) plus guardrails; add a README.md pointer. Docs-only; no source files touched.

- Run: `test -f docs/architecture-path-split.md && grep -c 'src/autoinstall' docs/architecture-path-split.md`
  Expected: file exists; 1+ hits
- Run + Expected: the standard gate (proves no source regression; count unchanged from wave-1 siblings' merges)

## Step 5 — LUKS keyfile (wave 2)

**Brief:** `docs/agent-tasks/installer-robustness/TASK-05-luks-keyfile.md` · P1 · M · Opus-class · ⚠ review-critical · depends on: none (rebases onto wave 1)

In `disk_ops.rs::setup_luks_encryption` (~333; anchors: `grep -n 'cryptsetup luksFormat' src/network/ssh_installer/disk_ops.rs` ~340, `grep -n 'cryptsetup open' ...` ~348) replace both `echo '<pass>' |` pipelines with a 0600 tempfile keyfile passed via `--key-file` to BOTH `luksFormat` and `open`, shredded (`shred -u`) on success AND failure paths — mirroring the proven Tang pattern (`grep -n 'uaa-tang-enroll.key' src/network/ssh_installer/system_setup.rs` ~660). Remove the inert `LUKS_KEY` env export from `installer.rs::setup_installation_variables` (anchor: `grep -n '"LUKS_KEY"' src/network/ssh_installer/installer.rs` ~483; scout-confirmed the export is a one-shot `runner.execute()` that never persists — downstream reads `config.luks_key`). LOCKED: stdin-only hiding while keeping the env export was rejected. Wipe-adjacent code (`luksFormat`): VM-validation only, never a live server (hard rule 1). Uses `partition_path` from Step 1 for the p4 paths.

- Run: `grep -rn "echo '{}' | cryptsetup" src/network/ssh_installer/disk_ops.rs`
  Expected: 0 hits
- Run: `grep -n '"LUKS_KEY"' src/network/ssh_installer/installer.rs`
  Expected: 0 hits
- Run: `cargo test --lib --offline luks`
  Expected: command-builder tests prove no passphrase substring in built commands; `--key-file` present in both calls
- Run + Expected: the standard gate

## Step 6 — detect_network_config: real ip -j parsing (wave 2)

**Brief:** `docs/agent-tasks/installer-robustness/TASK-03-detect-network-config-parse.md` · P2 · M · Sonnet-class · depends on: none (rebases onto Step 2's merged commands.rs)

Rewrite `detect_network_config` (`src/cli/commands.rs:~654` — anchor: `grep -n 'fn detect_network_config' src/cli/commands.rs`; hardcoded `eth0` at ~657) to parse `ip -j route` (default route → iface + gateway) and `ip -j addr` (global IPv4 → CIDR), falling back to `"dhcp"` when no static address is determinable (rendered validly by Step 7). Only the legacy no-`--config` path is affected. Fixture tests on captured `ip -j` JSON.

- Run: `cargo test --lib --offline detect_network_config`
  Expected: fixture tests pass; no hardcoded-eth0 production return remains
- Run + Expected: the standard gate

## Step 7 — Netplan renderer + dhcp4 rendering (wave 2)

**Brief:** `docs/agent-tasks/installer-robustness/TASK-04-netplan-renderer-dhcp.md` · P2 · M · Sonnet-class · depends on: none (rebases onto Step 3's merged config.rs — the new field MUST carry `#[serde(default)]` to coexist with `deny_unknown_fields`)

Add `NetplanRenderer` enum + `#[serde(default)] netplan_renderer` field to `InstallationConfig` (`config.rs`); in `system_setup.rs::setup_network_configuration` (anchor: `grep -n 'renderer: networkd' src/network/ssh_installer/system_setup.rs` ~199) render the configured renderer (**default `networkd`**, byte-identical output for existing configs — LOCKED) and, when `network_address == "dhcp"`, render `dhcp4: true` instead of a literal-`dhcp` `addresses:` entry (today invalid netplan). Tests: both renderer strings; dhcp4 branch; static path unchanged.

- Run: `cargo test --lib --offline netplan`
  Expected: renderer + dhcp4 tests pass; static-address rendering asserted unchanged
- Run + Expected: the standard gate

## Step 8 — curtin in-target compatibility (wave 3)

**Brief:** `docs/agent-tasks/installer-robustness/TASK-07-curtin-in-target.md` · P3 · M · Sonnet-class · depends on: none (rebases onto waves 1–2: `installer.rs` after Steps 1/5, `commands.rs` after Steps 2/6)

Additive detection (LOCKED): a conservative already-inside-target-chroot check (default FALSE) wired through `installer.rs` and `cli/commands.rs`; when true, skip mounts + debootstrap and run post-install configuration only. Pre-change anchor: `grep -rn 'curtin' --include='*.rs' src/` → 0 hits. MUST include an anti-over-suppression test proving a bare (non-chroot) run does not skip anything.

- Run: `cargo test --lib --offline in_target`
  Expected: detection + anti-over-suppression tests pass
- Run + Expected: the standard gate

---

## Step ↔ task ↔ wave summary

| Step | Task brief | Wave | Tier | Files |
|---|---|---|---|---|
| 1 | `TASK-01-partition-suffix-helper.md` | 1 | Opus ⚠ | partitions.rs (new), mod.rs, disk_ops.rs, zfs_ops.rs, system_setup.rs, installer.rs |
| 2 | `TASK-02-detect-primary-disk-json.md` | 1 | Sonnet | cli/commands.rs |
| 3 | `TASK-06-config-schema-hardening.md` | 1 | Haiku | config.rs |
| 4 | `TASK-08-path-a-b-split-doc.md` | 1 | Haiku | docs/architecture-path-split.md (new), README.md |
| 5 | `TASK-05-luks-keyfile.md` | 2 | Opus ⚠ | disk_ops.rs, installer.rs |
| 6 | `TASK-03-detect-network-config-parse.md` | 2 | Sonnet | cli/commands.rs |
| 7 | `TASK-04-netplan-renderer-dhcp.md` | 2 | Sonnet | system_setup.rs, config.rs |
| 8 | `TASK-07-curtin-in-target.md` | 3 | Sonnet | installer.rs, cli/commands.rs |

Downstream consumer: `testing-gates/TASK-01` (QEMU+swtpm virtio gate, wave 2) depends on Step 1 — the virtio `/dev/vda` run is the end-to-end proof of the suffix fix (ESP GUID detection self-heals phases 4–6 and can mask it; only Phase 2/3 failures prove it).

**Note to brief drafters:** each brief re-verifies its anchors with the exact `grep_cmd` lines from the evidence file before editing (line numbers above are scout-observed, marked `~`, and never trusted blind). Avoid `>=`-style tokens in verify bash blocks — the active plan-op write-fence hook can misread them as redirects.
