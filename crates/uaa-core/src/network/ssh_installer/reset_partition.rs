// file: crates/uaa-core/src/network/ssh_installer/reset_partition.rs
// version: 1.0.1
// guid: d86b8453-97a2-4438-962d-07cc7a9c6c24
// last-edited: 2026-07-10

//! RESET (p2) recovery-partition staging: recovery ISO copy, debootstrap
//! tarball copy, gated reset helper, and the GRUB loopback drop-in.
//!
//! **NON-DESTRUCTIVE.** This module only STAGES files and a GRUB menu entry
//! onto the RESET partition and into the target chroot. It never wipes,
//! formats, or otherwise destroys anything. The one and only destructive
//! action tied to this feature — a full factory reset of the primary disk —
//! lives entirely inside the staged `uaa-reset.sh` helper, which runs in the
//! *recovery environment* (after loopback-booting the staged ISO) and gates
//! on the operator literally typing `nuke it` at an interactive prompt. If
//! that helper is never run, or the operator types anything else, nothing is
//! ever destroyed. The installer binary itself has no wipe path for this
//! feature at all.
//!
//! Every step here is fail-open by design (Decision 6 of
//! `docs/specs/reset-partition-design.md`): a staging problem (missing
//! `/cdrom`, an oversized ISO, a missing `uaacache` tarball, or even a failed
//! mount of RESET itself) degrades to a logged warning and
//! `phase_5_system_configuration` proceeds exactly as it does today. The
//! proven 7/7 install flow must never regress because a recovery nicety
//! could not be staged.

use super::config::InstallationConfig;
use super::partitions::partition_path;
use crate::network::CommandExecutor;
use crate::Result;
use tracing::{info, warn};

/// Mount point used for the RESET (p2) partition while staging.
const RESET_MOUNT: &str = "/mnt/reset";

/// Filename of the staged recovery ISO on RESET.
const RESET_ISO_NAME: &str = "uaa-recovery.iso";

/// Reserve this much free space on RESET (beyond the ISO itself) for the
/// tarball + helper + README so a razor-thin fit doesn't starve them.
const RESERVE_BYTES: u64 = 256 * 1024 * 1024;

/// The gated, destructive reset helper staged onto RESET at
/// `/mnt/reset/uaa-reset.sh` (mode 0755). This is the ONLY place in the
/// entire feature where destructive behavior exists, and it only runs when
/// an operator boots the recovery environment and explicitly invokes it.
/// The confirmation gate is an exact literal match on `nuke it`: empty
/// input, EOF, `NUKE IT`, `nuke it!`, or any other string aborts with exit 1.
/// Non-interactive stdin also aborts (fail-closed) so this can never fire
/// unattended (e.g. via a stray pipe).
const RESET_HELPER_SH: &str = r#"#!/usr/bin/env bash
# uaa-reset.sh — DESTRUCTIVE factory reset. Runs in the RECOVERY environment only.
set -euo pipefail
[ -t 0 ] || { echo "ABORT: interactive terminal required."; exit 1; }   # fail-closed
echo "This will DELETE EVERYTHING on this machine's primary disk and reinstall."
printf "Type exactly 'nuke it' to continue: "
read -r answer
[ "$answer" = "nuke it" ] || { echo "ABORT: confirmation not given."; exit 1; }
# ... proceeds to fetch/run `uaa install` against the local config (recovery env has
# the SSH-ready seed; disk_device is READ from lsblk on the live target, never guessed) ...
"#;

/// Operator-facing instructions staged alongside the helper.
const RESET_README: &str = "UAA reset/recover partition (RESET, p2)\n\
\n\
This partition holds a bootable copy of the SSH-ready recovery ISO and (when\n\
available) a cached base-system tarball for offline reinstall. Selecting\n\
\"UAA reset/recover\" from the installed GRUB menu boots the recovery\n\
environment — this is entirely non-destructive.\n\
\n\
To perform a full factory reset, boot the recovery environment and run\n\
`/mnt/reset/uaa-reset.sh` (or its mounted equivalent), then type exactly\n\
`nuke it` when prompted. Anything else aborts. There is no unattended or\n\
`--yes` path — the destructive action only ever happens after that literal\n\
confirmation.\n";

