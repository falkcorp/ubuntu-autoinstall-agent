// file: crates/uaa-core/tests/integration_test.rs
// version: 1.0.2
// guid: z6a7b8c9-d0e1-2345-6789-012345zabcde

//! Integration tests for Ubuntu AutoInstall Agent

use std::path::PathBuf;
use tempfile::TempDir;
use uaa_core::{
    config::{loader::ConfigLoader, Architecture, TargetConfig},
    image::manager::ImageManager,
    Result,
};

#[tokio::test]
async fn test_config_loading_integration() -> Result<()> {
    let temp_dir = TempDir::new().unwrap();

    // Create a test target config file
    let config_content = r#"
hostname: test-server
architecture: amd64
disk_device: /dev/sda
timezone: UTC
network:
  interface: eth0
  dhcp: true
  dns_servers:
    - 1.1.1.1
users:
  - name: admin
    sudo: true
    ssh_keys:
      - ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABgQC7...
luks_config:
  passphrase: testpassword123
  cipher: aes-xts-plain64
  key_size: 512
  hash: sha256
packages:
  - openssh-server
"#;

    let config_path = temp_dir.path().join("test-config.yaml");
    tokio::fs::write(&config_path, config_content).await?;

    // Load and validate configuration
    let loader = ConfigLoader::new();
    let config = loader.load_target_config(&config_path)?;

    assert_eq!(config.hostname, "test-server");
    assert_eq!(config.architecture, Architecture::Amd64);
    assert_eq!(config.users.len(), 1);
    assert!(config.users[0].sudo);

    Ok(())
}

#[tokio::test]
async fn test_image_spec_loading() -> Result<()> {
    let temp_dir = TempDir::new().unwrap();

    // Create a test image spec file
    let spec_content = r#"
ubuntu_version: "24.04"
architecture: amd64
base_packages:
  - openssh-server
  - curl
  - htop
vm_config:
  memory_mb: 2048
  disk_size_gb: 20
  cpu_cores: 2
custom_scripts: []
"#;

    let spec_path = temp_dir.path().join("test-spec.yaml");
    tokio::fs::write(&spec_path, spec_content).await?;

    // Load and validate specification
    let loader = ConfigLoader::new();
    let spec = loader.load_image_spec(&spec_path)?;

    assert_eq!(spec.ubuntu_version, "24.04");
    assert_eq!(spec.architecture, Architecture::Amd64);
    assert_eq!(spec.base_packages.len(), 3);
    assert_eq!(spec.vm_config.memory_mb, 2048);

    Ok(())
}

#[tokio::test]
async fn test_image_manager_workflow() -> Result<()> {
    let temp_dir = TempDir::new().unwrap();
    let manager = ImageManager::with_images_dir(temp_dir.path());

    // Test that we start with no images
    let images = manager.list_images(None).await?;
    assert_eq!(images.len(), 0);

    // Create a mock image info
    let image_info = uaa_core::config::ImageInfo::new(
        "24.04".to_string(),
        Architecture::Amd64,
        1024 * 1024 * 1024, // 1GB
        "abcdef123456".to_string(),
        PathBuf::from("/tmp/test-image.qcow2"),
    );

    // Register the image
    manager.register_image(image_info.clone()).await?;

    // Verify it was registered
    let images = manager.list_images(None).await?;
    assert_eq!(images.len(), 1);
    assert_eq!(images[0].ubuntu_version, "24.04");

    // Test filtering by architecture
    let amd64_images = manager.list_images(Some(Architecture::Amd64)).await?;
    assert_eq!(amd64_images.len(), 1);

    let arm64_images = manager.list_images(Some(Architecture::Arm64)).await?;
    assert_eq!(arm64_images.len(), 0);

    // Test cleanup (this image is "new" so shouldn't be cleaned up)
    let old_images = manager.find_old_images(1).await?;
    assert_eq!(old_images.len(), 0);

    Ok(())
}

#[tokio::test]
async fn test_environment_variable_substitution() -> Result<()> {
    let temp_dir = TempDir::new().unwrap();

    // Create config with environment variable
    let config_content = r#"
hostname: test-server
architecture: amd64
disk_device: /dev/sda
timezone: UTC
network:
  interface: eth0
  dhcp: true
  dns_servers:
    - 1.1.1.1
users:
  - name: admin
    sudo: true
    ssh_keys:
      - ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABgQC7...
luks_config:
  passphrase: "${TEST_LUKS_PASSWORD}"
  cipher: aes-xts-plain64
  key_size: 512
  hash: sha256
packages:
  - openssh-server
"#;

    let config_path = temp_dir.path().join("test-config.yaml");
    tokio::fs::write(&config_path, config_content).await?;

    // Set up loader with test environment variable
    let mut loader = ConfigLoader::new();
    loader.set_env_var(
        "TEST_LUKS_PASSWORD".to_string(),
        "supersecret123".to_string(),
    );

    // Load configuration
    let config = loader.load_target_config(&config_path)?;

    assert_eq!(config.luks_config.passphrase, "supersecret123");

    Ok(())
}

#[tokio::test]
async fn test_missing_environment_variable() {
    let temp_dir = TempDir::new().unwrap();

    // Create config with missing environment variable
    let config_content = r#"
hostname: test-server
architecture: amd64
disk_device: /dev/sda
timezone: UTC
network:
  interface: eth0
  dhcp: true
  dns_servers:
    - 1.1.1.1
users:
  - name: admin
    sudo: true
    ssh_keys:
      - ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABgQC7...
luks_config:
  passphrase: "${MISSING_VARIABLE}"
  cipher: aes-xts-plain64
  key_size: 512
  hash: sha256
packages:
  - openssh-server
"#;

    let config_path = temp_dir.path().join("test-config.yaml");
    tokio::fs::write(&config_path, config_content)
        .await
        .unwrap();

    // Load configuration should fail
    let loader = ConfigLoader::new();
    let result = loader.load_target_config(&config_path);

    assert!(result.is_err());
    let error = result.unwrap_err();
    assert!(error.to_string().contains("Missing environment variables"));
}

#[tokio::test]
async fn test_validation_integration() -> Result<()> {
    use uaa_core::config::{LuksConfig, NetworkConfig, UserConfig};

    // Test valid target config validation
    let valid_config = TargetConfig {
        hostname: "test-server".to_string(),
        architecture: Architecture::Amd64,
        disk_device: "/dev/sda".to_string(),
        timezone: "UTC".to_string(),
        network: NetworkConfig {
            interface: "eth0".to_string(),
            ip_address: None,
            gateway: None,
            dns_servers: vec!["1.1.1.1".to_string()],
            dhcp: true,
        },
        users: vec![UserConfig {
            name: "admin".to_string(),
            sudo: true,
            ssh_keys: vec!["ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABgQC7...".to_string()],
            shell: Some("/bin/bash".to_string()),
        }],
        luks_config: LuksConfig {
            passphrase: "secure123".to_string(),
            cipher: "aes-xts-plain64".to_string(),
            key_size: 512,
            hash: "sha256".to_string(),
        },
        packages: vec!["openssh-server".to_string()],
    };

    // Should validate successfully
    assert!(valid_config.validate().is_ok());

    // Test invalid config (no users)
    let mut invalid_config = valid_config.clone();
    invalid_config.users.clear();
    assert!(invalid_config.validate().is_err());

    // Test invalid config (no sudo users)
    let mut invalid_config = valid_config.clone();
    invalid_config.users[0].sudo = false;
    assert!(invalid_config.validate().is_err());

    Ok(())
}
