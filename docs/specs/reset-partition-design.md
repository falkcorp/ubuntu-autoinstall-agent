<!-- file: docs/specs/reset-partition-design.md -->
<!-- version: 1.0.0 -->
<!-- guid: 5784b847-2eee-4e1e-8f8b-5ad791b1320f -->
<!-- last-edited: 2026-07-09 -->

# RESET Partition Population + GRUB Recover Entry — Design Spec

**Status:** Approved — ready for implementation planning
**Scope:** Rust, Path B installer only (`src/network/ssh_installer/`); one NEW module + two small
wiring edits. Companion plan: `docs/specs/reset-partition-plan.md`. Workstream: `boot-prod`
(TASK-02 is this design; TASK-01 efibootmgr is specified in its own brief and gets one plan step).

---

## Motivation

The installer already carves out a dedicated recovery partition and then abandons it:

- RESET p2 (4 GiB, GPT type 8300, name `RESET`) is **created** in
  `DiskManager::create_partitions` (`src/network/ssh_installer/disk_ops.rs`, fn at ~245, sgdisk
  step "Create RESET (p2)" at ~273: `sgdisk -n 2:0:+4G -t 2:8300 -c 2:'RESET' {disk}`).
- It is **formatted** in `DiskManager::format_partitions` (fn at ~314, `mkfs.ext4 -F -L RESET
  {}p2` at ~325).
- It is **never mounted or populated** anywhere in `src/` or `installer-image/`
  (`grep -rn "mount.*RESET\|RESET.*mount" src/ installer-image/` → 0 hits).

Meanwhile the fleet's only recovery story is walking a USB stick to the machine. `todo.md:114`
("Populate the RESET partition") records the operator's intent: put a copy of the bootable USB
plus the debootstrap tarball on p2, add a GRUB "reset/recover" entry, and gate the destructive
flow on an explicit typed confirmation.

Two repo facts shape what "recovery content" can even be:

1. **No in-repo kernel/initrd pipeline exists.** The installer debootstraps the target live from
   a mirror (`installer.rs` ~572/~580, `debootstrap {} /mnt/targetos ...`), and the only bootable
   media artifact the repo produces is the **re-mastered stock Ubuntu Server ISO** from
   `scripts/make-ssh-ready-iso.sh` (header line 7: "Re-master a stock Ubuntu Server ISO"). That
   script already extracts and patches the ISO's `/boot/grub/loopback.cfg` when present
   (make-ssh-ready-iso.sh lines 83–126), which is exactly the hook GRUB loopback booting needs.
2. **A debootstrap base tarball cache already has a convention.** Phase 4 mounts a
   `uaacache`-labelled device read-only at `/mnt/uaacache` and consumes
   `{release}-$(dpkg --print-architecture)-base.tar.gz` via `--unpack-tarball`
   (`system_setup.rs` ~107–116). A copy of that tarball on RESET makes an offline reinstall
   possible from the recovery environment.

**Goal:** after every Path B install, RESET p2 holds a bootable copy of the SSH-ready recovery
ISO plus the debootstrap base tarball (when cached), and the installed GRUB menu offers a
"UAA reset/recover" entry that loopback-boots that ISO — with the DESTRUCTIVE wipe living in the
recovery environment behind a literal typed `nuke it` gate, never in the installer.

## Goals

- Populate RESET p2 during Phase 5 with: (a) `uaa-recovery.iso` — a byte-for-byte copy of the
  re-mastered SSH-ready ISO the live session booted from; (b) the debootstrap base tarball from
  `/mnt/uaacache` when present; (c) `uaa-reset.sh` — the gated destructive-reset helper the
  operator runs *inside the recovery environment*.
- Write a GRUB drop-in (`/etc/grub.d/40_uaa_reset`-style) into the target chroot BEFORE the
  existing `update-grub` call in `configure_grub_in_chroot` runs (the "Updating GRUB config" step,
  `system_setup.rs` ~562), so the installed menu gains a "UAA reset/recover" loopback entry.
- The installer's part is strictly NON-DESTRUCTIVE staging: files + menu entry only. The wipe
  gate — operator literally types `nuke it` — executes only in the recovery environment.
- ISO-too-large-for-4-GiB is detected and handled by skip-with-warning, never by failing the
  install.
