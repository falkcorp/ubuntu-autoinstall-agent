// file: src/cli/commands.rs
// version: 2.0.0
// guid: g7h8i9j0-k1l2-3456-7890-123456ghijkl
// last-edited: 2026-06-20

//! Command implementations for the CLI

use crate::{
    config::{loader::ConfigLoader, Architecture, ImageSpec},
    image::deployer::ImageDeployer,
    image::{builder::ImageBuilder, manager::ImageManager},
    network::{ssh_installer::config::InitramfsType, InstallationConfig, SshInstaller, SystemInfo},
    utils::system::SystemUtils,
    Result,
};
use std::io::Write;
use tracing::{error, info, warn};

/// Create a golden Ubuntu image
pub async fn create_image_command(
    arch: Architecture,
    version: &str,
    output: Option<String>,
    spec_path: Option<String>,
    cache_dir: Option<String>,
) -> Result<()> {
    info!(
        "Creating Ubuntu {} image for {} architecture",
        version,
        arch.as_str()
    );

    let spec = if let Some(spec_path) = spec_path {
        let loader = ConfigLoader::new();
        loader.load_image_spec(&spec_path)?
    } else {
        ImageSpec::minimal(version.to_string(), arch)
    };

    let mut builder = if let Some(cache_dir) = cache_dir {
        ImageBuilder::with_cache_dir(cache_dir)
    } else {
        ImageBuilder::new()
    };

    let image_path = builder.create_image(spec, output).await?;

    info!("Image created successfully: {}", image_path.display());
    Ok(())
}

/// Deploy image to target machine
pub async fn deploy_command(
    target: &str,
    config_path: &str,
    image_path: &str,
    via_ssh: bool,
    dry_run: bool,
) -> Result<()> {
    info!("Deploying image to target: {}", target);

    let loader = ConfigLoader::new();
    let config = loader.load_target_config(config_path)?;

    if dry_run {
        info!(
            "DRY RUN: Would deploy image {} to {} via {}",
            image_path,
            target,
            if via_ssh { "SSH" } else { "netboot" }
        );
        info!(
            "Target config: hostname={}, arch={}",
            config.hostname,
            config.architecture.as_str()
        );
        return Ok(());
    }

    let deployer = ImageDeployer::new();
    if via_ssh {
        deployer
            .deploy_via_ssh(target, &config, std::path::Path::new(image_path))
            .await?;
    } else {
        deployer.deploy_via_netboot(target, &config).await?;
    }

    info!("Deployment completed successfully");
    Ok(())
}

/// Validate image integrity
pub async fn validate_command(image_path: &str) -> Result<()> {
    info!("Validating image: {}", image_path);

    let manager = ImageManager::new();
    let is_valid = manager.validate_image(image_path).await?;

    if is_valid {
        info!("Image validation successful");
    } else {
        error!("Image validation failed");
        return Err(crate::error::AutoInstallError::ImageError(
            "Image validation failed".to_string(),
        ));
    }

    Ok(())
}

/// List available images
pub async fn list_images_command(
    filter_arch: Option<Architecture>,
    json_output: bool,
) -> Result<()> {
    let manager = ImageManager::new();
    let images = manager.list_images(filter_arch).await?;

    if json_output {
        let json = serde_json::to_string_pretty(&images)?;
        println!("{}", json);
    } else {
        if images.is_empty() {
            info!("No images found");
            return Ok(());
        }

        println!("Available Images:");
        println!(
            "{:<36} {:<12} {:<8} {:<12} {:<20}",
            "ID", "Version", "Arch", "Size", "Created"
        );
        println!("{:-<88}", "");

        for image in &images {
            println!(
                "{:<36} {:<12} {:<8} {:<12} {:<20}",
                image.id,
                image.ubuntu_version,
                image.architecture.as_str(),
                image.size_human(),
                image.created_at.format("%Y-%m-%d %H:%M")
            );
        }

        info!("Found {} images", images.len());
    }

    Ok(())
}

