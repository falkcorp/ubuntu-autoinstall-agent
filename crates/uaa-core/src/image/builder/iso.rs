// file: crates/uaa-core/src/image/builder/iso.rs
// version: 1.1.1
// guid: a1a2a3a4-b5b6-7890-1234-567890abcdef
// last-edited: 2026-07-10

//! ISO management and download utilities

use crate::{
    config::{Architecture, ImageSpec},
    network::download::NetworkDownloader,
    Result,
};
use std::path::{Path, PathBuf};
use tokio::fs;
use tracing::info;

/// ISO download and caching manager
pub struct IsoManager {
    cache_dir: PathBuf,
}

impl IsoManager {
    /// Create a new ISO manager with cache directory
    pub fn new(cache_dir: PathBuf) -> Self {
        Self { cache_dir }
    }

    /// Download Ubuntu Server ISO if not cached and extract kernel/initrd for direct boot
    pub async fn get_ubuntu_iso(&self, spec: &ImageSpec) -> Result<PathBuf> {
        // For autoinstall, we need the full Ubuntu Server ISO, not netboot
        let iso_dir = self.cache_dir.join("isos").join(format!(
            "ubuntu-{}-{}",
            spec.ubuntu_version,
            spec.architecture.as_str()
        ));

        let extract_dir = self.cache_dir.join("extracted").join(format!(
            "ubuntu-{}-{}",
            spec.ubuntu_version,
            spec.architecture.as_str()
        ));

        fs::create_dir_all(&iso_dir)
            .await
            .map_err(crate::error::AutoInstallError::IoError)?;

        fs::create_dir_all(&extract_dir)
            .await
            .map_err(crate::error::AutoInstallError::IoError)?;

        // Check if kernel files already extracted
        let kernel_path = extract_dir.join("casper").join("vmlinuz");
        if kernel_path.exists() {
            info!(
                "Using cached Ubuntu Server ISO files: {}",
                extract_dir.display()
            );
            return Ok(extract_dir);
        }

        info!(
            "Downloading and extracting Ubuntu Server ISO: {}",
            extract_dir.display()
        );

        // Download Ubuntu Server ISO
        let iso_url = self.get_ubuntu_server_iso_url(spec)?;
        let iso_path = iso_dir.join(format!(
            "ubuntu-{}-live-server-{}.iso",
            spec.ubuntu_version,
            spec.architecture.as_str()
        ));

        info!("Downloading Ubuntu Server ISO from: {}", iso_url);
        self.download_file(&iso_url, &iso_path).await?;

        // Extract kernel and initrd from ISO
        self.extract_iso_boot_files(&iso_path, &extract_dir).await?;

        Ok(extract_dir)
    }
    /// Get Ubuntu Server ISO download URL
    fn get_ubuntu_server_iso_url(&self, spec: &ImageSpec) -> Result<String> {
        // Ubuntu Server ISO URLs follow this pattern:
        // https://releases.ubuntu.com/{codename}/ubuntu-{version}-live-server-{arch}.iso
        let arch_suffix = match spec.architecture {
            Architecture::Amd64 => "amd64",
            Architecture::Arm64 => "arm64",
        };

        // Convert version to codename for releases URL
        let codename = match spec.ubuntu_version.as_str() {
            "26.04" => "resolute",
            "25.10" => "questing",
            "25.04" => "plucky",
            "24.10" => "oracular",
            "24.04" => "noble",
            "23.10" => "mantic",
            "23.04" => "lunar",
            _ => {
                return Err(crate::error::AutoInstallError::ConfigError(format!(
                    "Unsupported Ubuntu version: {}",
                    spec.ubuntu_version
                )));
            }
        };

        Ok(format!(
            "https://releases.ubuntu.com/{}/ubuntu-{}-live-server-{}.iso",
            codename, spec.ubuntu_version, arch_suffix
        ))
    }