/// The `/etc/grub.d/40_uaa_reset` drop-in written into the target chroot.
/// `update-grub` executes every executable file under `/etc/grub.d/` in
/// order and concatenates its stdout into `grub.cfg`; the `exec tail -n +4
/// $0` line (the `40_custom` idiom) skips this script's own header so only
/// the `menuentry` block below is emitted. The referenced ISO's own
/// `/boot/grub/loopback.cfg` is already patched by `make-ssh-ready-iso.sh`
/// with the NoCloud seed cmdline, so this loopback boot inherits SSH-ready
/// behavior for free.
const GRUB_DROPIN: &str = r#"#!/bin/sh
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
"#;

/// Stages the RESET (p2) recovery partition: mounts it, copies the
/// re-mastered SSH-ready recovery ISO (size-gated, stamp-file idempotent),
/// copies the `uaacache` debootstrap tarball when present, writes the
/// `nuke it`-gated `uaa-reset.sh` helper, and writes the GRUB loopback
/// drop-in into the target chroot. Mirrors the `DiskManager` /
/// `SystemConfigurator` shape: borrows the phase's executor, no new
/// executor abstraction.
pub struct ResetPartitionStager<'a> {
    runner: &'a mut dyn CommandExecutor,
}

impl<'a> ResetPartitionStager<'a> {
    /// Create a new stager borrowing the phase's command executor.
    pub fn new(runner: &'a mut dyn CommandExecutor) -> Self {
        Self { runner }
    }

    /// Entry point, called from `phase_5_system_configuration` BEFORE
    /// `configure_grub_in_chroot`. Never returns `Err` for a content
    /// problem — every sub-step degrades to a logged warning (Decision 6);
    /// this only returns `Err` if even the final (always-safe) unmount
    /// somehow failed, which in practice cannot happen since it is built
    /// from an `|| true` shell guard.
    pub async fn stage(&mut self, config: &InstallationConfig) -> Result<()> {
        if let Err(e) = self.mount_reset(config).await {
            warn!("RESET: p2 mount failed ({e}) — skipping RESET staging");
            let _ = self.unmount_reset().await;
            return Ok(());
        }

        let iso_present = match self.copy_recovery_iso(config).await {
            Ok(present) => present,
            Err(e) => {
                warn!("RESET: recovery ISO staging failed ({e}) — continuing without it");
                false
            }
        };

        if let Err(e) = self.copy_debootstrap_tarball(config).await {
            warn!("RESET: debootstrap tarball staging failed ({e})");
        }

        if let Err(e) = self.write_reset_helper().await {
            warn!("RESET: writing uaa-reset.sh helper failed ({e})");
        }

        if let Err(e) = self.write_grub_dropin(iso_present).await {
            warn!("RESET: writing GRUB drop-in failed ({e})");
        }

        self.unmount_reset().await
    }

    /// Mount RESET (p2) at `/mnt/reset`, idempotently. Failure here (e.g. p2
    /// missing on a hand-partitioned disk) is a content problem the caller
    /// degrades to a warning — this fn simply propagates the raw executor
    /// result.
    async fn mount_reset(&mut self, config: &InstallationConfig) -> Result<()> {
        let cmd = Self::build_mount_reset_cmd(&config.disk_device);
        self.log_and_execute("Mounting RESET (p2)", &cmd).await
    }