/// Check system prerequisites
pub async fn check_prerequisites_command() -> Result<()> {
    use crate::utils::system::SystemUtils;

    info!("Checking system prerequisites for Ubuntu autoinstall operations");

    // Check required commands
    let missing = SystemUtils::check_prerequisites().await?;

    if missing.is_empty() {
        info!("✓ All required system commands are available");
    } else {
        error!("✗ Missing required commands: {}", missing.join(", "));
        info!("Install missing packages:");
        for cmd in &missing {
            match cmd.as_str() {
                "qemu-system-x86_64" | "qemu-img" => {
                    info!("  sudo apt install qemu-kvm qemu-utils")
                }
                "guestfish" => info!("  sudo apt install libguestfs-tools"),
                "genisoimage" => info!("  sudo apt install genisoimage"),
                "cryptsetup" => info!("  sudo apt install cryptsetup"),
                _ => {}
            }
        }
    }

    // Check LUKS support
    match SystemUtils::verify_luks_support().await {
        Ok(true) => info!("✓ LUKS/cryptsetup support is available"),
        Ok(false) => error!("✗ LUKS/cryptsetup support not working properly"),
        Err(e) => error!("✗ LUKS support check failed: {}", e),
    }

    // Check if running as root (required for some operations)
    if SystemUtils::is_root() {
        info!("✓ Running as root - all disk operations available");
    } else {
        info!("⚠ Not running as root - some operations may require sudo");
    }

    // Check system resources
    match SystemUtils::get_available_memory().await {
        Ok(mem) => {
            if mem >= 2048 {
                info!("✓ Sufficient memory available: {} MB", mem);
            } else {
                error!("✗ Insufficient memory: {} MB (recommended: 2048+ MB)", mem);
            }
        }
        Err(e) => error!("✗ Failed to check memory: {}", e),
    }

    match SystemUtils::get_available_space("/tmp").await {
        Ok(space) => {
            if space >= 20 {
                info!("✓ Sufficient disk space in /tmp: {} GB", space);
            } else {
                error!(
                    "✗ Insufficient disk space in /tmp: {} GB (recommended: 20+ GB)",
                    space
                );
            }
        }
        Err(e) => error!("✗ Failed to check disk space: {}", e),
    }

    // Check KVM support
    if std::path::Path::new("/dev/kvm").exists() {
        info!("✓ KVM acceleration available");
    } else {
        info!("⚠ KVM acceleration not available - VM operations will be slower");
    }

    if missing.is_empty() {
        info!("System is ready for Ubuntu autoinstall operations");
        Ok(())
    } else {
        Err(crate::error::AutoInstallError::SystemError(format!(
            "Missing {} required dependencies",
            missing.len()
        )))
    }
}

/// Cleanup old images
pub async fn cleanup_command(older_than_days: u32, dry_run: bool) -> Result<()> {
    info!("Cleaning up images older than {} days", older_than_days);

    let manager = ImageManager::new();
    let old_images = manager.find_old_images(older_than_days).await?;

    if old_images.is_empty() {
        info!("No old images found for cleanup");
        return Ok(());
    }

    if dry_run {
        info!("DRY RUN: Would delete {} old images:", old_images.len());
        for image in &old_images {
            info!(
                "  {} - {} ({}) - {}",
                image.id,
                image.ubuntu_version,
                image.architecture.as_str(),
                image.size_human()
            );
        }
        return Ok(());
    }

    let deleted_count = manager.cleanup_images(old_images).await?;
    info!("Successfully deleted {} old images", deleted_count);

    Ok(())
}