- Re-running the installer (or the phase-rerun flow from `phase-rerun/`) re-populates
  idempotently: no duplicate menu entries, no corrupt half-copies treated as complete.
- New code lives in a NEW module `src/network/ssh_installer/reset_partition.rs` to minimize
  same-file collisions with the six other tasks touching `installer.rs`/`system_setup.rs`.

## Non-goals (v1)

- **Subiquity reset-partition tooling** — rejected. Path A/subiquity is not used by Path B (the
  proven installer), and pulling subiquity's reset machinery would import a stack we do not run.
- **A separate recovery kernel/initrd/squashfs pipeline** — rejected. No in-repo source exists
  for one (scout-verified); the re-mastered ISO is the only bootable recovery artifact and
  loopback-booting it is sufficient.
- Automating the *destructive* reset end-to-end (auto-fetch config, auto-wipe). v1 stages a
  gated helper script; hardening/automation of the in-recovery flow is a follow-up.
- Touching `config.rs` (new config fields). The ISO source is auto-detected from the live boot
  medium; no schema change — keeps this task out of the `config.rs` collision set.
- Fixing the `todo.md:120` first-boot fall-through (loader boots USB despite BootOrder) — that is
  boot-prod/TASK-01 + hardware triage territory.

## Decisions (locked during design)

1. **Recovery content = the re-mastered SSH-ready USB ISO, loopback-booted.** The
   `make-ssh-ready-iso.sh` output is the only in-repo bootable recovery artifact; GRUB loopback
   (`loopback loop $isofile` + `configfile /boot/grub/loopback.cfg`) boots it from an ext4
   partition. Losing alternative: a separate recovery kernel/initrd pipeline (no in-repo source;
   rejected).
2. **ISO source is the live boot medium itself, sized via `isosize`.** The Path B live session
   was dd-booted from exactly the ISO we want to preserve; `findmnt -n -o SOURCE /cdrom` names
   the backing device and `isosize <dev>` yields the exact ISO9660 byte length, so a
   `dd ... count_bytes` copy reproduces the original file. Losing alternative: a new
   `InstallationConfig` field pointing at an ISO path (adds a `config.rs` collision and an
   operator chore; rejected for v1 — the auto-detect degrades to skip-with-warning when `/cdrom`
   is absent, e.g. curtin/netboot sessions).
3. **Debootstrap tarball rides along when `/mnt/uaacache` has it.** Reuse the exact Phase 4
   mount incantation (`mount -o ro /dev/disk/by-label/uaacache /mnt/uaacache`, guarded by
   `mountpoint -q`) and the exact cache filename convention
   `{release}-$(dpkg --print-architecture)-base.tar.gz`. Losing alternative: re-downloading or
   `--make-tarball`-ing a fresh base (slow, network-dependent; rejected).
4. **GRUB entry = an `/etc/grub.d/40_uaa_reset` drop-in written from the NEW module, called from
   `installer.rs` BEFORE `sc.configure_grub_in_chroot(config)`.** `update-grub` runs as the last
   step of `configure_grub_in_chroot` ("Updating GRUB config", `system_setup.rs` ~562); writing
   the drop-in before that call means the entry lands in `grub.cfg` with zero edits to
   `system_setup.rs` (which three other tasks already collide on). Losing alternative: editing
   `configure_grub_in_chroot` itself (adds a fourth collider to `system_setup.rs`; rejected).
5. **The DESTRUCTIVE gate lives in the recovery environment, not the installer.** The installer
   stages `uaa-reset.sh` onto p2; that script prompts `Type exactly 'nuke it' to continue:` and
   aborts on ANY other input, on empty input, and on non-interactive stdin (fail-closed). The
   installer itself never wipes as part of this feature. Losing alternative: a `uaa reset`
   installer subcommand (puts a wipe path in the everyday binary; rejected).
6. **Skip, never fail.** Every staging failure (no `/cdrom`, `isosize` failure, insufficient
   space, no uaacache) degrades to a logged warning; `phase_5_system_configuration` proceeds.
   The RESET partition is a recovery nicety and must not break the proven 7/7 flow.
   Fail-open on staging; fail-closed only inside the recovery gate.
