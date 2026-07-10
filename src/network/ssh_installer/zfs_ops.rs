// file: src/network/ssh_installer/zfs_ops.rs
// version: 2.5.0
// guid: sshzfs01-2345-6789-abcd-ef0123456789
// last-edited: 2026-07-10

//! ZFS operations for SSH installation

use super::config::InstallationConfig;
use super::partitions::partition_path;
use crate::network::CommandExecutor;
use crate::Result;
use std::collections::HashMap;
use tracing::{error, info};

pub struct ZfsManager<'a> {
    runner: &'a mut dyn CommandExecutor,
    variables: &'a mut HashMap<String, String>,
}

impl<'a> ZfsManager<'a> {
    pub fn new(runner: &'a mut dyn CommandExecutor, variables: &'a mut HashMap<String, String>) -> Self {
        Self { runner, variables }
    }

    /// Create ZFS pools and datasets
    pub async fn create_zfs_pools(&mut self, config: &InstallationConfig) -> Result<()> {
        info!("Creating ZFS pools and datasets");

        // Ensure target root exists; rpool datasets will mount here
        self.log_and_execute("Creating target directory", "mkdir -p /mnt/targetos")
            .await?;

        // Generate UUID for dataset naming
        let uuid = self.generate_installation_uuid().await?;
        self.variables.insert("UUID".to_string(), uuid.clone());

        // Create bpool if not present
        if !self
            .runner
            .check_silent("zpool list -H bpool >/dev/null 2>&1")
            .await
            .unwrap_or(false)
        {
            self.create_bpool(config).await?;
        } else {
            info!("bpool already exists; skipping pool creation");
        }

        // Create rpool with encryption if not present
        if !self
            .runner
            .check_silent("zpool list -H rpool >/dev/null 2>&1")
            .await
            .unwrap_or(false)
        {
            self.create_rpool(config).await?;
        } else {
            info!("rpool already exists; skipping pool creation");
        }

        // ORDER IS LOAD-BEARING: create the rpool ROOT datasets (mount `/`)
        // BEFORE the bpool BOOT datasets (mount `/boot`). Datasets auto-mount at
        // creation under the `-R /mnt/targetos` altroot, so if bpool `/boot` is
        // mounted first, the subsequent rpool `/` mount lands ON TOP and SHADOWS
        // it — then grub-probe resolves `/boot` to the rpool/LUKS device and
        // grub-install dies "unknown filesystem". Mounting `/` first, then
        // `/boot` on top (the OpenZFS HOWTO order), makes grub-probe resolve
        // `/boot` to the bpool vdev. This was THE grub-install blocker.
        if !self
            .runner
            .check_silent(&format!(
                "zfs list -H rpool/ROOT/ubuntu_{} >/dev/null 2>&1",
                uuid
            ))
            .await
            .unwrap_or(false)
        {
            self.create_rpool_datasets(&uuid).await?;
        } else {
            info!("rpool datasets already present; skipping dataset creation");
        }

        // bpool datasets AFTER rpool root, so /boot mounts on top of / (not shadowed).
        if !self
            .runner
            .check_silent("zfs list -H bpool/BOOT >/dev/null 2>&1")
            .await
            .unwrap_or(false)
        {
            self.create_bpool_datasets(&uuid).await?;
        } else {
            info!("bpool datasets already present; skipping dataset creation");
        }

        info!("ZFS pools and datasets created successfully");
        Ok(())
    }

    /// Verify ZFS state after creation
    pub async fn verify_zfs_state(&mut self) -> Result<()> {
        info!("Verifying ZFS state");
        self.log_and_execute("Check zpool status", "zpool status")
            .await?;
        self.log_and_execute("List ZFS datasets", "zfs list")
            .await?;
        Ok(())
    }

    // -------------------------------------------------------------------------
    // Non-destructive mount-existing-target prep (phase-rerun/TASK-02) — ALL new
    // code. There is no `zpool import` anywhere else in src/: pools are only ever
    // CREATED fresh in Phase 3. These helpers IMPORT + MOUNT an existing install
    // so a selective run that skips Phases 2-3 can reach the target. They never
    // create, destroy, wipe, or export anything.
    // -------------------------------------------------------------------------

    /// Import the existing `rpool` THEN `bpool` (in that order) for a selective
    /// re-run. Each pool is skipped if already imported (reusing preflight's
    /// exact `zpool list -H <pool>` check so mocks and preflight agree). Imports
    /// use `-N` (defer ALL mounting to [`mount_target_for_rerun`], keeping the
    /// load-bearing mount order ours) and `-R /mnt/targetos` (altroot). A missing
    /// pool is a HARD error — fail-closed before any phase runs.
    pub async fn import_pools_for_rerun(&mut self) -> Result<()> {
        for pool in ["rpool", "bpool"] {
            if self
                .runner
                .check_silent(&format!("zpool list -H {} >/dev/null 2>&1", pool))
                .await
                .unwrap_or(false)
            {
                info!("{} already imported; skipping import", pool);
                continue;
            }
            self.log_and_execute(
                &format!("Importing {} (altroot, no automount)", pool),
                &format!("zpool import -N -R /mnt/targetos {}", pool),
            )
            .await?;
        }
        Ok(())
    }