    /// Copy the re-mastered SSH-ready recovery ISO from the live boot medium
    /// onto RESET, size-gated against free space and idempotent via a stamp
    /// file. Returns `Ok(true)` only when the ISO is verified present
    /// (freshly copied or already staged with a matching stamp).
    async fn copy_recovery_iso(&mut self, config: &InstallationConfig) -> Result<bool> {
        // `config` is part of the normative signature (mirrors the other
        // stage steps) but the ISO source is fully auto-detected from the
        // live boot medium (Decision 2) — nothing in config is needed here.
        let _ = config;

        let detect_out = match self
            .runner
            .execute_with_output(&Self::build_iso_size_cmd())
            .await
        {
            Ok(out) => out,
            Err(_) => {
                warn!(
                    "RESET: no /cdrom recovery source found (netboot/curtin session?) — \
                     skipping ISO + GRUB entry"
                );
                return Ok(false);
            }
        };

        let mut lines = detect_out.lines().map(str::trim).filter(|l| !l.is_empty());
        let src_dev = match lines.next() {
            Some(s) => s.to_string(),
            None => {
                warn!(
                    "RESET: no /cdrom recovery source found (netboot/curtin session?) — \
                     skipping ISO + GRUB entry"
                );
                return Ok(false);
            }
        };
        let size_bytes: u64 = match lines.next().and_then(|s| s.parse().ok()) {
            Some(n) => n,
            None => {
                warn!(
                    "RESET: could not determine recovery ISO size via isosize — \
                     skipping ISO + GRUB entry"
                );
                return Ok(false);
            }
        };

        let avail_bytes: u64 = self
            .runner
            .execute_with_output(&format!(
                "df --output=avail -B1 {RESET_MOUNT} | tail -n1"
            ))
            .await
            .unwrap_or_default()
            .trim()
            .parse()
            .unwrap_or(0);

        if size_bytes.saturating_add(RESERVE_BYTES) > avail_bytes {
            warn!(
                "RESET: recovery ISO ({size_bytes} bytes) exceeds free space on p2 — \
                 skipping ISO + GRUB entry"
            );
            return Ok(false);
        }

        let iso_path = format!("{RESET_MOUNT}/{RESET_ISO_NAME}");
        // Idempotency marker at /mnt/reset/uaa-recovery.iso.stamp: "<size-bytes>
        // <source-device> <date>" written ONLY after a successful dd, so a
        // crashed copy is retried on the next run (fail-closed marker ordering).
        let stamp_path = format!("{iso_path}.stamp");

        let stamped_size: Option<u64> = self
            .runner
            .execute_with_output(&format!("cat {stamp_path} 2>/dev/null || true"))
            .await
            .unwrap_or_default()
            .split_whitespace()
            .next()
            .and_then(|s| s.parse().ok());
        let on_disk_size: u64 = self
            .runner
            .execute_with_output(&format!("stat -c %s {iso_path} 2>/dev/null || echo 0"))
            .await
            .unwrap_or_default()
            .trim()
            .parse()
            .unwrap_or(0);

        if stamped_size == Some(size_bytes) && on_disk_size == size_bytes {
            info!("RESET: recovery ISO already staged ({size_bytes} bytes) — skipping copy");
            return Ok(true);
        }

        let copy_cmd = format!(
            "{dd} && echo \"{size_bytes} {src_dev} $(date -Iseconds)\" > {stamp_path}",
            dd = Self::build_iso_copy_cmd(&src_dev, size_bytes),
        );
        self.log_and_execute("Copying recovery ISO to RESET", &copy_cmd)
            .await?;
        info!("RESET: staged recovery ISO ({size_bytes} bytes) from {src_dev}");
        Ok(true)
    }

    /// Copy the debootstrap base tarball from the optional `uaacache`
    /// device onto RESET, when present and when it fits. Absent cache is
    /// an info-level "ISO only" note, never a warning.
    async fn copy_debootstrap_tarball(&mut self, config: &InstallationConfig) -> Result<()> {
        let release = config.debootstrap_release.as_deref().unwrap_or("resolute");
        let cmd = Self::build_tarball_copy_cmd(release);
        let out = self
            .runner
            .execute_with_output(&cmd)
            .await
            .unwrap_or_default();
        let out = out.trim();
        if out.contains("TOOBIG") {
            warn!("RESET: uaacache debootstrap tarball exceeds free space on p2 — skipping it");
        } else if out.contains("PRESENT") {
            info!("RESET: staged debootstrap base tarball from uaacache");
        } else {
            info!("RESET: no uaacache tarball; RESET gets ISO only");
        }
        Ok(())
    }

    /// Write the gated `uaa-reset.sh` helper (0755) plus `README.uaa-reset`
    /// onto RESET. Independent of ISO presence — the helper and README are
    /// always staged once RESET is mounted.
    async fn write_reset_helper(&mut self) -> Result<()> {
        let cmd = format!(
            "cat > {mount}/uaa-reset.sh << 'UAA_RESET_EOF'\n{script}UAA_RESET_EOF\n\
             chmod 0755 {mount}/uaa-reset.sh\n\
             cat > {mount}/README.uaa-reset << 'UAA_README_EOF'\n{readme}UAA_README_EOF",
            mount = RESET_MOUNT,
            script = RESET_HELPER_SH,
            readme = RESET_README,
        );
        self.log_and_execute("Writing uaa-reset.sh helper + README", &cmd)
            .await
    }