7. **No menu entry without an ISO.** The drop-in is written only after a verified ISO copy;
   when the ISO is absent/skipped, any stale `40_uaa_reset` drop-in is removed (`rm -f`) so
   `update-grub` never emits a dangling entry.
8. **Partition path via the `partition_path` suffix-aware helper from installer-robustness/
   TASK-01** (wave 1; this task is wave 6, so it lands first). Soft dependency: if the helper
   were unavailable, the existing `format!("{}p2", config.disk_device)` idiom works on the
   current fleet (nvme/md device names) — but the helper is the required form, since bare
   `{}p2` produces `/dev/sdap2` on sd-style disks.

## Data model

No persistent config or schema changes. On-disk layout of RESET p2 (ext4, label `RESET`):

```text
/uaa-recovery.iso          # byte-copy of the re-mastered SSH-ready ISO (loopback-bootable)
/uaa-recovery.iso.stamp    # "<size-bytes> <source-device> <date>" — idempotency marker
/uaa-reset.sh              # 0755; the gated destructive-reset helper (runs in recovery env)
/cache/<release>-<arch>-base.tar.gz   # debootstrap base tarball (when uaacache had one)
/README.uaa-reset          # operator instructions (how to boot the entry, what nuke it does)
```

Target chroot addition (written by the installer, consumed by `update-grub`):

```text
/mnt/targetos/etc/grub.d/40_uaa_reset   # 0755 drop-in emitting the loopback menuentry
```

### Rust surface (normative signatures)

```rust
// file: src/network/ssh_installer/reset_partition.rs
// version: 1.0.0
// guid: <new-guid-at-implementation-time>
// last-edited: <impl date>

//! RESET (p2) recovery-partition staging: recovery ISO copy, debootstrap
//! tarball copy, gated reset helper, and the GRUB loopback drop-in.
//! NON-DESTRUCTIVE: only stages files; the wipe gate lives in uaa-reset.sh.

use crate::error::Result;
use crate::network::executor::CommandExecutor;
use super::config::InstallationConfig;

/// Mirrors DiskManager / SystemConfigurator: borrows the phase's executor.
pub struct ResetPartitionStager<'a> {
    runner: &'a mut dyn CommandExecutor,
}

impl<'a> ResetPartitionStager<'a> {
    pub fn new(runner: &'a mut dyn CommandExecutor) -> Self { Self { runner } }

    /// Entry point, called from phase_5_system_configuration BEFORE
    /// configure_grub_in_chroot. Never returns Err for content problems —
    /// each sub-step degrades to a logged warning (Decision 6). Err only on
    /// executor/transport failure.
    pub async fn stage(&mut self, config: &InstallationConfig) -> Result<()>;

    // -- internal steps (same async fn shape as DiskManager internals) --
    async fn mount_reset(&mut self, config: &InstallationConfig) -> Result<()>;
    async fn copy_recovery_iso(&mut self, config: &InstallationConfig) -> Result<bool>; // true = ISO present+verified
    async fn copy_debootstrap_tarball(&mut self, config: &InstallationConfig) -> Result<()>;
    async fn write_reset_helper(&mut self) -> Result<()>;
    async fn write_grub_dropin(&mut self, iso_present: bool) -> Result<()>;
    async fn unmount_reset(&mut self) -> Result<()>;

    // -- pure command builders (unit-testable, mirrors DiskManager's
    //    build_sgdisk_* / build_mkfs_* cfg(test) idiom at disk_ops.rs ~380-419;
    //    these are non-test because stage() uses them) --
    fn build_mount_reset_cmd(disk: &str) -> String;
    fn build_iso_size_cmd() -> String;            // findmnt /cdrom + isosize
    fn build_iso_copy_cmd(src_dev: &str, size_bytes: u64) -> String;
    fn build_tarball_copy_cmd(release: &str) -> String;
    fn build_grub_dropin_cmd() -> String;         // heredoc write + chmod 0755
    fn build_grub_dropin_remove_cmd() -> String;  // rm -f (Decision 7)
}

/// Literal script content embedded as consts (single source of truth, testable):
const RESET_HELPER_SH: &str = /* uaa-reset.sh body, see C3 */;
const GRUB_DROPIN: &str = /* 40_uaa_reset body, see C4 */;
const RESET_MOUNT: &str = "/mnt/reset";
const RESET_ISO_NAME: &str = "uaa-recovery.iso";
```

