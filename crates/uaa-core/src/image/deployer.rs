// file: crates/uaa-core/src/image/deployer.rs
// version: 1.0.1
// guid: m3n4o5p6-q7r8-9012-3456-789012mnopqr

//! Image deployment via SSH and netboot

use crate::config::TargetConfig;
use crate::network::SshClient;
use crate::security::LuksManager;
use crate::utils::QemuUtils;
use crate::Result;
use std::path::Path;
use tracing::{debug, info};

/// Deployer for golden images to target machines
pub struct ImageDeployer {
    luks_manager: LuksManager,
}

impl ImageDeployer {
    /// Create a new image deployer
    pub fn new() -> Self {
        Self {
            luks_manager: LuksManager::new(),
        }
    }

    /// Deploy image via SSH to target machine
    pub async fn deploy_via_ssh(
        &self,
        target: &str,
        config: &TargetConfig,
        golden_image_path: &Path,
    ) -> Result<()> {
        info!("Deploying via SSH to: {}", target);

        // Connect to target machine
        let mut ssh = SshClient::new();

        // Try connecting as root first, then as rescue user
        let connection_result = ssh.connect(target, "root").await;
        if connection_result.is_err() {
            info!("Failed to connect as root, trying with rescue user...");
            ssh.connect(target, "rescue").await.map_err(|e| {
                crate::error::AutoInstallError::SshError(format!(
                    "Failed to connect to target {}: {}. Ensure target is booted into rescue mode.",
                    target, e
                ))
            })?;
        }

        info!("Connected to target machine. Checking system status...");

        // Verify we're in a rescue environment
        self.verify_rescue_environment(&mut ssh).await?;

        // Setup LUKS encryption on target disk
        self.setup_luks_disk(&mut ssh, config).await?;

        // Download and deploy image
        self.deploy_image_to_disk(&mut ssh, config, golden_image_path)
            .await?;

        // Configure bootloader
        self.configure_bootloader(&mut ssh, config).await?;

        // Apply target-specific customizations
        self.apply_customizations(&mut ssh, config).await?;

        info!("SSH deployment completed successfully. Target is ready for reboot.");
        Ok(())
    }

    /// Deploy image via netboot/PXE
    pub async fn deploy_via_netboot(&self, target: &str, _config: &TargetConfig) -> Result<()> {
        info!("Deploying via netboot to: {}", target);

        // This would implement PXE boot deployment
        // For now, return an error as this is a complex implementation
        Err(crate::error::AutoInstallError::NetworkError(
            "Netboot deployment not yet implemented".to_string(),
        ))
    }

    /// Verify target is booted into a rescue environment
    async fn verify_rescue_environment(&self, ssh: &mut SshClient) -> Result<()> {
        info!("Verifying rescue environment");

        // Check if we can access necessary tools
        ssh.execute("command -v cryptsetup").await.map_err(|_| {
            crate::error::AutoInstallError::SshError(
                "cryptsetup not available in rescue environment".to_string(),
            )
        })?;

        ssh.execute("command -v mkfs.ext4").await.map_err(|_| {
            crate::error::AutoInstallError::SshError(
                "filesystem tools not available in rescue environment".to_string(),
            )
        })?;

        // Ensure no mounted filesystems from target disk
        let mounted = ssh
            .execute_with_output("mount | grep '/dev/sd' || true")
            .await?;
        if !mounted.trim().is_empty() {
            info!("Unmounting existing filesystems...");
            ssh.execute("umount -l /dev/sd* 2>/dev/null || true")
                .await?;
        }

        debug!("Rescue environment verification completed");
        Ok(())
    }

    /// Setup LUKS encryption on target disk
    async fn setup_luks_disk(&self, ssh: &mut SshClient, config: &TargetConfig) -> Result<()> {
        info!("Setting up LUKS encryption on {}", config.disk_device);

        // Wipe the disk
        ssh.execute(&format!("wipefs -a {}", config.disk_device))
            .await?;

        // Create LUKS partition
        self.luks_manager
            .create_luks_partition(ssh, &config.disk_device, &config.luks_config)
            .await?;

        // Create filesystem
        let luks_device = "/dev/mapper/ubuntu-root";
        ssh.execute(&format!("mkfs.ext4 {}", luks_device)).await?;

        debug!("LUKS setup completed");
        Ok(())
    }

