// file: crates/uaa-core/src/network/ssh_installer/disk_native.rs
// version: 1.0.0
// guid: caac7413-829a-44ec-9a7f-9f725b27faba
// last-edited: 2026-07-22

//! Multi-disk partitioner for [`StorageMode::NativeKeystore`] (U1 / the future
//! server profile) — the *applier* that turns [`layout::plan_layout`]'s pure
//! [`layout::PartitionPlan`] into real `sgdisk` on the target.
//!
//! Parallel to [`super::disk_ops::DiskManager`] (the single-disk PlainLuks
//! path), selected by `config.storage_mode` in the installer's Phase 2. It
//! **only** partitions: the System (Optane) disks get `ESP + bpool + special`;
//! the Data (SSD) disks are left whole-disk (consumed raw by `rpool`). The pool,
//! keystore zvol, and its LUKS are Phase 3 ([`super::zfs_native`]) — there is no
//! root-LUKS mapper here (unlike PlainLuks, whose p4 is the encrypted root).
//!
//! Every command mirrors the sequence hand-validated on the real U1 hardware
//! (2026-07-22): `wipefs -a` + `sgdisk --zap-all` per disk, then
//! `sgdisk -n N:0:<end> -t N:<code> -c N:<label>` per partition.
//!
//! [`StorageMode::NativeKeystore`]: super::config::StorageMode::NativeKeystore

use super::config::{DiskRole, InstallationConfig};
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

        // Wipe every target disk (System + Data), then lay down partitions only
        // on the System disks; Data disks stay whole-disk for the rpool mirror.
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
            if dp.role == DiskRole::Data {
                info!("{} left whole-disk (rpool data member)", dp.id);
            }
        }

        // Re-read partition tables and wait for the by-id `-partN` symlinks the
        // pool creation (Phase 3) will reference.
        let system_ids: Vec<&str> = plan
            .system_disks()
            .map(|d| d.id.as_str())
            .collect();
        self.log_and_execute(
            "Re-read partition tables",
            &format!("partprobe {} 2>/dev/null || true", system_ids.join(" ")),
        )
        .await?;
        self.log_and_execute("Settle udev", "udevadm settle").await?;

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
            DiskSpec { id: "/dev/disk/by-id/nvme-OPT0".into(), role: DiskRole::System },
            DiskSpec { id: "/dev/disk/by-id/nvme-OPT1".into(), role: DiskRole::System },
            DiskSpec { id: "/dev/disk/by-id/ata-SSD0".into(), role: DiskRole::Data },
            DiskSpec { id: "/dev/disk/by-id/ata-SSD1".into(), role: DiskRole::Data },
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
    fn optane_partition_commands_match_validated_hand_run() {
        let plan = plan_layout(&u1_roster()).expect("valid roster");
        let opt0 = plan.system_disks().next().expect("a system disk");
        let cmds: Vec<String> = opt0
            .partitions
            .iter()
            .map(|p| DiskNativeManager::build_sgdisk(&opt0.id, p))
            .collect();
        // p1 ESP 1G EF00, p2 bpool 2G BE00, p3 special rest BF00 — exactly the
        // sequence proven on real U1 hardware.
        assert!(cmds[0].contains("sgdisk -n 1:0:+1G -t 1:EF00 -c 1:'ESP1'"));
        assert!(cmds[1].contains("sgdisk -n 2:0:+2G -t 2:BE00 -c 2:'bpool-0'"));
        assert!(cmds[2].contains("sgdisk -n 3:0:0 -t 3:BF00 -c 3:'special-0'"));
        assert!(cmds[2].ends_with(&opt0.id));
    }

    #[test]
    fn data_disks_have_no_partition_commands() {
        let plan = plan_layout(&u1_roster()).expect("valid roster");
        for d in plan.data_disks() {
            assert!(d.partitions.is_empty(), "SSD {} is whole-disk", d.id);
        }
    }

    #[test]
    fn esp_typecode_and_kind_line_up() {
        let plan = plan_layout(&u1_roster()).expect("valid roster");
        let p1 = &plan.system_disks().next().unwrap().partitions[0];
        assert_eq!(p1.kind, layout::PartKind::Esp);
        assert_eq!(p1.typecode, "EF00");
    }
}