## Components

### C1. `ResetPartitionStager::mount_reset` / `unmount_reset` (`reset_partition.rs`)

Mount p2 at `/mnt/reset` using the TASK-01 `partition_path(disk, 2)` helper (Decision 8):
`mkdir -p /mnt/reset; mountpoint -q /mnt/reset || mount <p2-path> /mnt/reset` — the
`mountpoint -q ||` guard is the same idempotency idiom Phase 4 uses for `/mnt/uaacache`
(`system_setup.rs` ~108). Unmount with `umount /mnt/reset || true` at the end of `stage()`
(also on the early-return warning paths — RESET must not be left mounted into Phase 6).
Mount failure (e.g. p2 missing on a hand-partitioned disk) → warn + return without staging.

### C2. `copy_recovery_iso` — ISO copy with size gate (`reset_partition.rs`)

1. `findmnt -n -o SOURCE /cdrom` → the casper boot medium (e.g. `/dev/sdb`). Empty/missing →
   **warn + skip** (netboot/curtin sessions have no `/cdrom`); returns `false`.
2. `isosize <dev>` → exact ISO9660 byte length. Failure → warn + skip.
3. **Size gate (LOCKED edge case):** compare against free space on the mounted RESET fs
   (`df --output=avail -B1 /mnt/reset`), reserving headroom for the tarball + helper
   (**default reserve: 256 MiB**). A stock Ubuntu Server ISO ≈ 2.6–3 GiB fits 4 GiB ext4; a
   fatter re-master may not. Too large → `warn!("RESET: recovery ISO ({} bytes) exceeds free
   space on p2 — skipping ISO + GRUB entry")` and return `false`. The install continues.
4. **Idempotency:** if `/mnt/reset/uaa-recovery.iso.stamp` exists and records the same byte
   size as the current `isosize` output AND the ISO file's on-disk size matches, skip the copy
   (log "already staged"). Otherwise (missing, size mismatch = interrupted prior copy) re-copy.
5. Copy: `dd if=<dev> of=/mnt/reset/uaa-recovery.iso bs=4M count=<size> iflag=count_bytes
   conv=fsync` then write the stamp (`<size> <dev> <date>`). Stamp is written ONLY after a
   successful dd, so a crashed copy is retried next run (fail-closed marker ordering).
6. Return `true` on verified presence (fresh copy or valid pre-existing stamp).

### C3. `write_reset_helper` + `copy_debootstrap_tarball` (`reset_partition.rs`)

- Tarball: re-run the exact Phase 4 cache mount (`mountpoint -q /mnt/uaacache || mount -o ro
  /dev/disk/by-label/uaacache /mnt/uaacache || true` — verbatim reuse of `system_setup.rs`
  ~107–108), compute `CACHE=/mnt/uaacache/{release}-$(dpkg --print-architecture)-base.tar.gz`
  with `release` from `config.debootstrap_release` (same `unwrap_or("resolute")` default as
  Phase 4), and `cp` into `/mnt/reset/cache/` **only if present and only if it fits** (df check
  again). Absent cache → info-level "no uaacache tarball; RESET gets ISO only". `cp` is
  idempotent by overwrite; size-equal short-circuit (`cmp -s || cp`) avoids rewriting 300+ MB.
- Helper: write `RESET_HELPER_SH` to `/mnt/reset/uaa-reset.sh` (0755) plus `README.uaa-reset`.
  The helper is the ONLY place the destructive gate exists:

```bash
#!/usr/bin/env bash
# uaa-reset.sh — DESTRUCTIVE factory reset. Runs in the RECOVERY environment only.
set -euo pipefail
[ -t 0 ] || { echo "ABORT: interactive terminal required."; exit 1; }   # fail-closed
echo "This will DELETE EVERYTHING on this machine's primary disk and reinstall."
printf "Type exactly 'nuke it' to continue: "
read -r answer
[ "$answer" = "nuke it" ] || { echo "ABORT: confirmation not given."; exit 1; }
# ... proceeds to fetch/run `uaa install` against the local config (recovery env has
# the SSH-ready seed; disk_device is READ from lsblk on the live target, never guessed) ...
```

  Gate semantics (normative): exact literal match `nuke it`; empty input, EOF, any other
  string, or non-tty stdin → abort with exit 1. No `--yes` bypass flag in v1.