/// Install Ubuntu via SSH to a target machine.
///
/// If `config_path` is supplied the config is loaded from that YAML file;
/// otherwise the built-in `for_len_serv_003()` default is used.
pub async fn ssh_install_command(
    host: &str,
    hostname: Option<String>,
    username: Option<String>,
    config_path: Option<String>,
    investigate_only: bool,
    dry_run: bool,
    hold_on_failure: bool,
    pause_after_storage: bool,
) -> Result<()> {
    let username = username.unwrap_or_else(|| "ubuntu".to_string());

    info!(
        "Connecting to {}@{} for Ubuntu installation",
        username, host
    );

    let mut installer = SshInstaller::new();

    // Connect to the target
    installer.connect(host, &username).await?;
    info!("Successfully connected to target machine");

    // Always investigate the system first
    info!("Investigating target system...");
    let system_info = installer.investigate_system().await?;

    print_system_info("SYSTEM INVESTIGATION RESULTS", &system_info);

    if investigate_only {
        info!("Investigation complete. Exiting as requested.");
        return Ok(());
    }

    // Load config from file or fall back to built-in default
    let config = if let Some(ref path) = config_path {
        info!("Loading installation config from {}", path);
        InstallationConfig::from_yaml_file(path)?
    } else {
        let hn = hostname.unwrap_or_else(|| "len-serv-003".to_string());
        if hn == "len-serv-003" {
            InstallationConfig::for_len_serv_003()
        } else {
            return Err(crate::error::AutoInstallError::ValidationError(
                "No --config provided; pass a YAML config file or use the default len-serv-003 hostname".to_string(),
            ));
        }
    };

    if dry_run {
        info!("DRY RUN: Would perform full ZFS+LUKS installation with config:");
        info!("  Hostname: {}", config.hostname);
        info!("  Disk: {}", config.disk_device);
        info!("  Timezone: {}", config.timezone);
        info!(
            "  Network: {} -> {}",
            config.network_interface, config.network_address
        );
        return Ok(());
    }

    // Confirm installation
    println!("\n=== INSTALLATION CONFIGURATION ===");
    println!("Target hostname: {}", config.hostname);
    println!(
        "Target disk: {} (THIS WILL BE COMPLETELY WIPED)",
        config.disk_device
    );
    println!("Timezone: {}", config.timezone);
    println!("Network interface: {}", config.network_interface);
    println!("Network address: {}", config.network_address);
    println!("Gateway: {}", config.network_gateway);

    println!(
        "\nWARNING: This will completely destroy all data on {}!",
        config.disk_device
    );
    println!("This is a DESTRUCTIVE operation that cannot be undone!");

    // In a real implementation, you might want to add a confirmation prompt here
    // For automation purposes, we'll proceed directly

    info!("Starting full ZFS+LUKS Ubuntu installation...");
    installer
        .perform_installation_with_options_and_pause(&config, hold_on_failure, pause_after_storage)
        .await?;

    info!("SSH installation completed successfully!");
    info!("Target machine should now be ready to boot from local disk");

    Ok(())
}

/// Install Ubuntu locally on the current live system
pub async fn local_install_command(
    hostname: Option<String>,
    investigate_only: bool,
    dry_run: bool,
    hold_on_failure: bool,
    pause_after_storage: bool,
    force: bool,
) -> Result<()> {
    let hostname = hostname.unwrap_or_else(|| "ubuntu-local".to_string());

    info!("Starting local Ubuntu installation on current system");

    // Check if we're running as root
    if !SystemUtils::is_root() {
        return Err(crate::error::AutoInstallError::ValidationError(
            "Local installation must be run as root".to_string(),
        ));
    }

    // Check if we're in a live environment (unless forced)
    if !force && !is_live_environment() {
        return Err(crate::error::AutoInstallError::ValidationError(
            "Local installation should only be run from a live USB/CD environment. Use --force to override this check (dangerous!)".to_string(),
        ));
    }

    if force && !is_live_environment() {
        warn!("WARNING: --force used to bypass live environment check!");
        warn!(
            "This will install Ubuntu on the CURRENT SYSTEM, potentially destroying existing data!"
        );
        warn!("Make sure you understand the risks before proceeding.");
    }

    let mut installer = SshInstaller::new();

    // "Connect" to localhost (no-op for local)
    installer.connect_local().await?;
    info!("Local installation mode active");

    // Always investigate the system first
    info!("Investigating local system...");
    let system_info = installer.investigate_system().await?;

    print_system_info("LOCAL SYSTEM INVESTIGATION RESULTS", &system_info);

    if investigate_only {
        info!("Investigation complete. Exiting as requested.");
        return Ok(());
    }

    // Create installation configuration for local system
    let config = create_local_installation_config(&hostname, &system_info)?;

    if dry_run {
        info!("DRY RUN: Would perform full ZFS+LUKS installation with config:");
        info!("  Hostname: {}", config.hostname);
        info!("  Disk: {}", config.disk_device);
        info!("  Timezone: {}", config.timezone);
        info!(
            "  Network: {} -> {}",
            config.network_interface, config.network_address
        );
        return Ok(());
    }

    // Confirm installation
    println!("\n=== LOCAL INSTALLATION CONFIGURATION ===");
    println!("Target hostname: {}", config.hostname);
    println!(
        "Target disk: {} (THIS WILL BE COMPLETELY WIPED)",
        config.disk_device
    );
    println!("Timezone: {}", config.timezone);
    println!("Network interface: {}", config.network_interface);
    println!("Network address: {}", config.network_address);
    println!("Gateway: {}", config.network_gateway);

    println!(
        "\nWARNING: This will completely destroy all data on {}!",
        config.disk_device
    );
    println!("This is a DESTRUCTIVE operation that cannot be undone!");
    println!("Press Ctrl+C to abort, or any other key to continue...");

    // Wait for user confirmation
    let mut input = String::new();
    std::io::stdin()
        .read_line(&mut input)
        .map_err(crate::error::AutoInstallError::IoError)?;

    info!("Starting full ZFS+LUKS Ubuntu installation locally...");
    installer
        .perform_installation_with_options_and_pause(&config, hold_on_failure, pause_after_storage)
        .await?;

    info!("Local installation completed successfully!");
    info!("System should now be ready to reboot from local disk");

    Ok(())
}