    /// Mount an imported target in the LOAD-BEARING order: `/` (rpool ROOT)
    /// BEFORE `/boot` (bpool BOOT) BEFORE the ESP. Reversed order shadows `/boot`
    /// and breaks grub-probe (root cause fixed by faea48e; see the create-order
    /// comment on `create_zfs_pools`). Every mount is `mountpoint -q … ||`-guarded
    /// so partial-mount state re-runs idempotently.
    pub async fn mount_target_for_rerun(&mut self, esp_partition: &str) -> Result<()> {
        // 1) Discover the root dataset (rpool/ROOT/ubuntu_<uuid>).
        let root_dataset = self
            .runner
            .execute_with_output(
                "zfs list -H -o name -r rpool/ROOT | grep -m1 '^rpool/ROOT/ubuntu_'",
            )
            .await?;
        let root_dataset = root_dataset.trim();
        if root_dataset.is_empty() {
            return Err(crate::error::AutoInstallError::InstallationError(
                "no rpool/ROOT/ubuntu_* dataset found — is this an installed target?".to_string(),
            ));
        }

        // 2) Mount `/` FIRST.
        self.log_and_execute(
            "Mount root dataset (/) first",
            &format!("mountpoint -q /mnt/targetos || zfs mount {}", root_dataset),
        )
        .await?;

        // 3) THEN `/boot` (derive bpool/BOOT/ubuntu_<uuid> from the same uuid
        //    suffix; use the TRIMMED last path component so no stray newline
        //    leaks into the dataset name on real targets).
        let uuid_suffix = root_dataset.rsplit('/').next().unwrap_or(root_dataset);
        let boot_dataset = format!("bpool/BOOT/{}", uuid_suffix);
        self.log_and_execute(
            "Mount boot dataset (/boot) after root",
            &format!(
                "mountpoint -q /mnt/targetos/boot || zfs mount {}",
                boot_dataset
            ),
        )
        .await?;

        // 4) THEN remaining child datasets (nest under the mounted `/`, cannot
        //    shadow `/boot`).
        self.log_and_execute("Mount remaining child datasets", "zfs mount -a || true")
            .await?;

        // 5) THEN the ESP, LAST.
        self.log_and_execute(
            "Mount ESP last",
            &format!(
                "mkdir -p /mnt/targetos/boot/efi; mountpoint -q /mnt/targetos/boot/efi || mount {} /mnt/targetos/boot/efi",
                esp_partition
            ),
        )
        .await?;

        Ok(())
    }

    // Removed: prepare_zfs_key_storage - no file-based key, using passphrase-opened LUKS for rpool block device

    /// Generate unique UUID for this installation
    async fn generate_installation_uuid(&mut self) -> Result<String> {
        let uuid_output = self
            .runner
            .execute_with_output(
                "dd if=/dev/urandom bs=1 count=100 2>/dev/null | tr -dc 'a-z0-9' | cut -c-6",
            )
            .await?;
        let uuid = uuid_output.trim().to_string();

        // Write UUID to target
        self.runner
            .execute(&format!("echo 'UUID={}' > /mnt/targetos/uuid", uuid))
            .await?;
        self.runner
            .execute(&format!(
                "echo 'DISK={}' >> /mnt/targetos/uuid",
                self.variables.get("DISK").unwrap_or(&"unknown".to_string())
            ))
            .await?;

        info!("Generated installation UUID: {}", uuid);
        Ok(uuid)
    }

    /// Create bpool (boot pool)
    async fn create_bpool(&mut self, config: &InstallationConfig) -> Result<()> {
        info!("Creating bpool");

        let bpool_cmd = Self::build_bpool_create_command(&config.disk_device);
        self.log_and_execute("Creating bpool", &bpool_cmd).await?;

        // Set the pool-root mountpoint to /boot AFTER creation. The OpenZFS HOWTO
        // sets it at create time, but that only works with a pristine altroot;
        // this installer reuses /mnt/targetos across runs, so a create-time
        // `mountpoint=/boot` fails with "mountpoint exists and is not empty".
        // Setting it afterward just writes the property (canmount=off → nothing
        // mounts). grub-probe reads this pool-root mountpoint to resolve /boot to
        // the bpool vdev instead of mis-resolving to the rpool/LUKS device.
        self.log_and_execute(
            "Set bpool mountpoint=/boot",
            "zfs set mountpoint=/boot bpool",
        )
        .await?;

        Ok(())
    }