### C4. `write_grub_dropin` — the "UAA reset/recover" menu entry (`reset_partition.rs`)

Called with `iso_present` from C2. If `false`: `rm -f /mnt/targetos/etc/grub.d/40_uaa_reset`
(Decision 7) and return. If `true`: write `GRUB_DROPIN` to
`/mnt/targetos/etc/grub.d/40_uaa_reset`, `chmod 0755` — an `/etc/grub.d/40_custom`-style
script (LOCKED form). Drop-in body (normative):

```sh
#!/bin/sh
# 40_uaa_reset — UAA reset/recover loopback entry (staged by uaa installer)
exec tail -n +4 $0
menuentry "UAA reset/recover (boots recovery environment — non-destructive)" {
	insmod part_gpt
	insmod ext2
	insmod loopback
	insmod iso9660
	search --no-floppy --label RESET --set=reset_root
	set isofile=/uaa-recovery.iso
	loopback loop ($reset_root)$isofile
	set root=(loop)
	set iso_path=$isofile
	export iso_path
	configfile /boot/grub/loopback.cfg
}
```

Rationale: the ISO's own `/boot/grub/loopback.cfg` is the canonical loopback entry point —
Ubuntu's loopback.cfg passes `iso-scan/filename=${iso_path}` to casper, and
`make-ssh-ready-iso.sh` ALREADY patches that same file with the NoCloud seed cmdline (script
lines 84, 117, 126), so the loopback boot inherits SSH-ready behavior with no extra work.
Selecting the entry is non-destructive: it boots the live recovery env; destruction requires
running `uaa-reset.sh` and typing `nuke it` (C3). Ordering guarantee: `installer.rs` calls
`stage()` (which writes this drop-in) BEFORE `sc.configure_grub_in_chroot(config)`
(`installer.rs` ~542), whose last step is "Updating GRUB config" → `update-grub` picks the
drop-in up in the same install run. No edit to `system_setup.rs` is made or needed.

### C5. Wiring (`mod.rs`, `installer.rs`)

- `mod.rs`: add `pub mod reset_partition;` alongside the existing module list (header version
  bump per repo standard).
- `installer.rs` `phase_5_system_configuration` (~538): before constructing the existing
  `SystemConfigurator`, run the stager in its own scope so the `&mut *self.runner` reborrow
  ends before `sc` takes it:

```rust
// Before: (installer.rs ~538-542)
async fn phase_5_system_configuration(&mut self, config: &InstallationConfig) -> Result<()> {
    let mut sc = SystemConfigurator::new(&mut *self.runner);
    // ...
    sc.configure_grub_in_chroot(config).await?;

// After:
async fn phase_5_system_configuration(&mut self, config: &InstallationConfig) -> Result<()> {
    {
        // Stage RESET p2 (recovery ISO + tarball + gated helper + GRUB drop-in).
        // Non-fatal by design: a staging problem must not break the proven 7/7 flow.
        let mut rp = ResetPartitionStager::new(&mut *self.runner);
        if let Err(e) = rp.stage(config).await {
            warn!("RESET partition staging skipped: {e}");
        }
    }
    let mut sc = SystemConfigurator::new(&mut *self.runner);
    // ... unchanged ...
    sc.configure_grub_in_chroot(config).await?;
```

## Migration / integration

- Purely additive to the phase sequence; no existing call moves. Hosts installed before this
  lands simply have an empty-but-formatted RESET p2 — re-running Phase 5 (via the
  `phase-rerun` workstream's `--phases` selection, or a full reinstall) populates it.
- Collision surface (skeleton `collisions`): `installer.rs` is shared with
  installer-robustness/TASK-01, TASK-05, TASK-07 and phase-rerun/TASK-01/02; `mod.rs` with
  installer-robustness/TASK-01. This task is deliberately **wave 6** (last) so it rebases onto
  all of them; the new-module design keeps its `installer.rs`/`mod.rs` diffs to a few lines.
- Pin during implementation: the exact `partition_path` helper name/signature as merged by
  installer-robustness/TASK-01 (wave 1).

## Milestones

- **M1 — module + builders.** `reset_partition.rs` with consts, command builders, and unit
  tests; `mod.rs` export. Additive — no existing behavior changes (nothing calls it yet).
- **M2 — staging wired into Phase 5 (the ONE behavior-changing milestone).** The `installer.rs`
  call. Gated not by a flag but by construction: every failure path inside `stage()` degrades
  to warn-and-continue (Decision 6), so the worst case equals today's behavior (empty RESET,
  no menu entry). Validate in the QEMU gate (`testing-gates/TASK-01` harness) before any
  hardware run; NEVER on 172.16.2.30 or len-serv-003.