    /// Extract kernel and initrd from Ubuntu Server ISO for direct boot
    async fn extract_iso_boot_files(&self, iso_path: &Path, extract_dir: &Path) -> Result<()> {
        use tokio::process::Command;

        info!("Extracting boot files from ISO: {}", iso_path.display());

        // Mount the ISO and extract kernel/initrd files
        let mount_dir = extract_dir.join("mnt");
        fs::create_dir_all(&mount_dir)
            .await
            .map_err(crate::error::AutoInstallError::IoError)?;

        // Create a temporary mount script since we need sudo
        let mount_script = extract_dir.join("mount_iso.sh");
        let mount_script_content = format!(
            r#"#!/bin/bash
set -e

# Mount ISO
sudo mount -o loop "{}" "{}"

# Copy casper directory (contains kernel and initrd)
cp -r "{}/casper" "{}"

# Copy .disk directory (contains installer metadata)
if [ -d "{}/disk" ]; then
    cp -r "{}/disk" "{}"
fi

# Unmount ISO
sudo umount "{}"

echo "ISO boot files extracted successfully"
"#,
            iso_path.display(),
            mount_dir.display(),
            mount_dir.display(),
            extract_dir.display(),
            mount_dir.display(),
            mount_dir.display(),
            extract_dir.display(),
            mount_dir.display()
        );

        fs::write(&mount_script, mount_script_content)
            .await
            .map_err(crate::error::AutoInstallError::IoError)?;

        // Make script executable
        Command::new("chmod")
            .args(["+x", mount_script.to_str().unwrap()])
            .output()
            .await
            .map_err(crate::error::AutoInstallError::IoError)?;

        // Execute mount script
        let output = Command::new("bash")
            .arg(mount_script.to_str().unwrap())
            .output()
            .await
            .map_err(crate::error::AutoInstallError::IoError)?;

        if !output.status.success() {
            return Err(crate::error::AutoInstallError::ProcessError {
                command: format!("bash {}", mount_script.display()),
                exit_code: output.status.code(),
                stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            });
        }

        // Clean up mount script
        let _ = fs::remove_file(mount_script).await;

        info!("Successfully extracted boot files from Ubuntu Server ISO");
        Ok(())
    }

    /// Download file with progress
    async fn download_file(&self, url: &str, dest: &Path) -> Result<()> {
        let downloader = NetworkDownloader::new();
        downloader.download_with_progress(url, dest).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Architecture, ImageSpec};
    use tempfile::TempDir;
    use tokio::fs as async_fs;

    #[test]
    fn test_iso_manager_new() {
        // Arrange
        let temp_dir = TempDir::new().unwrap();
        let cache_dir = temp_dir.path().to_path_buf();

        // Act
        let iso_manager = IsoManager::new(cache_dir.clone());

        // Assert
        assert_eq!(iso_manager.cache_dir, cache_dir);
    }

    #[test]
    fn test_get_ubuntu_server_iso_url_amd64() {
        // Arrange
        let temp_dir = TempDir::new().unwrap();
        let iso_manager = IsoManager::new(temp_dir.path().to_path_buf());
        let spec = ImageSpec {
            ubuntu_version: "24.04".to_string(),
            architecture: Architecture::Amd64,
            base_packages: vec![],
            vm_config: crate::config::VmConfig {
                memory_mb: 2048,
                disk_size_gb: 20,
                cpu_cores: 2,
            },
            custom_scripts: vec![],
        };

        // Act
        let result = iso_manager.get_ubuntu_server_iso_url(&spec);

        // Assert
        assert!(result.is_ok());
        let url = result.unwrap();
        assert_eq!(
            url,
            "https://releases.ubuntu.com/noble/ubuntu-24.04-live-server-amd64.iso"
        );
        assert!(url.contains("noble"));
        assert!(url.contains("amd64"));
        assert!(url.contains("24.04"));
    }

    #[test]
    fn test_get_ubuntu_server_iso_url_arm64() {
        // Arrange
        let temp_dir = TempDir::new().unwrap();
        let iso_manager = IsoManager::new(temp_dir.path().to_path_buf());
        let spec = ImageSpec {
            ubuntu_version: "24.10".to_string(),
            architecture: Architecture::Arm64,
            base_packages: vec![],
            vm_config: crate::config::VmConfig {
                memory_mb: 2048,
                disk_size_gb: 20,
                cpu_cores: 2,
            },
            custom_scripts: vec![],
        };

        // Act
        let result = iso_manager.get_ubuntu_server_iso_url(&spec);

        // Assert
        assert!(result.is_ok());
        let url = result.unwrap();
        assert_eq!(
            url,
            "https://releases.ubuntu.com/oracular/ubuntu-24.10-live-server-arm64.iso"
        );
        assert!(url.contains("oracular"));
        assert!(url.contains("arm64"));
        assert!(url.contains("24.10"));
    }

    #[test]
    fn test_get_ubuntu_server_iso_url_all_versions() {
        // Arrange
        let temp_dir = TempDir::new().unwrap();
        let iso_manager = IsoManager::new(temp_dir.path().to_path_buf());

        let test_cases = vec![
            ("25.04", "plucky"),
            ("24.10", "oracular"),
            ("24.04", "noble"),
            ("23.10", "mantic"),
            ("23.04", "lunar"),
        ];

        for (version, expected_codename) in test_cases {
            // Arrange
            let spec = ImageSpec {
                ubuntu_version: version.to_string(),
                architecture: Architecture::Amd64,
                base_packages: vec![],
                vm_config: crate::config::VmConfig {
                    memory_mb: 2048,
                    disk_size_gb: 20,
                    cpu_cores: 2,
                },
                custom_scripts: vec![],
            };

            // Act
            let result = iso_manager.get_ubuntu_server_iso_url(&spec);

            // Assert
            assert!(result.is_ok(), "Failed for version {}", version);
            let url = result.unwrap();
            assert!(
                url.contains(expected_codename),
                "URL {} should contain codename {} for version {}",
                url,
                expected_codename,
                version
            );
            assert!(url.contains(version));
            assert!(url.starts_with("https://releases.ubuntu.com/"));
        }
    }