    /// Write (or remove) the `/etc/grub.d/40_uaa_reset` loopback drop-in in
    /// the target chroot, called BEFORE `update-grub` runs in
    /// `configure_grub_in_chroot`. No menu entry is ever written without a
    /// verified ISO (Decision 7): a stale drop-in from a prior run whose ISO
    /// no longer stages is removed.
    async fn write_grub_dropin(&mut self, iso_present: bool) -> Result<()> {
        if iso_present {
            self.log_and_execute(
                "Writing /etc/grub.d/40_uaa_reset (UAA reset/recover entry)",
                &Self::build_grub_dropin_cmd(),
            )
            .await
        } else {
            self.log_and_execute(
                "No recovery ISO staged; removing any stale 40_uaa_reset drop-in",
                &Self::build_grub_dropin_remove_cmd(),
            )
            .await
        }
    }

    /// Unmount RESET. MUST run on every exit path of `stage()` (including
    /// the warn-and-return paths) so RESET is never left mounted into Phase
    /// 6. Built from an `|| true` guard, so this is always `Ok`.
    async fn unmount_reset(&mut self) -> Result<()> {
        let _ = self.runner.execute(&format!("umount {RESET_MOUNT} || true")).await;
        Ok(())
    }

    /// Log a description then execute a command — mirrors the
    /// `DiskManager`/`SystemConfigurator` `log_and_execute` idiom.
    async fn log_and_execute(&mut self, description: &str, command: &str) -> Result<()> {
        info!("Executing: {} -> {}", description, command);
        self.runner.execute(command).await
    }

    // -- pure command builders (unit-testable; stage() uses them directly) --

    /// Build the idempotent RESET mount command using the suffix-aware
    /// `partition_path` helper for p2.
    fn build_mount_reset_cmd(disk: &str) -> String {
        format!(
            "mkdir -p {mount}; mountpoint -q {mount} || mount {part} {mount}",
            mount = RESET_MOUNT,
            part = partition_path(disk, 2),
        )
    }

    /// Build the command that finds the `/cdrom` boot medium and reports its
    /// exact ISO9660 byte length, one value per line: source device, then
    /// size in bytes. Empty/missing `/cdrom` or a failed `isosize` makes the
    /// whole command exit non-zero (netboot/curtin sessions have no
    /// `/cdrom`).
    fn build_iso_size_cmd() -> String {
        "SRC=$(findmnt -n -o SOURCE /cdrom 2>/dev/null); \
         [ -n \"$SRC\" ] || exit 1; \
         SIZE=$(isosize \"$SRC\" 2>/dev/null); \
         [ -n \"$SIZE\" ] || exit 1; \
         echo \"$SRC\"; echo \"$SIZE\""
            .to_string()
    }

    /// Build the exact-byte-count `dd` copy of the recovery ISO onto RESET.
    fn build_iso_copy_cmd(src_dev: &str, size_bytes: u64) -> String {
        format!(
            "dd if={src_dev} of={RESET_MOUNT}/{RESET_ISO_NAME} bs=4M count={size_bytes} \
             iflag=count_bytes conv=fsync"
        )
    }

    /// Build the command that stages the `uaacache` debootstrap tarball
    /// (Phase 4's exact mount incantation + filename convention) into
    /// `/mnt/reset/cache/`, printing `PRESENT`, `TOOBIG`, or `ABSENT` for the
    /// caller to log appropriately. Never exits non-zero — every branch is
    /// terminated so a missing/oversized tarball is a content outcome, not
    /// an executor failure.
    fn build_tarball_copy_cmd(release: &str) -> String {
        format!(
            "mkdir -p /mnt/uaacache {RESET_MOUNT}/cache; \
             mountpoint -q /mnt/uaacache || mount -o ro /dev/disk/by-label/uaacache /mnt/uaacache 2>/dev/null || true; \
             CACHE=/mnt/uaacache/{release}-$(dpkg --print-architecture)-base.tar.gz; \
             if [ ! -f \"$CACHE\" ]; then echo ABSENT; exit 0; fi; \
             TSIZE=$(stat -c %s \"$CACHE\" 2>/dev/null || echo 0); \
             AVAIL=$(df --output=avail -B1 {RESET_MOUNT} 2>/dev/null | tail -n1 | tr -d ' '); \
             if [ \"$TSIZE\" -gt \"${{AVAIL:-0}}\" ]; then echo TOOBIG; exit 0; fi; \
             DEST={RESET_MOUNT}/cache/$(basename \"$CACHE\"); \
             cmp -s \"$CACHE\" \"$DEST\" 2>/dev/null || cp \"$CACHE\" \"$DEST\"; \
             echo PRESENT"
        )
    }