## Files modified

| File | Change |
|---|---|
| `src/network/ssh_installer/reset_partition.rs` | NEW — `ResetPartitionStager`, consts, builders, tests |
| `src/network/ssh_installer/mod.rs` | add `pub mod reset_partition;` + header bump |
| `src/network/ssh_installer/installer.rs` | scoped, non-fatal `rp.stage(config)` call in `phase_5_system_configuration` before `configure_grub_in_chroot` + header bump |

## Testing

Follow the existing `disk_ops.rs` idiom (pure command-builder assertions, `#[cfg(test)] mod
tests`); no live executor needed. Gate: `cargo test --lib --offline` (baseline 237 passed) +
`cargo build --offline` + `cargo clippy --offline`.

| Test | Asserts |
|---|---|
| `test_mount_reset_cmd_uses_partition_path` | builder output contains the TASK-01-suffixed p2 path for `/dev/nvme0n1` (`nvme0n1p2`) AND `/dev/sda` (`sda2`, not `sdap2`); contains `mountpoint -q` guard |
| `test_iso_copy_cmd_exact_bytes` | `dd` command carries `count=<size> iflag=count_bytes conv=fsync` and targets `/mnt/reset/uaa-recovery.iso` |
| `test_grub_dropin_contents` | `GRUB_DROPIN` contains `search --no-floppy --label RESET`, `loopback loop`, `configfile /boot/grub/loopback.cfg`, and the literal title `UAA reset/recover` |
| `test_grub_dropin_remove_cmd` | remove builder is `rm -f` on `/mnt/targetos/etc/grub.d/40_uaa_reset` (no-ISO ⇒ no entry, Decision 7) |
| `test_reset_helper_gate_literal` | `RESET_HELPER_SH` contains the exact string `nuke it`, the `[ -t 0 ]` tty check, and aborts (`exit 1`) on mismatch — the anti-over-suppression case: the gate must NOT match `NUKE IT`/`nuke it!`/empty |
| `test_tarball_copy_uses_uaacache_convention` | builder embeds `/mnt/uaacache/` + `-base.tar.gz` + `dpkg --print-architecture`, matching the Phase 4 convention verbatim |

## Rollback

- M1 is dormant until M2 wires the call; reverting the single `installer.rs` hunk (or the whole
  commit via `git revert`) restores today's exact behavior: RESET created + formatted, never
  mounted/populated, no `40_uaa_reset` drop-in, no menu entry.
- On an already-installed host, rollback = delete `/etc/grub.d/40_uaa_reset` + `update-grub`
  (menu entry gone) and optionally `mkfs.ext4 -F -L RESET <p2>` (content gone). No other state
  is touched; ESP/BPOOL/LUKS layout is unchanged by this feature.

## Open questions (resolved — recorded for the plan)

1. ~~Where does the recovery ISO come from at install time?~~ → The live boot medium itself
   (`findmnt /cdrom` + `isosize` + `dd count_bytes`); no config field, skip-with-warning when
   absent (Decision 2).
2. ~~Edit `configure_grub_in_chroot` or stay out of `system_setup.rs`?~~ → Stay out: the
   drop-in is written from the new module via an `installer.rs` call placed before
   `sc.configure_grub_in_chroot(config)`, which is early enough for its `update-grub`
   (Decision 4).
3. ~~What if the ISO doesn't fit 4 GiB?~~ → df-based free-space gate; skip ISO AND menu entry
   with a warning; tarball/helper still staged if they fit (Decision 6 + 7, LOCKED).
4. ~~Hard or soft dependency on installer-robustness/TASK-01's `partition_path`?~~ → Soft:
   wave ordering (1 vs 6) guarantees it is merged first and the brief uses it; the legacy
   `{}p2` format works on the current fleet if ever needed (Decision 8).