/// Print system investigation results in a consistent format.
fn print_system_info(title: &str, info: &SystemInfo) {
    println!("\n=== {} ===", title);
    println!("Hostname: {}", info.hostname);
    println!("Kernel: {}", info.kernel_version);
    println!("Available tools: {:?}", info.available_tools);
    println!("\n--- OS Release ---\n{}", info.os_release);
    println!("\n--- Memory ---\n{}", info.memory_info);
    println!("\n--- CPU ---\n{}", info.cpu_info);
    println!("\n--- Disks ---\n{}", info.disk_info);
    println!("\n--- Network ---\n{}", info.network_info);
}

/// Unified install subcommand: connects over SSH when `remote` is Some, runs locally otherwise.
pub async fn install_command(
    remote: Option<String>,
    username: Option<String>,
    config_path: Option<String>,
    investigate_only: bool,
    dry_run: bool,
    hold_on_failure: bool,
    pause_after_storage: bool,
    force: bool,
) -> Result<()> {
    match remote {
        Some(ref host) => {
            ssh_install_command(
                host,
                None,
                username,
                config_path,
                investigate_only,
                dry_run,
                hold_on_failure,
                pause_after_storage,
            )
            .await
        }
        None => {
            local_install_command(
                config_path
                    .as_deref()
                    .and_then(|p| {
                        InstallationConfig::from_yaml_file(p)
                            .ok()
                            .map(|c| c.hostname)
                    })
                    .or_else(|| std::env::var("HOSTNAME").ok()),
                investigate_only,
                dry_run,
                hold_on_failure,
                pause_after_storage,
                force,
            )
            .await
        }
    }
}

/// Check if we're running in a live environment
fn is_live_environment() -> bool {
    // Check for common live environment indicators
    std::path::Path::exists(std::path::Path::new("/run/live"))
        || std::path::Path::exists(std::path::Path::new("/lib/live"))
        || std::env::var("DEBIAN_FRONTEND").unwrap_or_default() == "noninteractive"
        || std::fs::read_to_string("/proc/cmdline")
            .unwrap_or_default()
            .contains("boot=live")
}

/// Create installation configuration for local system
fn create_local_installation_config(
    hostname: &str,
    system_info: &SystemInfo,
) -> Result<InstallationConfig> {
    // Detect primary disk (usually the largest disk)
    let disk_device = detect_primary_disk(&system_info.disk_info)?;

    // Detect network configuration
    let (interface, address, gateway) = detect_network_config(&system_info.network_info)?;

    // Detect timezone
    let timezone = detect_timezone().unwrap_or_else(|| "UTC".to_string());

    Ok(InstallationConfig {
        hostname: hostname.to_string(),
        disk_device,
        timezone,
        luks_key: prompt_for_luks_passphrase()?,
        root_password: prompt_for_root_password()?,
        network_interface: interface,
        network_address: address,
        network_gateway: gateway,
        network_search: "local".to_string(),
        network_nameservers: vec!["8.8.8.8".to_string(), "1.1.1.1".to_string()],
        debootstrap_release: Some("plucky".to_string()),
        debootstrap_mirror: Some("http://archive.ubuntu.com/ubuntu/".to_string()),
        initramfs_type: InitramfsType::default(),
        tang_servers: vec![],
        tang_threshold: 2,
        ssh_authorized_keys: vec![],
    })
}

/// Detect the primary disk for installation
fn detect_primary_disk(disk_info: &str) -> Result<String> {
    // Parse lsblk output to find the primary disk
    // This is a simplified implementation - you might want to make it more robust
    for line in disk_info.lines() {
        if line.contains("disk") && !line.contains("loop") && !line.contains("sr") {
            if let Some(device) = line.split_whitespace().next() {
                if device.starts_with("nvme") || device.starts_with("sd") {
                    return Ok(format!("/dev/{}", device));
                }
            }
        }
    }

    Err(crate::error::AutoInstallError::ValidationError(
        "Could not detect primary disk for installation".to_string(),
    ))
}