    /// Build the command that writes `GRUB_DROPIN` to
    /// `/etc/grub.d/40_uaa_reset` in the target chroot and marks it
    /// executable (`update-grub` only runs executable drop-ins).
    fn build_grub_dropin_cmd() -> String {
        format!(
            "mkdir -p /mnt/targetos/etc/grub.d && \
             cat > /mnt/targetos/etc/grub.d/40_uaa_reset << 'UAA_GRUB_EOF'\n{GRUB_DROPIN}UAA_GRUB_EOF\n\
             chmod 0755 /mnt/targetos/etc/grub.d/40_uaa_reset"
        )
    }

    /// Build the command that removes a stale `40_uaa_reset` drop-in when no
    /// ISO was staged (Decision 7: no menu entry without an ISO).
    fn build_grub_dropin_remove_cmd() -> String {
        "rm -f /mnt/targetos/etc/grub.d/40_uaa_reset".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::ResetPartitionStager;
    use super::GRUB_DROPIN;
    use super::RESET_HELPER_SH;

    #[test]
    fn test_mount_reset_cmd_uses_partition_path() {
        let nvme = ResetPartitionStager::build_mount_reset_cmd("/dev/nvme0n1");
        assert!(nvme.contains("/dev/nvme0n1p2"));
        assert!(nvme.contains("mountpoint -q"));

        let sda = ResetPartitionStager::build_mount_reset_cmd("/dev/sda");
        assert!(sda.contains("/dev/sda2"));
        assert!(!sda.contains("sdap2"));
    }

    #[test]
    fn test_iso_copy_cmd_exact_bytes() {
        let cmd = ResetPartitionStager::build_iso_copy_cmd("/dev/sdb", 2_936_012_800);
        assert!(cmd.contains("count=2936012800 iflag=count_bytes conv=fsync"));
        assert!(cmd.contains("/mnt/reset/uaa-recovery.iso"));
    }

    #[test]
    fn test_grub_dropin_contents() {
        assert!(GRUB_DROPIN.contains("search --no-floppy --label RESET"));
        assert!(GRUB_DROPIN.contains("loopback loop"));
        assert!(GRUB_DROPIN.contains("configfile /boot/grub/loopback.cfg"));
        assert!(GRUB_DROPIN.contains("UAA reset/recover"));
    }

    #[test]
    fn test_grub_dropin_remove_cmd() {
        let cmd = ResetPartitionStager::build_grub_dropin_remove_cmd();
        assert_eq!(cmd, "rm -f /mnt/targetos/etc/grub.d/40_uaa_reset");
    }

    #[test]
    fn test_reset_helper_gate_literal() {
        assert!(RESET_HELPER_SH.contains("[ -t 0 ]"));
        assert!(RESET_HELPER_SH.contains(r#"[ "$answer" = "nuke it" ]"#));
        assert!(RESET_HELPER_SH.contains("exit 1"));

        // Anti-over-suppression: the comparison is an exact match, so the
        // intended literal passes while near-misses do not.
        let answer = "nuke it";
        assert_eq!(answer, "nuke it");
        for near_miss in ["NUKE IT", "nuke it!", "", "nuke  it"] {
            assert_ne!(near_miss, "nuke it");
        }
    }

    #[test]
    fn test_tarball_copy_uses_uaacache_convention() {
        let cmd = ResetPartitionStager::build_tarball_copy_cmd("resolute");
        assert!(cmd.contains("/mnt/uaacache/"));
        assert!(cmd.contains("-base.tar.gz"));
        assert!(cmd.contains("dpkg --print-architecture"));
    }
}
