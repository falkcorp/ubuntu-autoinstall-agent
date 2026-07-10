// file: crates/uaa-core/src/image/builder/cloudinit.rs
// version: 1.1.1
// guid: c1c2c3c4-d5d6-7890-1234-567890cdefgh

//! Cloud-init configuration generation

use crate::config::ImageSpec;
use crate::Result;
use std::path::PathBuf;
use tokio::fs;
use tracing::debug;

/// Cloud-init configuration manager
pub struct CloudInitManager {
    work_dir: PathBuf,
}

impl CloudInitManager {
    /// Create a new cloud-init manager
    pub fn new(work_dir: PathBuf) -> Self {
        Self { work_dir }
    }

    /// Create cloud-init configuration for automated installation
    pub async fn create_cloud_init_config(&self, spec: &ImageSpec) -> Result<PathBuf> {
        let cloud_init_dir = self.work_dir.join("cloud-init");
        fs::create_dir_all(&cloud_init_dir).await?;

        // Create user-data
        let user_data = self.generate_user_data(spec)?;
        let user_data_path = cloud_init_dir.join("user-data");
        fs::write(&user_data_path, user_data).await?;

        // Create meta-data
        let meta_data = format!("instance-id: ubuntu-autoinstall-{}\n", uuid::Uuid::new_v4());
        let meta_data_path = cloud_init_dir.join("meta-data");
        fs::write(&meta_data_path, meta_data).await?;

        debug!("Created cloud-init config in: {}", cloud_init_dir.display());
        Ok(cloud_init_dir)
    }

    /// Generate cloud-init user-data for automated installation
    fn generate_user_data(&self, spec: &ImageSpec) -> Result<String> {
        let packages = spec.base_packages.join("\n    - ");

        // Generate a password hash for the ubuntu user (password: 'ubuntu')
        // In production, this should be configurable or use key-based auth only
        let password_hash = "$6$rounds=4096$saltsalt$Nn9XLY39PZBO1NMdM9M1BKoJFtIpEcj1zLEHdN6mU.FWrJKOvE4PN8.BGeLhq.KOdFBVZ3MmE7Bl/VLy5L78O1";

        let config = format!(
            r#"#cloud-config
autoinstall:
  version: 1
  locale: en_US.UTF-8
  keyboard:
    layout: us
    variant: ''
  network:
    version: 2
    ethernets:
      eth0:
        dhcp4: true
  storage:
    layout:
      name: direct
      match:
        size: largest
  packages:
    - {}
  ssh:
    install-server: true
    allow-pw: false
    authorized-keys:
      - ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABgQDHQGvTZ8nZ8/temp-key-for-image-creation
  identity:
    realname: Ubuntu User
    username: ubuntu
    hostname: ubuntu-autoinstall
    password: '{}'
  kernel:
    flavor: generic
  timezone: UTC
  updates: security
  shutdown: reboot
  late-commands:
    # Configure GRUB for serial console
    - echo 'GRUB_TERMINAL="console serial"' >> /target/etc/default/grub
    - echo 'GRUB_SERIAL_COMMAND="serial --speed=115200 --unit=0 --word=8 --parity=no --stop=1"' >> /target/etc/default/grub
    - echo 'GRUB_CMDLINE_LINUX_DEFAULT="console=ttyS0,115200n8 console=tty0"' >> /target/etc/default/grub
    # Configure sudoers for passwordless sudo
    - echo 'ubuntu ALL=(ALL) NOPASSWD:ALL' >> /target/etc/sudoers.d/ubuntu-nopasswd
    - chmod 440 /target/etc/sudoers.d/ubuntu-nopasswd
    # Remove temporary SSH key - it will be replaced during VM provisioning
    - rm -f /target/home/ubuntu/.ssh/authorized_keys
    - echo "Image creation completed at $(date)" > /target/var/log/autoinstall.log
    # Update GRUB configuration
    - chroot /target update-grub
  error-commands:
    - echo "Installation failed at $(date)" > /target/var/log/autoinstall-error.log
    - journalctl -b > /target/var/log/autoinstall-journal.log
"#,
            packages, password_hash
        );

        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Architecture, VmConfig};
    use tempfile::TempDir;

    fn create_test_image_spec() -> ImageSpec {
        ImageSpec {
            ubuntu_version: "24.04".to_string(),
            architecture: Architecture::Amd64,
            base_packages: vec![
                "openssh-server".to_string(),
                "curl".to_string(),
                "htop".to_string(),
            ],
            vm_config: VmConfig {
                memory_mb: 2048,
                disk_size_gb: 20,
                cpu_cores: 2,
            },
            custom_scripts: vec![],
        }
    }

    #[test]
    fn test_cloud_init_manager_creation() {
        // Arrange
        let temp_dir = TempDir::new().unwrap();
        let work_dir = temp_dir.path().to_path_buf();

        // Act
        let manager = CloudInitManager::new(work_dir.clone());

        // Assert
        assert_eq!(manager.work_dir, work_dir);
    }

