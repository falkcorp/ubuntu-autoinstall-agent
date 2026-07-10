// file: src/network/ssh_installer/disk_ops.rs
// version: 2.2.0
// guid: sshdisk1-2345-6789-abcd-ef0123456789
// last-edited: 2026-07-10

//! Disk operations for SSH installation

use super::config::InstallationConfig;
use super::partitions::partition_path;
use crate::network::CommandExecutor;
use crate::Result;
use tracing::info;

pub struct DiskManager<'a> {
    runner: &'a mut dyn CommandExecutor,
}

impl<'a> DiskManager<'a> {
    pub fn new(runner: &'a mut dyn CommandExecutor) -> Self {
        Self { runner }
    }

    /// Perform complete disk preparation and partitioning
    pub async fn prepare_disk(&mut self, config: &InstallationConfig) -> Result<()> {
        info!("Starting disk preparation for {}", config.disk_device);

        // Clean up any existing mounts first
        self.cleanup_existing_mounts(config).await?;

        // Destroy existing ZFS pools
        self.destroy_existing_zfs_pools().await?;

        // Wipe and partition disk
        self.wipe_disk(config).await?;
        self.create_partitions(config).await?;
        self.format_partitions(config).await?;
        self.setup_luks_encryption(config).await?;

        info!("Disk preparation completed successfully");
        Ok(())
    }

    /// Perform a robust recovery cleanup and wipe in case of prior failures
    ///
    /// This will:
    /// - Unmount chroot bind mounts and anything under /mnt/targetos
    /// - Unmount /mnt/luks if mounted
    /// - Unmount ZFS filesystems and export/destroy pools (best-effort)
    /// - Close any open LUKS mapper devices
    /// - Wipe the disk GPT and FS signatures
    pub async fn recover_after_failure_and_wipe(
        &mut self,
        config: &InstallationConfig,
    ) -> Result<()> {
        info!("Recovery: cleaning up mounts, closing LUKS, exporting ZFS, and wiping disk");

        // 1) Unmount common chroot bind mounts and EFI if present
        let _ = self
            .log_and_execute(
                "Recovery: umount /mnt/targetos/sys",
                "umount -lf /mnt/targetos/sys 2>/dev/null || true",
            )
            .await;
        let _ = self
            .log_and_execute(
                "Recovery: umount /mnt/targetos/proc",
                "umount -lf /mnt/targetos/proc 2>/dev/null || true",
            )
            .await;
        let _ = self
            .log_and_execute(
                "Recovery: umount /mnt/targetos/dev",
                "umount -lf /mnt/targetos/dev 2>/dev/null || true",
            )
            .await;
        let _ = self
            .log_and_execute(
                "Recovery: umount /mnt/targetos/boot/efi",
                "umount -lf /mnt/targetos/boot/efi 2>/dev/null || true",
            )
            .await;

        // 2) Unmount anything still mounted under /mnt/targetos (deepest-first)
        let _ = self.log_and_execute(
            "Recovery: unmount all under /mnt/targetos",
            "mount | awk '$3 ~ /^\\/mnt\\/targetos/ {print $3}' | sort -r | xargs -r -n1 umount -lf 2>/dev/null || true"
        ).await;

        // 3) Unmount ZFS filesystems and export pools (best-effort)
        let _ = self
            .log_and_execute(
                "Recovery: zfs unmount -a",
                "zfs unmount -a 2>/dev/null || true",
            )
            .await;
        let _ = self
            .log_and_execute(
                "Recovery: zpool export -a",
                "zpool export -a 2>/dev/null || true",
            )
            .await;

        // As an extra measure, try to destroy common pools if they linger
        let _ = self
            .log_and_execute(
                "Recovery: destroy bpool",
                "zpool destroy bpool 2>/dev/null || true",
            )
            .await;
        let _ = self
            .log_and_execute(
                "Recovery: destroy rpool",
                "zpool destroy rpool 2>/dev/null || true",
            )
            .await;

        // 4) Unmount /mnt/luks if mounted
        let _ = self
            .log_and_execute(
                "Recovery: unmount /mnt/luks if mounted",
                "mountpoint -q /mnt/luks && umount -lf /mnt/luks || true",
            )
            .await;

        // 5) Close LUKS mapper devices
        // Try the known name first, then any crypt devices discovered under /dev/mapper
        let _ = self
            .log_and_execute(
                "Recovery: close luks",
                "cryptsetup close luks 2>/dev/null || true",
            )
            .await;
        let _ = self.log_and_execute(
            "Recovery: close any crypt mappers",
            "for m in $(ls /dev/mapper 2>/dev/null | grep -E '^(luks|crypt)' || true); do cryptsetup close \"$m\" 2>/dev/null || true; done"
        ).await;

        // 6) Finally wipe the disk and GPT
        self.wipe_disk(config).await?;

        Ok(())
    }