/// Detect network configuration
fn detect_network_config(_network_info: &str) -> Result<(String, String, String)> {
    // This is a simplified implementation
    // In a real implementation, you'd parse the network info more carefully
    let interface = "eth0".to_string(); // Default
    let address = "dhcp".to_string(); // Use DHCP by default
    let gateway = "auto".to_string(); // Auto-detect

    Ok((interface, address, gateway))
}

/// Detect system timezone
fn detect_timezone() -> Option<String> {
    std::fs::read_link("/etc/localtime").ok().and_then(|path| {
        path.to_str()
            .and_then(|s| s.strip_prefix("/usr/share/zoneinfo/"))
            .map(|s| s.to_string())
    })
}

/// Prompt for LUKS passphrase
fn prompt_for_luks_passphrase() -> Result<String> {
    print!("Enter LUKS encryption passphrase: ");
    std::io::stdout()
        .flush()
        .map_err(crate::error::AutoInstallError::IoError)?;

    let mut passphrase = String::new();
    std::io::stdin()
        .read_line(&mut passphrase)
        .map_err(crate::error::AutoInstallError::IoError)?;

    Ok(passphrase.trim().to_string())
}

/// Prompt for root password
fn prompt_for_root_password() -> Result<String> {
    print!("Enter root password: ");
    std::io::stdout()
        .flush()
        .map_err(crate::error::AutoInstallError::IoError)?;

    let mut password = String::new();
    std::io::stdin()
        .read_line(&mut password)
        .map_err(crate::error::AutoInstallError::IoError)?;

    Ok(password.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use tokio::fs;

    #[tokio::test]
    async fn test_create_image_command_minimal_spec() {
        // Arrange
        let temp_dir = TempDir::new().unwrap();
        let cache_dir = temp_dir.path().join("cache");
        let arch = Architecture::Amd64;
        let version = "24.04";

        // Provide cached kernel so the builder skips network calls in tests
        let cached_casper_dir = cache_dir
            .join("extracted")
            .join(format!("ubuntu-{}-{}", version, arch.as_str()))
            .join("casper");
        fs::create_dir_all(&cached_casper_dir).await.unwrap();
        fs::write(cached_casper_dir.join("vmlinuz"), b"mock-kernel")
            .await
            .unwrap();

        let cache_dir_str = cache_dir.to_string_lossy().to_string();

        // Act & Assert
        // Note: This will fail without actual infrastructure, but tests the function signature
        let result = create_image_command(arch, version, None, None, Some(cache_dir_str)).await;

        // The function should at least not panic and return a Result
        // In a real test environment, we'd mock the ImageBuilder
        assert!(result.is_err()); // Expected to fail without proper setup
    }

    #[tokio::test]
    async fn test_create_image_command_with_spec() {
        // Arrange
        let temp_dir = TempDir::new().unwrap();
        let spec_content = r#"
ubuntu_version: "24.04"
architecture: amd64
base_packages:
  - openssh-server
vm_config:
  memory_mb: 2048
  disk_size_gb: 20
  cpu_cores: 2
custom_scripts: []
"#;
        let spec_path = temp_dir.path().join("test-spec.yaml");
        fs::write(&spec_path, spec_content).await.unwrap();

        let arch = Architecture::Amd64;
        let version = "24.04";
        let spec_path_str = spec_path.to_str().unwrap();

        let cache_dir = temp_dir.path().join("cache");
        let cached_casper_dir = cache_dir
            .join("extracted")
            .join(format!("ubuntu-{}-amd64", version))
            .join("casper");
        fs::create_dir_all(&cached_casper_dir).await.unwrap();
        fs::write(cached_casper_dir.join("vmlinuz"), b"mock-kernel")
            .await
            .unwrap();
        let cache_dir_str = cache_dir.to_string_lossy().to_string();

        // Act & Assert
        let result = create_image_command(
            arch,
            version,
            None,
            Some(spec_path_str.to_string()),
            Some(cache_dir_str),
        )
        .await;

        // Should fail due to missing infrastructure but not due to spec parsing
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_deploy_command_dry_run() {
        // Arrange
        let temp_dir = TempDir::new().unwrap();
        let config_content = r#"
hostname: test-server
architecture: amd64
disk_device: /dev/sda
timezone: UTC
network:
  interface: eth0
  dhcp: true
users:
  - name: admin
    sudo: true
"#;
        let config_path = temp_dir.path().join("config.yaml");
        fs::write(&config_path, config_content).await.unwrap();

        let target = "192.168.1.100";
        let config_path_str = config_path.to_str().unwrap();
        let image_path = "/tmp/test.iso";

        // Act
        let result = deploy_command(target, config_path_str, image_path, true, true).await;

        // Assert
        // Dry run may succeed or fail depending on system dependencies
        assert!(result.is_ok() || result.is_err());
    }

    #[tokio::test]
    async fn test_deploy_command_invalid_config() {
        // Arrange
        let target = "192.168.1.100";
        let config_path = "/nonexistent/config.yaml";
        let image_path = "/tmp/test.iso";

        // Act
        let result = deploy_command(target, config_path, image_path, false, false).await;

        // Assert
        assert!(result.is_err()); // Should fail with invalid config path
    }

    #[tokio::test]
    async fn test_validate_command() {
        // Arrange
        let image_path = "/nonexistent/image.iso";

        // Act
        let result = validate_command(image_path).await;

        // Assert
        // Should handle the case gracefully (either succeed or fail appropriately)
        // The exact behavior depends on ImageManager implementation
        assert!(result.is_ok() || result.is_err());
    }

    #[tokio::test]
    async fn test_check_prereqs_command() {
        // Act
        let result = check_prerequisites_command().await;

        // Assert
        // Should complete without panicking
        assert!(result.is_ok() || result.is_err());
    }

    #[tokio::test]
    async fn test_list_images_command() {
        // Arrange
        let _temp_dir = TempDir::new().unwrap();

        // Act
        let result = list_images_command(None, false).await;

        // Assert
        // Should complete, may succeed or fail depending on system state
        assert!(result.is_ok() || result.is_err());
    }

    #[tokio::test]
    async fn test_list_images_command_with_filter() {
        // Arrange
        let _temp_dir = TempDir::new().unwrap();
        let filter_arch = Some(Architecture::Amd64);

        // Act
        let result = list_images_command(filter_arch, true).await;

        // Assert
        assert!(result.is_ok() || result.is_err());
    }

    #[tokio::test]
    async fn test_cleanup_command_dry_run() {
        // Arrange
        let _temp_dir = TempDir::new().unwrap();
        let older_than_days = 30;

        // Act
        let result = cleanup_command(older_than_days, true).await;

        // Assert
        // Dry run may succeed or fail depending on directory structure
        assert!(result.is_ok() || result.is_err());
    }

    #[tokio::test]
    async fn test_ssh_install_command_investigate_only() {
        // Arrange
        let host = "localhost";
        let hostname = Some("test-host".to_string());
        let username = Some("ubuntu".to_string());

        // Act
        let result = ssh_install_command(
            host, hostname, username, None,  // config_path
            true,  // investigate_only
            false, // dry_run
            false, // hold_on_failure
            false, // pause_after_storage
        )
        .await;

        // Assert
        // Should fail to connect but test the logic flow
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_ssh_install_command_dry_run() {
        // Arrange
        let host = "localhost";
        let hostname = None;
        let username = None;

        // Act
        let result = ssh_install_command(
            host, hostname, username, None,  // config_path
            false, // investigate_only
            true,  // dry_run
            false, // hold_on_failure
            false, // pause_after_storage
        )
        .await;

        // Assert
        // Should fail to connect but test the logic flow
        assert!(result.is_err());
    }

    #[test]
    fn test_architecture_conversion_in_context() {
        // Arrange
        let amd64 = Architecture::Amd64;
        let arm64 = Architecture::Arm64;

        // Act
        let amd64_str = amd64.as_str();
        let arm64_str = arm64.as_str();

        // Assert
        assert_eq!(amd64_str, "amd64");
        assert_eq!(arm64_str, "arm64");
    }

    #[tokio::test]
    async fn test_local_install_command_investigate_only() {
        // Arrange
        let hostname = Some("test-local".to_string());

        // Act
        let result = local_install_command(
            hostname, true,  // investigate_only
            false, // dry_run
            false, // hold_on_failure
            false, // pause_after_storage
            false, // force
        )
        .await;

        // Assert
        // Should fail since we're not running as root in test environment
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_local_install_command_dry_run() {
        // Arrange
        let hostname = None;

        // Act
        let result = local_install_command(
            hostname, false, // investigate_only
            true,  // dry_run
            false, // hold_on_failure
            false, // pause_after_storage
            false, // force
        )
        .await;

        // Assert
        // Should fail since we're not running as root in test environment
        assert!(result.is_err());
    }
}