    /// Create rpool (root pool) with encryption
    async fn create_rpool(&mut self, _config: &InstallationConfig) -> Result<()> {
        info!("Creating rpool with encryption");

        // Create rpool on the LUKS-mapped block device; encryption is provided by LUKS, so ZFS native encryption is optional and disabled here
        let rpool_cmd = Self::build_rpool_create_command();
        self.log_and_execute("Creating rpool", &rpool_cmd).await?;

        Ok(())
    }

    /// Build the zpool create command for rpool using the LUKS mapper device
    fn build_rpool_create_command() -> String {
        String::from(
            // NOTE: mountpoint=none (+ -m none), NOT the HOWTO's mountpoint=/ —
            // this installer writes /mnt/targetos/uuid before pool creation, so a
            // mountpoint=/ rpool would try to mount over a non-empty altroot and
            // fail. The pool-level mountpoint is irrelevant to boot anyway: the
            // real root is rpool/ROOT/ubuntu_<uuid> (mountpoint=/), and GRUB never
            // reads rpool. Only bpool's mountpoint=/boot matters (for grub-probe).
            "zpool create -o ashift=12 -o autotrim=on \
             -O acltype=posixacl -O xattr=sa -O dnodesize=auto -O compression=lz4 \
             -O normalization=formD -O relatime=on -O canmount=off -O mountpoint=none \
             -m none -R /mnt/targetos rpool /dev/mapper/luks",
        )
    }

    /// Build the zpool create command for bpool. Feature set matches the OpenZFS
    /// "Ubuntu Root on ZFS" HOWTO (grub2 compat + livelist/zpool_checkpoint,
    /// which modern GRUB reads).
    ///
    /// Created with `mountpoint=none -m none` so `zpool create` never tries to
    /// mount over this installer's non-pristine `/mnt/targetos` altroot; the
    /// pool-root mountpoint is set to `/boot` immediately afterward (see
    /// `create_bpool`), which is what grub-probe keys on to resolve /boot to the
    /// bpool vdev.
    fn build_bpool_create_command(disk: &str) -> String {
        format!(
            "zpool create -o ashift=12 -o autotrim=on -o cachefile=/etc/zfs/zpool.cache \
             -o compatibility=grub2 -o feature@livelist=enabled -o feature@zpool_checkpoint=enabled \
             -O devices=off -O acltype=posixacl -O xattr=sa -O compression=lz4 \
             -O normalization=formD -O relatime=on -O canmount=off -O mountpoint=none \
             -m none -R /mnt/targetos bpool {}",
            partition_path(disk, 3)
        )
    }

    /// Create bpool datasets
    async fn create_bpool_datasets(&mut self, uuid: &str) -> Result<()> {
        info!("Creating bpool datasets");

        // Ensure mountpoint exists for /boot
        self.log_and_execute("Ensure /boot mountpoint", "mkdir -p /mnt/targetos/boot")
            .await?;

        self.log_and_execute(
            "Creating bpool/BOOT",
            "zfs create -o canmount=off -o mountpoint=none bpool/BOOT",
        )
        .await?;
        self.log_and_execute(
            "Creating bpool boot dataset",
            &format!("zfs create -o mountpoint=/boot bpool/BOOT/ubuntu_{}", uuid),
        )
        .await?;

        Ok(())
    }