    #[test]
    fn test_get_ubuntu_server_iso_url_unsupported_version() {
        // Arrange
        let temp_dir = TempDir::new().unwrap();
        let iso_manager = IsoManager::new(temp_dir.path().to_path_buf());
        let spec = ImageSpec {
            ubuntu_version: "99.99".to_string(),
            architecture: Architecture::Amd64,
            base_packages: vec![],
            vm_config: crate::config::VmConfig {
                memory_mb: 2048,
                disk_size_gb: 20,
                cpu_cores: 2,
            },
            custom_scripts: vec![],
        };

        // Act
        let result = iso_manager.get_ubuntu_server_iso_url(&spec);

        // Assert
        assert!(result.is_err());
        match result {
            Err(crate::error::AutoInstallError::ConfigError(msg)) => {
                assert!(msg.contains("Unsupported Ubuntu version: 99.99"));
            }
            _ => panic!("Expected ConfigError for unsupported version"),
        }
    }

    #[tokio::test]
    async fn test_get_ubuntu_iso_cache_directories() {
        // Arrange
        let temp_dir = TempDir::new().unwrap();
        let cache_dir = temp_dir.path().to_path_buf();
        let iso_manager = IsoManager::new(cache_dir.clone());
        let spec = ImageSpec {
            ubuntu_version: "24.04".to_string(),
            architecture: Architecture::Amd64,
            base_packages: vec![],
            vm_config: crate::config::VmConfig {
                memory_mb: 2048,
                disk_size_gb: 20,
                cpu_cores: 2,
            },
            custom_scripts: vec![],
        };

        // Seed cache with expected kernel/initrd so download path is skipped in tests
        let extract_dir = cache_dir
            .join("extracted")
            .join("ubuntu-24.04-amd64")
            .join("casper");
        async_fs::create_dir_all(&extract_dir).await.unwrap();
        async_fs::write(extract_dir.join("vmlinuz"), b"mock-kernel")
            .await
            .unwrap();
        async_fs::write(extract_dir.join("initrd"), b"mock-initrd")
            .await
            .unwrap();

        // Act
        let result = iso_manager.get_ubuntu_iso(&spec).await;

        // Assert
        let expected_extract_dir = cache_dir.join("extracted").join("ubuntu-24.04-amd64");
        assert!(expected_extract_dir.exists());
        assert!(cache_dir.join("isos").join("ubuntu-24.04-amd64").exists());

        let output_path = result.expect("iso manager should return cached path");
        assert_eq!(output_path, expected_extract_dir);
    }

    #[test]
    fn test_iso_manager_path_construction() {
        // Arrange
        let temp_dir = TempDir::new().unwrap();
        let cache_dir = temp_dir.path().to_path_buf();
        let iso_manager = IsoManager::new(cache_dir.clone());

        // Test different architecture path construction
        let test_cases = vec![
            (Architecture::Amd64, "amd64"),
            (Architecture::Arm64, "arm64"),
        ];

        for (arch, arch_str) in test_cases {
            // Arrange
            let spec = ImageSpec {
                ubuntu_version: "24.04".to_string(),
                architecture: arch,
                base_packages: vec![],
                vm_config: crate::config::VmConfig {
                    memory_mb: 2048,
                    disk_size_gb: 20,
                    cpu_cores: 2,
                },
                custom_scripts: vec![],
            };

            // Act
            let url_result = iso_manager.get_ubuntu_server_iso_url(&spec);

            // Assert
            assert!(url_result.is_ok());
            let url = url_result.unwrap();
            assert!(url.contains(arch_str));
            assert!(url.contains("24.04"));
        }
    }

    #[tokio::test]
    async fn test_download_file_method_structure() {
        // Arrange
        let temp_dir = TempDir::new().unwrap();
        let iso_manager = IsoManager::new(temp_dir.path().to_path_buf());
        let test_file = temp_dir.path().join("test_download.txt");

        // Use mocked downloader to simulate a failure without making real network calls
        crate::network::download::set_mock_download_with_progress(Err(
            crate::error::AutoInstallError::NetworkError("Simulated download failure".to_string()),
        ));

        // Act
        let result = iso_manager
            .download_file("http://unused.test/does-not-exist", &test_file)
            .await;

        // Assert
        // Should fail gracefully with network error
        assert!(result.is_err());
        // File should not be created for failed download
        assert!(!test_file.exists());
    }
}
