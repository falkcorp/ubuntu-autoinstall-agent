// file: crates/uaa-core/src/network/ssh_installer/disk_native.rs
// version: 2.0.0
// guid: caac7413-829a-44ec-9a7f-9f725b27faba
// last-edited: 2026-07-23

//! Multi-disk partitioner for [`StorageMode::NativeKeystore`] (U1 / the future
//! server profile) — the *applier* that turns [`layout::plan_layout`]'s pure
//! [`layout::PartitionPlan`] into real `sgdisk` on the target.
//!
//! Parallel to [`super::disk_ops::DiskManager`] (the single-disk PlainLuks
//! path), selected by `config.storage_mode` in the installer's Phase 2. It
//! **only** partitions: the System (bootable SATA SSD) disks get
//! `ESP + bpool + data`; the Special (Optane) disks get a single half-disk
//! `special` member (the other half is left free for a future spinning array —
//! see [`super::layout`]). Every disk is partitioned; there are no whole-disk
//! vdevs. The pool, keystore zvol, and its LUKS are Phase 3
//! ([`super::zfs_native`]) — there is no root-LUKS mapper here (unlike
//! PlainLuks, whose p4 is the encrypted root).
//!
//! Every command mirrors the sequence hand-validated on real U1 hardware:
//! `wipefs -a` + `sgdisk --zap-all` per disk, then
//! `sgdisk -n N:0:<end> -t N:<code> -c N:<label>` per partition.
//!
//! [`StorageMode::NativeKeystore`]: super::config::StorageMode::NativeKeystore

use super::config::InstallationConfig;
use super::installer::WipeAuthorization;
use super::layout::{self, PartSize, Partition};
use crate::network::CommandExecutor;
use crate::Result;
use tracing::info;

/// 1 GiB in bytes — for formatting `sgdisk` fixed sizes (`sgdisk` size suffixes
/// are binary: `G` = GiB, `M` = MiB).
const GIB: u64 = 1024 * 1024 * 1024;
/// 1 MiB in bytes.
const MIB: u64 = 1024 * 1024;

pub struct DiskNativeManager<'a> {
    runner: &'a mut dyn CommandExecutor,
}

impl<'a> DiskNativeManager<'a> {
    pub fn new(runner: &'a mut dyn CommandExecutor) -> Self {
        Self { runner }
    }

    /// Wipe and partition every disk in `config.disks` per the NativeKeystore
    /// layout. Requires a [`WipeAuthorization`] (mintable only when Phase 2 is
    /// selected) exactly like [`super::disk_ops::DiskManager::prepare_disk`] —
    /// the compiler forbids wiping without a wipe right.
    ///
    /// Idempotent-on-rerun: tears down any prior pools / keystore mapper first,
    /// so a repeated install (U1 is wiped as many times as it takes) starts
    /// clean instead of failing on an in-use device.
    pub async fn prepare_disks(
        &mut self,
        config: &InstallationConfig,
        _auth: &WipeAuthorization,
    ) -> Result<()> {
        // plan_layout validates the roster (>=2 system, >=2 data, no empty/dup
        // ids) and computes the partitions; a bad roster fails here, before any
        // destructive command runs.
        let plan = layout::plan_layout(&config.disks)
            .map_err(|e| crate::error::AutoInstallError::ConfigError(e.to_string()))?;
        info!(
            "NativeKeystore partitioning: {} disk(s), {} ESP(s)",
            plan.disks.len(),
            plan.esp_count()
        );

        self.cleanup_existing().await?;

        // Wipe then partition every target disk: System (SSD) → ESP+bpool+data,
        // Special (Optane) → the half-disk special member. No whole-disk vdevs.
        for dp in &plan.disks {
            self.wipe_disk(&dp.id).await?;
            for part in &dp.partitions {
                let cmd = Self::build_sgdisk(&dp.id, part);
                self.log_and_execute(
                    &format!("Partition {} p{} ({:?})", dp.id, part.number, part.kind),
                    &cmd,
                )
                .await?;
            }
        }

        // Re-read partition tables and wait for the by-id `-partN` symlinks the
        // pool creation (Phase 3) will reference — on every disk now, since both
        // roles carry ZFS member partitions (System p2/p3, Special p1).
        let all_ids: Vec<&str> = plan.disks.iter().map(|d| d.id.as_str()).collect();
        self.log_and_execute(
            "Re-read partition tables",
            &format!("partprobe {} 2>/dev/null || true", all_ids.join(" ")),
        )
        .await?;
        self.log_and_execute("Settle udev", "udevadm settle").await?;

        // Format each System disk's ESP as FAT32 — the base-system install mounts
        // it at /boot/efi and grub-install writes the signed shim there. (bpool /
        // data / special partitions are ZFS members and are formatted by `zpool
        // create`, so only the ESP needs an mkfs.) Two independent ESPs, one per
        // bootable SATA SSD, per the two-ESPs-in-NVRAM design.
        for (i, dp) in plan.system_disks().enumerate() {
            self.log_and_execute(
                &format!("Format ESP on {}", dp.id),
                &format!("mkfs.vfat -F32 -n ESP{} {}-part1", i + 1, dp.id),
            )
            .await?;
        }

        info!("NativeKeystore partitioning complete");
        Ok(())
    }

    /// Best-effort teardown of a prior install so a re-run starts clean. All
    /// non-fatal (`|| true`): on a fresh disk none of these exist.
    async fn cleanup_existing(&mut self) -> Result<()> {
        self.log_and_execute(
            "Cleanup: unmount target + keystore",
            "umount -R /mnt/targetos 2>/dev/null || true; \
             umount /run/keystore/rpool 2>/dev/null || true",
        )
        .await?;
        self.log_and_execute(
            "Cleanup: close keystore LUKS mapper",
            "cryptsetup close keystore-rpool 2>/dev/null || true",
        )
        .await?;
        self.log_and_execute(
            "Cleanup: export existing pools",
            "zpool export -f rpool 2>/dev/null || true; \
             zpool export -f bpool 2>/dev/null || true",
        )
        .await?;
        Ok(())
    }