    /// Create comprehensive rpool dataset structure
    async fn create_rpool_datasets(&mut self, uuid: &str) -> Result<()> {
        info!("Creating rpool dataset structure");

        // Root dataset structure
        self.log_and_execute(
            "Creating rpool/ROOT",
            "zfs create -o canmount=off -o mountpoint=none rpool/ROOT",
        )
        .await?;

        let current_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        self.log_and_execute("Creating root filesystem",
            &format!("zfs create -o mountpoint=/ -o com.ubuntu.zsys:bootfs=yes -o com.ubuntu.zsys:last-used={} rpool/ROOT/ubuntu_{}", current_time, uuid)).await?;

        // System directories
        let datasets = vec![
            (
                "usr",
                "rpool/ROOT/ubuntu_{}/usr",
                "-o com.ubuntu.zsys:bootfs=no -o canmount=off",
            ),
            (
                "var",
                "rpool/ROOT/ubuntu_{}/var",
                "-o com.ubuntu.zsys:bootfs=no -o canmount=off",
            ),
            ("var/lib", "rpool/ROOT/ubuntu_{}/var/lib", ""),
            ("var/log", "rpool/ROOT/ubuntu_{}/var/log", ""),
            ("var/spool", "rpool/ROOT/ubuntu_{}/var/spool", ""),
            ("var/cache", "rpool/ROOT/ubuntu_{}/var/cache", ""),
            ("var/lib/nfs", "rpool/ROOT/ubuntu_{}/var/lib/nfs", ""),
            ("var/tmp", "rpool/ROOT/ubuntu_{}/var/tmp", ""),
            ("var/lib/apt", "rpool/ROOT/ubuntu_{}/var/lib/apt", ""),
            ("var/lib/dpkg", "rpool/ROOT/ubuntu_{}/var/lib/dpkg", ""),
            (
                "srv",
                "rpool/ROOT/ubuntu_{}/srv",
                "-o com.ubuntu.zsys:bootfs=no",
            ),
            ("usr/local", "rpool/ROOT/ubuntu_{}/usr/local", ""),
            ("var/games", "rpool/ROOT/ubuntu_{}/var/games", ""),
            (
                "var/lib/AccountsService",
                "rpool/ROOT/ubuntu_{}/var/lib/AccountsService",
                "",
            ),
        ];

        for (name, dataset, opts) in datasets {
            let dataset_name = dataset.replace("{}", uuid);
            self.log_and_execute(
                &format!("Creating {}", name),
                &format!("zfs create {} {}", opts, dataset_name),
            )
            .await?;
        }

        // Set special permissions
        self.log_and_execute("Ensure /root exists", "mkdir -p /mnt/targetos/root")
            .await?;
        self.log_and_execute("Ensure /var/tmp exists", "mkdir -p /mnt/targetos/var/tmp")
            .await?;
        self.log_and_execute("Setting /root permissions", "chmod 700 /mnt/targetos/root")
            .await?;
        self.log_and_execute(
            "Setting /var/tmp permissions",
            "chmod 1777 /mnt/targetos/var/tmp",
        )
        .await?;

        // Create USERDATA structure
        self.log_and_execute(
            "Creating USERDATA",
            "zfs create -o canmount=off -o mountpoint=/ rpool/USERDATA",
        )
        .await?;
        self.log_and_execute("Creating root user data",
            &format!("zfs create -o com.ubuntu.zsys:bootfs-datasets=rpool/ROOT/ubuntu_{} -o canmount=on -o mountpoint=/root rpool/USERDATA/root_{}", uuid, uuid)).await?;

        Ok(())
    }

    /// Helper method to log and execute commands with better error handling
    async fn log_and_execute(&mut self, description: &str, command: &str) -> Result<()> {
        info!("Executing: {} -> {}", description, command);

        match self
            .runner
            .execute_with_error_collection(command, description)
            .await
        {
            Ok((exit_code, stdout, stderr)) => {
                if exit_code == 0 {
                    if !stdout.is_empty() {
                        info!("Command output: {}", stdout.trim());
                    }
                    Ok(())
                } else {
                    error!(
                        "Command '{}' failed with exit code {}",
                        description, exit_code
                    );
                    error!("STDOUT: {}", stdout);
                    error!("STDERR: {}", stderr);

                    // Don't immediately fail - collect debug info
                    if let Ok(debug_info) = self.runner.collect_debug_info().await {
                        error!("System debug information:\n{}", debug_info);
                    }

                    Err(crate::error::AutoInstallError::SshError(format!(
                        "Command '{}' failed with exit code {}: stderr={}",
                        description, exit_code, stderr
                    )))
                }
            }
            Err(e) => {
                error!("Failed to execute command '{}': {}", description, e);

                // Try to collect debug info even if the command completely failed
                if let Ok(debug_info) = self.runner.collect_debug_info().await {
                    error!("System debug information:\n{}", debug_info);
                }

                Err(e)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_rpool_create_command_uses_luks_mapper() {
        let cmd = ZfsManager::build_rpool_create_command();
        assert!(cmd.contains("zpool create"));
        assert!(cmd.contains(" rpool "));
        assert!(cmd.contains("/dev/mapper/luks"));
        assert!(cmd.contains("-R /mnt/targetos"));
    }

    #[test]
    fn test_build_bpool_create_command_has_expected_flags() {
        let cmd = ZfsManager::build_bpool_create_command("/dev/sda");
        assert!(cmd.contains("zpool create"));
        // device should be present and appear at the end of the command
        assert!(cmd.contains(" bpool "));
        assert!(cmd.ends_with("/dev/sda3"));
        assert!(cmd.contains(" -R /mnt/targetos "));
        assert!(cmd.contains("compatibility=grub2"));
        assert!(cmd.contains("cachefile=/etc/zfs/zpool.cache"));
        assert!(cmd.contains("devices=off"));
        assert!(cmd.contains("compression=lz4"));
    }
}
