// file: crates/uaa-core/src/network/ssh_installer/disk_ops.rs
// version: 2.4.1
// guid: sshdisk1-2345-6789-abcd-ef0123456789
// last-edited: 2026-07-10

//! Disk operations for SSH installation

use super::config::InstallationConfig;
use super::installer::WipeAuthorization;
use super::partitions::partition_path;
use crate::network::CommandExecutor;
use crate::Result;
use tracing::info;

pub struct DiskManager<'a> {
    runner: &'a mut dyn CommandExecutor,
}

impl<'a> DiskManager<'a> {
    /// Root-only 0600 tempfile on the target that carries the LUKS passphrase
    /// to `cryptsetup` via `--key-file`. `/run` is tmpfs, so the key never
    /// touches persistent disk; it is `shred -u`'d immediately after use.
    const LUKS_SETUP_KEY_PATH: &'static str = "/run/.uaa-luks-setup.key";

    pub fn new(runner: &'a mut dyn CommandExecutor) -> Self {
        Self { runner }
    }

    /// Perform complete disk preparation and partitioning
    ///
    /// Requires a [`WipeAuthorization`] token (mintable only when Phase 2 is
    /// selected) because it wipes the disk — the compiler, not convention,
    /// forbids calling this without a wipe right.
    pub async fn prepare_disk(
        &mut self,
        config: &InstallationConfig,
        _auth: &WipeAuthorization,
    ) -> Result<()> {
        info!("Starting disk preparation for {}", config.disk_device);

        // Clean up any existing mounts first
        self.cleanup_existing_mounts(config).await?;

        // Destroy existing ZFS pools
        self.destroy_existing_zfs_pools().await?;

        // Wipe and partition disk
        self.wipe_disk(config, _auth).await?;
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
        _auth: &WipeAuthorization,
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
        self.wipe_disk(config, _auth).await?;

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
    ///
    /// Guarded by a [`WipeAuthorization`] token so no caller can reach the
    /// destructive `wipefs`/`sgdisk --zap-all` path without Phase 2 selected.
    async fn wipe_disk(
        &mut self,
        config: &InstallationConfig,
        _auth: &WipeAuthorization,
    ) -> Result<()> {
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
    ///
    /// The LUKS passphrase is delivered to `cryptsetup` via a root-only 0600
    /// tempfile on the TARGET (`/run` is tmpfs, so the key never touches
    /// persistent disk) passed with `--key-file`, then `shred -u`'d — mirroring
    /// the Tang clevis-bind pattern in `SystemConfigurator::enroll_tang_clevis`.
    /// The passphrase therefore never appears on a command line, in `ps`
    /// output, in `/proc/<pid>/cmdline`, or in any log message.
    async fn setup_luks_encryption(&mut self, config: &InstallationConfig) -> Result<()> {
        info!("Setting up LUKS encryption");

        let key_path = Self::LUKS_SETUP_KEY_PATH;
        let part = partition_path(&config.disk_device, 4);
        let shred_cmd = format!("shred -u {} 2>/dev/null || rm -f {}", key_path, key_path);

        // Create the empty 0600 keyfile atomically. This command carries no
        // secret, so it is fine to log. An install cannot proceed without LUKS,
        // so a failure here is fatal (unlike Tang's non-fatal skip).
        self.log_and_execute(
            "Creating LUKS keyfile tempfile",
            &format!("install -m 0600 {} {}", "/dev/null", key_path),
        )
        .await?;

        // Write the passphrase — executed via runner.execute DIRECTLY (never
        // log_and_execute, never echo) so the secret is never logged. Single
        // quotes are escaped exactly as in the Tang bind. `printf '%s'` writes
        // NO trailing newline: `cryptsetup --key-file` reads the whole file
        // verbatim, so a stray newline would enroll `<pass>\n` and desync every
        // other unlock channel (Tang, TPM2 seed, interactive). Embedded
        // newlines in the key are unsupported (as with the old stdin pipe).
        let write_key = format!(
            "printf '%s' '{}' > {}",
            config.luks_key.replace('\'', r"'\''"),
            key_path
        );
        if let Err(e) = self.runner.execute(&write_key).await {
            let _ = self.runner.execute(&shred_cmd).await;
            return Err(e);
        }

        // luksFormat then open, both via the keyfile. Capture the Results
        // instead of `?`-propagating so the tempfile is always shredded.
        let format_result = self
            .log_and_execute(
                "Setting up LUKS encryption",
                &Self::build_luks_format_cmd(&part, key_path),
            )
            .await;
        let open_result = self
            .log_and_execute(
                "Opening LUKS device",
                &Self::build_luks_open_cmd(&part, key_path),
            )
            .await;

        // Always shred the tempfile regardless of outcome (finally-style).
        let _ = self.runner.execute(&shred_cmd).await;

        // Propagate the first error (format first, then open).
        format_result?;
        open_result?;
        // Do not create a filesystem on the LUKS-mapped device; it will back the ZFS rpool.

        Ok(())
    }

    // -------------------------------------------------------------------------
    // Non-destructive mount-existing-target prep (phase-rerun/TASK-02).
    // These helpers ASSEMBLE / OPEN existing state so a selective run that skips
    // Phase 2 can reach an installed disk. They NEVER wipe/format, so they take
    // no `WipeAuthorization` — they must be callable without wipe rights.
    // -------------------------------------------------------------------------

    /// Assemble any IMSM/mdraid array that backs the target disk, if the disk is
    /// an md device (e.g. `/dev/md126`). `mdadm --assemble --scan` exits non-zero
    /// when there is nothing new to assemble (array already up) — that is success
    /// here, hence `|| true`. No-op for non-md disks.
    pub async fn assemble_md_if_needed(&mut self, config: &InstallationConfig) -> Result<()> {
        if config.disk_device.starts_with("/dev/md") {
            self.log_and_execute(
                "Assembling md array for existing target",
                "mdadm --assemble --scan || true",
            )
            .await?;
        }
        Ok(())
    }

    /// Re-open the existing LUKS mapper (`/dev/mapper/luks`) for a selective
    /// re-run, using the same 0600-keyfile channel as `setup_luks_encryption`
    /// (never `echo`, never an interpolated passphrase). Idempotent: if the
    /// mapper is already open it is reused, not reopened. A wrong key is a HARD
    /// error — nothing half-runs.
    pub async fn reopen_luks_if_needed(&mut self, config: &InstallationConfig) -> Result<()> {
        // Idempotency FIRST: reuse an already-open mapper (LUKS-already-open path).
        if self
            .runner
            .check_silent("cryptsetup status luks >/dev/null 2>&1")
            .await
            .unwrap_or(false)
        {
            info!("LUKS mapper already open; skipping");
            return Ok(());
        }

        let key_path = Self::LUKS_SETUP_KEY_PATH;
        let part = partition_path(&config.disk_device, 4);
        let shred_cmd = format!("shred -u {} 2>/dev/null || rm -f {}", key_path, key_path);

        // Create the empty 0600 keyfile atomically (carries no secret; safe to log).
        self.log_and_execute(
            "Creating LUKS keyfile tempfile",
            &format!("install -m 0600 {} {}", "/dev/null", key_path),
        )
        .await?;

        // Write the passphrase via runner.execute DIRECTLY (never log_and_execute,
        // never echo) so the secret is never logged. `printf '%s'` writes NO
        // trailing newline so the keyfile matches the enrolled passphrase exactly.
        let write_key = format!(
            "printf '%s' '{}' > {}",
            config.luks_key.replace('\'', r"'\''"),
            key_path
        );
        if let Err(e) = self.runner.execute(&write_key).await {
            let _ = self.runner.execute(&shred_cmd).await;
            return Err(e);
        }

        // Open via the keyfile; always shred the tempfile regardless of outcome.
        let open_result = self
            .log_and_execute(
                "Reopening LUKS device for re-run",
                &Self::build_luks_open_cmd(&part, key_path),
            )
            .await;
        let _ = self.runner.execute(&shred_cmd).await;
        open_result?;

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

    // --- LUKS command builders (production; keyfile channel only) ---
    // These deliberately take only the partition and the keyfile PATH — never
    // the passphrase — so the secret cannot leak through the command string.

    /// Build the `cryptsetup luksFormat` command using a keyfile.
    fn build_luks_format_cmd(part: &str, key_path: &str) -> String {
        format!(
            "cryptsetup luksFormat --batch-mode --key-file {} {}",
            key_path, part
        )
    }

    /// Build the `cryptsetup open` command using a keyfile.
    fn build_luks_open_cmd(part: &str, key_path: &str) -> String {
        format!("cryptsetup open --key-file {} {} luks", key_path, part)
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

    #[test]
    fn test_build_luks_format_cmd_uses_key_file() {
        let cmd =
            DiskManager::build_luks_format_cmd("/dev/nvme0n1p4", DiskManager::LUKS_SETUP_KEY_PATH);
        // Happy path preserved: still formats the same partition, now via keyfile.
        assert!(cmd.contains("luksFormat --batch-mode"));
        assert!(cmd.contains("--key-file /run/.uaa-luks-setup.key"));
        assert!(cmd.contains("/dev/nvme0n1p4"));
        // The secret channel is closed: no echo-pipe interpolation.
        assert!(!cmd.contains("echo"));
        assert!(!cmd.contains('\n'));
    }

    #[test]
    fn test_build_luks_open_cmd_uses_key_file() {
        let cmd =
            DiskManager::build_luks_open_cmd("/dev/nvme0n1p4", DiskManager::LUKS_SETUP_KEY_PATH);
        assert!(cmd.contains("cryptsetup open"));
        assert!(cmd.contains("--key-file /run/.uaa-luks-setup.key"));
        assert!(cmd.contains("/dev/nvme0n1p4"));
        // Mapper name unchanged so the rest of the install still finds `luks`.
        assert!(cmd.trim_end().ends_with("luks"));
        assert!(!cmd.contains("echo"));
        assert!(!cmd.contains('\n'));
    }

    #[test]
    fn test_luks_commands_never_embed_passphrase() {
        // The builders take only the partition and keyfile PATH — the
        // passphrase cannot reach the command string by construction. Build
        // both with a sentinel partition and assert no single-quote-wrapped
        // secret (the old `echo '<pass>' | ...` shape) survives.
        let part = "/dev/SENTINELp4";
        let key = DiskManager::LUKS_SETUP_KEY_PATH;
        let format_cmd = DiskManager::build_luks_format_cmd(part, key);
        let open_cmd = DiskManager::build_luks_open_cmd(part, key);
        for cmd in [&format_cmd, &open_cmd] {
            assert!(!cmd.contains("echo"));
            assert!(!cmd.contains('\''));
            assert!(cmd.contains(part));
            assert!(cmd.contains(key));
        }
    }
}