    /// Deploy image to encrypted disk
    async fn deploy_image_to_disk(
        &self,
        ssh: &mut SshClient,
        _config: &TargetConfig,
        golden_image_path: &Path,
    ) -> Result<()> {
        info!("Deploying golden image to encrypted disk");

        let mount_point = "/mnt/target";
        let luks_device = "/dev/mapper/ubuntu-root";

        // Mount the encrypted filesystem
        ssh.execute(&format!("mkdir -p {}", mount_point)).await?;
        ssh.execute(&format!("mount {} {}", luks_device, mount_point))
            .await?;

        // Extract golden image to target
        self.extract_golden_image(ssh, golden_image_path, mount_point)
            .await?;

        // Unmount
        ssh.execute(&format!("umount {}", mount_point)).await?;

        info!("Image deployment to disk completed");
        Ok(())
    }

    /// Extract golden image to target filesystem
    async fn extract_golden_image(
        &self,
        ssh: &mut SshClient,
        golden_image: &Path,
        target_mount: &str,
    ) -> Result<()> {
        info!("Extracting golden image to target");

        // Use QemuUtils for efficient image extraction
        QemuUtils::extract_image_contents(golden_image, std::path::Path::new(target_mount)).await?;

        // Set up proper ownership and permissions
        ssh.execute(&format!("chown -R root:root {}", target_mount))
            .await?;

        // Ensure boot directory has correct permissions
        ssh.execute(&format!(
            "chmod 755 {}/boot {}/etc {}/var",
            target_mount, target_mount, target_mount
        ))
        .await?;

        info!("Golden image extraction completed");
        Ok(())
    }
    /// Configure bootloader for LUKS
    async fn configure_bootloader(&self, ssh: &mut SshClient, config: &TargetConfig) -> Result<()> {
        info!("Configuring bootloader");

        let mount_point = "/mnt/target";
        let luks_device = "/dev/mapper/ubuntu-root";

        // Remount for configuration
        ssh.execute(&format!("mount {} {}", luks_device, mount_point))
            .await?;

        // Mount special filesystems
        ssh.execute(&format!("mount --bind /dev {}/dev", mount_point))
            .await?;
        ssh.execute(&format!("mount --bind /proc {}/proc", mount_point))
            .await?;
        ssh.execute(&format!("mount --bind /sys {}/sys", mount_point))
            .await?;

        // Update GRUB configuration for LUKS
        let grub_config = format!(
            r#"GRUB_CMDLINE_LINUX="cryptdevice={}:ubuntu-root root={} rw""#,
            config.disk_device, luks_device
        );

        ssh.execute(&format!(
            "echo '{}' >> {}/etc/default/grub",
            grub_config, mount_point
        ))
        .await?;

        // Install and configure GRUB
        ssh.execute(&format!(
            "chroot {} grub-install {}",
            mount_point, config.disk_device
        ))
        .await?;

        ssh.execute(&format!("chroot {} update-grub", mount_point))
            .await?;

        // Update initramfs for LUKS support
        ssh.execute(&format!("chroot {} update-initramfs -u", mount_point))
            .await?;

        // Cleanup mounts
        ssh.execute(&format!("umount {}/sys", mount_point)).await?;
        ssh.execute(&format!("umount {}/proc", mount_point)).await?;
        ssh.execute(&format!("umount {}/dev", mount_point)).await?;
        ssh.execute(&format!("umount {}", mount_point)).await?;

        info!("Bootloader configuration completed");
        Ok(())
    }