    /// Zap a single disk: `wipefs -a` (clears FS/RAID signatures) then
    /// `sgdisk --zap-all` (clears the GPT). Both target the stable by-id path.
    async fn wipe_disk(&mut self, disk_id: &str) -> Result<()> {
        self.log_and_execute(
            &format!("Wipe signatures on {disk_id}"),
            &format!("wipefs -a {disk_id} 2>/dev/null || true"),
        )
        .await?;
        self.log_and_execute(
            &format!("Zap GPT on {disk_id}"),
            &format!("sgdisk --zap-all {disk_id}"),
        )
        .await?;
        Ok(())
    }

    /// Build the `sgdisk` command for one planned partition (pure — unit-tested).
    ///
    /// `-n N:0:<end>`: start `0` lets `sgdisk` pick the first 1 MiB-aligned
    /// sector; `<end>` is `+<size>` for a fixed partition or `0` (= rest of
    /// disk) for the `special` remainder. Matches the validated hand-run.
    fn build_sgdisk(disk_id: &str, part: &Partition) -> String {
        format!(
            "sgdisk -n {n}:0:{end} -t {n}:{code} -c {n}:'{label}' {disk}",
            n = part.number,
            end = Self::sgdisk_end(part.size),
            code = part.typecode,
            label = part.label,
            disk = disk_id,
        )
    }

    /// Render a [`PartSize`] as the `sgdisk` end-of-partition token.
    fn sgdisk_end(size: PartSize) -> String {
        match size {
            PartSize::Remainder => "0".to_string(),
            PartSize::Fixed(bytes) if bytes % GIB == 0 => format!("+{}G", bytes / GIB),
            PartSize::Fixed(bytes) if bytes % MIB == 0 => format!("+{}M", bytes / MIB),
            PartSize::Fixed(bytes) => format!("+{bytes}"),
        }
    }

    /// Log then execute (mirrors [`super::disk_ops::DiskManager::log_and_execute`]).
    async fn log_and_execute(&mut self, description: &str, command: &str) -> Result<()> {
        info!("Executing: {} -> {}", description, command);
        self.runner.execute(command).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::ssh_installer::config::{DiskRole, DiskSpec};
    use crate::network::ssh_installer::layout::plan_layout;

    fn u1_roster() -> Vec<DiskSpec> {
        vec![
            DiskSpec { id: "/dev/disk/by-id/ata-SSD0".into(), role: DiskRole::System },
            DiskSpec { id: "/dev/disk/by-id/ata-SSD1".into(), role: DiskRole::System },
            DiskSpec { id: "/dev/disk/by-id/nvme-OPT0".into(), role: DiskRole::Special },
            DiskSpec { id: "/dev/disk/by-id/nvme-OPT1".into(), role: DiskRole::Special },
        ]
    }

    #[test]
    fn sgdisk_end_formats_binary_units_and_remainder() {
        assert_eq!(DiskNativeManager::sgdisk_end(PartSize::Fixed(GIB)), "+1G");
        assert_eq!(DiskNativeManager::sgdisk_end(PartSize::Fixed(2 * GIB)), "+2G");
        assert_eq!(DiskNativeManager::sgdisk_end(PartSize::Fixed(512 * MIB)), "+512M");
        assert_eq!(DiskNativeManager::sgdisk_end(PartSize::Remainder), "0");
    }

    #[test]
    fn system_ssd_partition_commands_are_esp_bpool_data() {
        let plan = plan_layout(&u1_roster()).expect("valid roster");
        let ssd0 = plan.system_disks().next().expect("a system disk");
        let cmds: Vec<String> = ssd0
            .partitions
            .iter()
            .map(|p| DiskNativeManager::build_sgdisk(&ssd0.id, p))
            .collect();
        // p1 ESP 1G EF00, p2 bpool 2G BE00, p3 data rest BF00 — boot lives on the
        // bootable SATA SSD now (the firmware can't boot the Optane).
        assert!(cmds[0].contains("sgdisk -n 1:0:+1G -t 1:EF00 -c 1:'ESP1'"));
        assert!(cmds[1].contains("sgdisk -n 2:0:+2G -t 2:BE00 -c 2:'bpool-0'"));
        assert!(cmds[2].contains("sgdisk -n 3:0:0 -t 3:BF00 -c 3:'data-0'"));
        assert!(cmds[2].ends_with(&ssd0.id));
    }

    #[test]
    fn special_optane_gets_one_half_disk_partition() {
        let plan = plan_layout(&u1_roster()).expect("valid roster");
        let opt0 = plan.special_disks().next().expect("a special disk");
        assert_eq!(opt0.partitions.len(), 1, "Optane has exactly one special member");
        let cmd = DiskNativeManager::build_sgdisk(&opt0.id, &opt0.partitions[0]);
        // p1 special, fixed 6G (half the drive, NOT ':0' remainder), BF00.
        assert!(cmd.contains("sgdisk -n 1:0:+6G -t 1:BF00 -c 1:'special-0'"), "got: {cmd}");
        assert!(cmd.ends_with(&opt0.id));
    }

    #[test]
    fn esp_typecode_and_kind_line_up() {
        let plan = plan_layout(&u1_roster()).expect("valid roster");
        let p1 = &plan.system_disks().next().unwrap().partitions[0];
        assert_eq!(p1.kind, layout::PartKind::Esp);
        assert_eq!(p1.typecode, "EF00");
    }
}