    /// Clean up existing mounts and filesystem structures
    async fn cleanup_existing_mounts(&mut self, config: &InstallationConfig) -> Result<()> {
        info!("Cleaning up existing mounts and filesystems");

        // Robust release of a PRIOR install so re-runs don't hit "device busy"
        // (idempotency). LAZY unmount the target tree deepest-first, REPEATED a
        // few times — a single umount loses to a transient/busy holder — then
        // unmount all ZFS, force-export every imported pool (which releases the
        // underlying disk/mapper), and close LUKS. Without this, a re-run over a
        // prior install fails at wipefs and cascades.
        let release_cmd = "for i in 1 2 3; do \
             mount | awk '$3 ~ \"/mnt/targetos\" {print $3}' | sort -r | xargs -r -n1 umount -lf 2>/dev/null || true; \
             done; \
             zfs unmount -a 2>/dev/null || true; \
             for p in $(zpool list -H -o name 2>/dev/null); do zpool export -f \"$p\" 2>/dev/null || true; done; \
             cryptsetup close luks 2>/dev/null || true";
        self.log_and_execute(
            "Robust release: lazy-unmount target, export pools, close LUKS",
            release_cmd,
        )
        .await?;

        // Unmount any existing mounts on the target disk
        let mounted_parts = self
            .runner
            .execute_with_output(&format!(
                "mount | grep '{}' | awk '{{print $1}}' || true",
                config.disk_device
            ))
            .await?;

        for mount in mounted_parts.lines() {
            if !mount.trim().is_empty() {
                self.log_and_execute(
                    &format!("Unmounting {}", mount.trim()),
                    &format!("umount -f {} || true", mount.trim()),
                )
                .await?;
            }
        }

        // Close any open LUKS devices
        self.log_and_execute("Closing LUKS devices", "cryptsetup close luks || true")
            .await?;

        // Also unmount /mnt/luks if it is mounted (best-effort)
        let _ = self
            .log_and_execute(
                "Unmount /mnt/luks if mounted",
                "mountpoint -q /mnt/luks && umount -lf /mnt/luks || true",
            )
            .await;

        Ok(())
    }

    /// Destroy existing ZFS pools
    async fn destroy_existing_zfs_pools(&mut self) -> Result<()> {
        info!("Destroying existing ZFS pools");

        let existing_pools = self
            .runner
            .execute_with_output("zpool list -H -o name 2>/dev/null || true")
            .await?;
        if !existing_pools.trim().is_empty() {
            for pool in existing_pools.lines() {
                if !pool.trim().is_empty() {
                    self.log_and_execute(
                        &format!("Destroying ZFS pool: {}", pool.trim()),
                        &format!("zpool destroy {} || true", pool.trim()),
                    )
                    .await?;
                }
            }
        }

        Ok(())
    }

    /// Wipe the target disk completely
    async fn wipe_disk(&mut self, config: &InstallationConfig) -> Result<()> {
        info!("Wiping target disk");

        self.log_and_execute(
            "Wiping disk signatures",
            &format!("wipefs -a {}", config.disk_device),
        )
        .await?;
        self.log_and_execute(
            "Discarding blocks",
            &format!("blkdiscard -f {} || true", config.disk_device),
        )
        .await?;
        self.log_and_execute(
            "Zapping GPT structures",
            &format!("sgdisk --zap-all {}", config.disk_device),
        )
        .await?;

        Ok(())
    }

    /// Create disk partitions
    async fn create_partitions(&mut self, config: &InstallationConfig) -> Result<()> {
        info!("Creating disk partitions");

        // Use sgdisk to create partitions with exact GPT type codes and names:
        // 1: EF00 (EFI System Partition) 512MiB
        // 2: 8300 (Linux filesystem) 4GiB (RESET)
        // 3: BE00 (Solaris boot) 2GiB (BPOOL)
        // 4: 8309 (Linux LUKS) remainder of disk (RPOOL via LUKS mapper)

        // Create new GPT
        self.log_and_execute(
            "Create new GPT label",
            &format!("sgdisk -o {}", config.disk_device),
        )
        .await?;

        // Partition 1: EFI System, 512MiB starting at sector 2048 (~1MiB)
        self.log_and_execute(
            "Create ESP (p1)",
            &format!(
                "sgdisk -n 1:2048:+512M -t 1:EF00 -c 1:'EFI System Partition' {}",
                config.disk_device
            ),
        )
        .await?;

        // Partition 2: RESET ext4, 4GiB
        self.log_and_execute(
            "Create RESET (p2)",
            &format!(
                "sgdisk -n 2:0:+4G -t 2:8300 -c 2:'RESET' {}",
                config.disk_device
            ),
        )
        .await?;

        // Partition 3: BPOOL, 2GiB, ZFS boot pool type
        self.log_and_execute(
            "Create BPOOL (p3)",
            &format!(
                "sgdisk -n 3:0:+2G -t 3:BE00 -c 3:'BPOOL' {}",
                config.disk_device
            ),
        )
        .await?;

        // Partition 4: LUKS, rest of disk
        self.log_and_execute(
            "Create LUKS (p4)",
            &format!(
                "sgdisk -n 4:0:0 -t 4:8309 -c 4:'LUKS' {}",
                config.disk_device
            ),
        )
        .await?;

        // Inform the kernel of partition table changes
        self.log_and_execute(
            "Reload partition table",
            &format!("partprobe {} || true", config.disk_device),
        )
        .await?;
        self.log_and_execute("Settle udev", "udevadm settle || true")
            .await?;

        Ok(())
    }