    /// Apply target-specific customizations
    async fn apply_customizations(&self, ssh: &mut SshClient, config: &TargetConfig) -> Result<()> {
        info!("Applying target customizations");

        let mount_point = "/mnt/target";
        let luks_device = "/dev/mapper/ubuntu-root";

        // Remount for customization
        ssh.execute(&format!("mount {} {}", luks_device, mount_point))
            .await?;

        // Set hostname
        ssh.execute(&format!(
            "echo '{}' > {}/etc/hostname",
            config.hostname, mount_point
        ))
        .await?;

        // Configure network
        self.configure_network(ssh, config, mount_point).await?;

        // Create users
        self.create_users(ssh, config, mount_point).await?;

        // Install additional packages
        if !config.packages.is_empty() {
            self.install_packages(ssh, config, mount_point).await?;
        }

        // Set timezone
        ssh.execute(&format!(
            "chroot {} ln -sf /usr/share/zoneinfo/{} /etc/localtime",
            mount_point, config.timezone
        ))
        .await?;

        ssh.execute(&format!("umount {}", mount_point)).await?;

        info!("Target customizations completed");
        Ok(())
    }

    /// Configure network settings
    async fn configure_network(
        &self,
        ssh: &mut SshClient,
        config: &TargetConfig,
        mount_point: &str,
    ) -> Result<()> {
        let netplan_config = if config.network.dhcp {
            format!(
                r#"
network:
  version: 2
  ethernets:
    {}:
      dhcp4: true
      nameservers:
        addresses: [{}]
"#,
                config.network.interface,
                config.network.dns_servers.join(", ")
            )
        } else {
            format!(
                r#"
network:
  version: 2
  ethernets:
    {}:
      addresses: [{}]
      gateway4: {}
      nameservers:
        addresses: [{}]
"#,
                config.network.interface,
                config.network.ip_address.as_ref().unwrap(),
                config.network.gateway.as_ref().unwrap(),
                config.network.dns_servers.join(", ")
            )
        };

        ssh.execute(&format!(
            "cat > {}/etc/netplan/01-netcfg.yaml << 'EOF'\n{}\nEOF",
            mount_point, netplan_config
        ))
        .await?;

        Ok(())
    }

    /// Create user accounts
    async fn create_users(
        &self,
        ssh: &mut SshClient,
        config: &TargetConfig,
        mount_point: &str,
    ) -> Result<()> {
        for user in &config.users {
            // Create user
            let shell = user.shell.as_deref().unwrap_or("/bin/bash");
            ssh.execute(&format!(
                "chroot {} useradd -m -s {} {}",
                mount_point, shell, user.name
            ))
            .await?;

            // Add to sudo group if needed
            if user.sudo {
                ssh.execute(&format!(
                    "chroot {} usermod -aG sudo {}",
                    mount_point, user.name
                ))
                .await?;
            }

            // Setup SSH keys
            if !user.ssh_keys.is_empty() {
                let ssh_dir = format!("{}/home/{}/.ssh", mount_point, user.name);
                ssh.execute(&format!("mkdir -p {}", ssh_dir)).await?;

                for key in &user.ssh_keys {
                    ssh.execute(&format!("echo '{}' >> {}/authorized_keys", key, ssh_dir))
                        .await?;
                }

                ssh.execute(&format!(
                    "chroot {} chown -R {}:{} /home/{}/.ssh",
                    mount_point, user.name, user.name, user.name
                ))
                .await?;

                ssh.execute(&format!("chmod 700 {}", ssh_dir)).await?;
                ssh.execute(&format!("chmod 600 {}/authorized_keys", ssh_dir))
                    .await?;
            }
        }

        Ok(())
    }

    /// Install additional packages
    async fn install_packages(
        &self,
        ssh: &mut SshClient,
        config: &TargetConfig,
        mount_point: &str,
    ) -> Result<()> {
        let packages = config.packages.join(" ");

        // Update package lists
        ssh.execute(&format!("chroot {} apt update", mount_point))
            .await?;

        // Install packages
        ssh.execute(&format!(
            "chroot {} apt install -y {}",
            mount_point, packages
        ))
        .await?;

        Ok(())
    }
}

impl Default for ImageDeployer {
    fn default() -> Self {
        Self::new()
    }
}