    #[tokio::test]
    async fn test_create_cloud_init_config() {
        // Arrange
        let temp_dir = TempDir::new().unwrap();
        let manager = CloudInitManager::new(temp_dir.path().to_path_buf());
        let spec = create_test_image_spec();

        // Act
        let result = manager.create_cloud_init_config(&spec).await;

        // Assert
        assert!(result.is_ok());
        let cloud_init_dir = result.unwrap();
        assert!(cloud_init_dir.exists());
        assert!(cloud_init_dir.join("user-data").exists());
        assert!(cloud_init_dir.join("meta-data").exists());
    }

    #[tokio::test]
    async fn test_user_data_content() {
        // Arrange
        let temp_dir = TempDir::new().unwrap();
        let manager = CloudInitManager::new(temp_dir.path().to_path_buf());
        let spec = create_test_image_spec();

        // Act
        let cloud_init_dir = manager.create_cloud_init_config(&spec).await.unwrap();
        let user_data_content = fs::read_to_string(cloud_init_dir.join("user-data"))
            .await
            .unwrap();

        // Assert
        assert!(user_data_content.contains("#cloud-config"));
        assert!(user_data_content.contains("autoinstall:"));
        assert!(user_data_content.contains("openssh-server"));
        assert!(user_data_content.contains("curl"));
        assert!(user_data_content.contains("htop"));
        assert!(user_data_content.contains("version: 1"));
        assert!(user_data_content.contains("locale: en_US.UTF-8"));
    }

    #[tokio::test]
    async fn test_meta_data_content() {
        // Arrange
        let temp_dir = TempDir::new().unwrap();
        let manager = CloudInitManager::new(temp_dir.path().to_path_buf());
        let spec = create_test_image_spec();

        // Act
        let cloud_init_dir = manager.create_cloud_init_config(&spec).await.unwrap();
        let meta_data_content = fs::read_to_string(cloud_init_dir.join("meta-data"))
            .await
            .unwrap();

        // Assert
        assert!(meta_data_content.contains("instance-id: ubuntu-autoinstall-"));
        assert!(meta_data_content.len() > 20); // Should have UUID
    }

    #[test]
    fn test_generate_user_data() {
        // Arrange
        let temp_dir = TempDir::new().unwrap();
        let manager = CloudInitManager::new(temp_dir.path().to_path_buf());
        let spec = create_test_image_spec();

        // Act
        let result = manager.generate_user_data(&spec);

        // Assert
        assert!(result.is_ok());
        let user_data = result.unwrap();

        // Check specific configuration elements
        assert!(user_data.contains("openssh-server"));
        assert!(user_data.contains("curl"));
        assert!(user_data.contains("htop"));
        assert!(user_data.contains("autoinstall:"));
        assert!(user_data.contains("version: 1"));
        assert!(user_data.contains("timezone: UTC"));
        assert!(user_data.contains("shutdown: reboot"));
    }

    #[test]
    fn test_generate_user_data_with_different_packages() {
        // Arrange
        let temp_dir = TempDir::new().unwrap();
        let manager = CloudInitManager::new(temp_dir.path().to_path_buf());
        let mut spec = create_test_image_spec();
        spec.base_packages = vec!["git".to_string(), "docker.io".to_string()];

        // Act
        let result = manager.generate_user_data(&spec);

        // Assert
        assert!(result.is_ok());
        let user_data = result.unwrap();
        assert!(user_data.contains("git"));
        assert!(user_data.contains("docker.io"));
        assert!(!user_data.contains("openssh-server"));
    }

    #[test]
    fn test_generate_user_data_contains_security_features() {
        // Arrange
        let temp_dir = TempDir::new().unwrap();
        let manager = CloudInitManager::new(temp_dir.path().to_path_buf());
        let spec = create_test_image_spec();

        // Act
        let user_data = manager.generate_user_data(&spec).unwrap();

        // Assert
        // Verify security-related configurations
        assert!(user_data.contains("install-server: true"));
        assert!(user_data.contains("allow-pw: false"));
        assert!(user_data.contains("NOPASSWD:ALL"));
        assert!(user_data.contains("updates: security"));
    }

    #[test]
    fn test_generate_user_data_contains_grub_config() {
        // Arrange
        let temp_dir = TempDir::new().unwrap();
        let manager = CloudInitManager::new(temp_dir.path().to_path_buf());
        let spec = create_test_image_spec();

        // Act
        let user_data = manager.generate_user_data(&spec).unwrap();

        // Assert
        // Verify GRUB configuration for serial console
        assert!(user_data.contains("GRUB_TERMINAL="));
        assert!(user_data.contains("GRUB_SERIAL_COMMAND="));
        assert!(user_data.contains("console=ttyS0,115200n8"));
        assert!(user_data.contains("update-grub"));
    }

    #[test]
    fn test_generate_user_data_contains_error_handling() {
        // Arrange
        let temp_dir = TempDir::new().unwrap();
        let manager = CloudInitManager::new(temp_dir.path().to_path_buf());
        let spec = create_test_image_spec();

        // Act
        let user_data = manager.generate_user_data(&spec).unwrap();

        // Assert
        // Verify error handling and logging
        assert!(user_data.contains("late-commands:"));
        assert!(user_data.contains("error-commands:"));
        assert!(user_data.contains("autoinstall.log"));
        assert!(user_data.contains("autoinstall-error.log"));
    }
}