    /// Format partitions
    async fn format_partitions(&mut self, config: &InstallationConfig) -> Result<()> {
        info!("Formatting partitions");

        // Format ESP and RESET partitions
        self.log_and_execute(
            "Formatting ESP (vfat)",
            &format!(
                "mkfs.vfat -F32 -n ESP {}",
                partition_path(&config.disk_device, 1)
            ),
        )
        .await?;
        self.log_and_execute(
            "Formatting RESET (ext4)",
            &format!(
                "mkfs.ext4 -F -L RESET {}",
                partition_path(&config.disk_device, 2)
            ),
        )
        .await?;

        Ok(())
    }

    /// Setup LUKS encryption
    async fn setup_luks_encryption(&mut self, config: &InstallationConfig) -> Result<()> {
        info!("Setting up LUKS encryption");

        // Setup LUKS encryption
        self.log_and_execute(
            "Setting up LUKS encryption",
            &format!(
                "echo '{}' | cryptsetup luksFormat --batch-mode {}",
                config.luks_key,
                partition_path(&config.disk_device, 4)
            ),
        )
        .await?;
        self.log_and_execute(
            "Opening LUKS device",
            &format!(
                "echo '{}' | cryptsetup open {} luks",
                config.luks_key,
                partition_path(&config.disk_device, 4)
            ),
        )
        .await?;
        // Do not create a filesystem on the LUKS-mapped device; it will back the ZFS rpool.

        Ok(())
    }

    /// Helper method to log and execute commands
    async fn log_and_execute(&mut self, description: &str, command: &str) -> Result<()> {
        info!("Executing: {} -> {}", description, command);
        self.runner.execute(command).await
    }

    // --- Test helpers (pure builders) ---
    #[cfg(test)]
    fn build_sgdisk_esp(disk: &str) -> String {
        format!(
            "sgdisk -n 1:2048:+512M -t 1:EF00 -c 1:'EFI System Partition' {}",
            disk
        )
    }

    #[cfg(test)]
    fn build_sgdisk_reset(disk: &str) -> String {
        format!("sgdisk -n 2:0:+4G -t 2:8300 -c 2:'RESET' {}", disk)
    }

    #[cfg(test)]
    fn build_sgdisk_bpool(disk: &str) -> String {
        format!("sgdisk -n 3:0:+2G -t 3:BE00 -c 3:'BPOOL' {}", disk)
    }

    #[cfg(test)]
    fn build_sgdisk_luks(disk: &str) -> String {
        format!("sgdisk -n 4:0:0 -t 4:8309 -c 4:'LUKS' {}", disk)
    }

    #[cfg(test)]
    fn build_mkfs_esp(disk: &str) -> String {
        format!("mkfs.vfat -F32 -n ESP {}", partition_path(disk, 1))
    }

    #[cfg(test)]
    fn build_mkfs_reset(disk: &str) -> String {
        format!("mkfs.ext4 -F -L RESET {}", partition_path(disk, 2))
    }
}

#[cfg(test)]
mod tests {
    use super::DiskManager;

    #[test]
    fn test_sgdisk_partition_commands() {
        assert!(DiskManager::build_sgdisk_esp("/dev/sda").contains("-t 1:EF00"));
        assert!(DiskManager::build_sgdisk_reset("/dev/sda").contains("-t 2:8300"));
        assert!(DiskManager::build_sgdisk_bpool("/dev/sda").contains("-t 3:BE00"));
        assert!(DiskManager::build_sgdisk_luks("/dev/sda").contains("-t 4:8309"));
    }

    #[test]
    fn test_format_commands() {
        assert_eq!(
            DiskManager::build_mkfs_esp("/dev/nvme0n1"),
            "mkfs.vfat -F32 -n ESP /dev/nvme0n1p1"
        );
        assert_eq!(
            DiskManager::build_mkfs_reset("/dev/nvme0n1"),
            "mkfs.ext4 -F -L RESET /dev/nvme0n1p2"
        );
    }
}
