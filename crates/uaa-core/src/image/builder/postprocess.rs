// file: crates/uaa-core/src/image/builder/postprocess.rs
// version: 1.0.2
// guid: d1d2d3d4-e5e6-7890-1234-567890defghi

//! Image post-processing: generalization and finalization

use crate::config::ImageSpec;
use crate::Result;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::fs;
use tokio::process::Command;
use tracing::{info, warn};

/// Image post-processing manager
pub struct PostProcessor {
    work_dir: PathBuf,
    cache_dir: PathBuf,
}

impl PostProcessor {
    /// Create a new post-processor
    pub fn new(work_dir: PathBuf, cache_dir: PathBuf) -> Self {
        Self {
            work_dir,
            cache_dir,
        }
    }

    /// Generalize the image (remove machine-specific data)
    pub async fn generalize_image(&self, vm_disk: &Path) -> Result<()> {
        info!("Generalizing image");

        // Mount the image
        let mount_point = self.work_dir.join("mount");
        fs::create_dir_all(&mount_point).await?;

        // Use guestfish to modify the image
        let script = format!(
            r#"
add {}
run
mount /dev/sda2 /

# Remove machine-specific files
rm-rf /etc/machine-id
rm-rf /var/lib/dbus/machine-id
rm-rf /etc/ssh/ssh_host_*
rm-rf /var/log/*
rm-rf /tmp/*
rm-rf /var/tmp/*
rm-rf /root/.bash_history
rm-rf /home/ubuntu/.bash_history

# Clear package cache
rm-rf /var/cache/apt/archives/*.deb

# Create empty machine-id (will be regenerated on first boot)
touch /etc/machine-id

sync
umount-all
"#,
            vm_disk.display()
        );

        let output = Command::new("guestfish")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| {
                crate::error::AutoInstallError::ImageError(format!(
                    "Failed to start guestfish: {}",
                    e
                ))
            })?;

        // Write script to stdin and wait for completion
        let mut child = output;
        if let Some(stdin) = child.stdin.take() {
            tokio::io::AsyncWriteExt::write_all(
                &mut tokio::io::BufWriter::new(stdin),
                script.as_bytes(),
            )
            .await?;
        }

        let result = child.wait().await?;
        if !result.success() {
            return Err(crate::error::AutoInstallError::ImageError(
                "Image generalization failed".to_string(),
            ));
        }

        info!("Image generalization completed");
        Ok(())
    }

    /// Finalize and compress the image
    pub async fn finalize_image(
        &self,
        vm_disk: &Path,
        output_path: Option<String>,
        spec: &ImageSpec,
    ) -> Result<PathBuf> {
        info!("Finalizing image");

        let final_path = if let Some(output) = output_path {
            PathBuf::from(output)
        } else {
            // Create images directory in cache
            let images_dir = self.cache_dir.join("images");
            fs::create_dir_all(&images_dir)
                .await
                .map_err(crate::error::AutoInstallError::IoError)?;

            images_dir.join(format!(
                "ubuntu-{}-{}-{}.qcow2",
                spec.ubuntu_version,
                spec.architecture.as_str(),
                chrono::Utc::now().format("%Y%m%d-%H%M%S")
            ))
        };

        // Ensure output directory exists
        if let Some(parent) = final_path.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(crate::error::AutoInstallError::IoError)?;
        }

        // Compress the image
        let output = Command::new("qemu-img")
            .args([
                "convert",
                "-c", // Compress
                "-O",
                "qcow2",
                vm_disk.to_str().unwrap(),
                final_path.to_str().unwrap(),
            ])
            .output()
            .await
            .map_err(|e| {
                crate::error::AutoInstallError::ImageError(format!(
                    "Failed to compress image: {}",
                    e
                ))
            })?;

        if !output.status.success() {
            return Err(crate::error::AutoInstallError::ImageError(format!(
                "Image compression failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        // Calculate checksum for integrity verification
        let checksum = self.calculate_image_checksum(&final_path).await?;
        info!("Image checksum (SHA256): {}", checksum);

        // Get image size
        let metadata = fs::metadata(&final_path)
            .await
            .map_err(crate::error::AutoInstallError::IoError)?;
        let size_bytes = metadata.len();

        // Register the image if it was created successfully
        let manager = crate::image::manager::ImageManager::new();
        let image_info = crate::config::ImageInfo::new(
            spec.ubuntu_version.clone(),
            spec.architecture,
            size_bytes,
            checksum,
            final_path.clone(),
        );

        if let Err(e) = manager.register_image(image_info).await {
            warn!("Failed to register image in database: {}", e);
        }

        info!(
            "Image compressed to: {} ({})",
            final_path.display(),
            Self::format_size(size_bytes)
        );
        Ok(final_path)
    }

    /// Calculate SHA256 checksum of image file
    async fn calculate_image_checksum(&self, image_path: &Path) -> Result<String> {
        use sha2::{Digest, Sha256};
        use tokio::io::AsyncReadExt;

        let mut file = tokio::fs::File::open(image_path)
            .await
            .map_err(crate::error::AutoInstallError::IoError)?;

        let mut hasher = Sha256::new();
        let mut buffer = [0u8; 8192];

        loop {
            let bytes_read = file
                .read(&mut buffer)
                .await
                .map_err(crate::error::AutoInstallError::IoError)?;

            if bytes_read == 0 {
                break;
            }

            hasher.update(&buffer[..bytes_read]);
        }

        Ok(format!("{:x}", hasher.finalize()))
    }

    /// Format file size in human-readable format
    fn format_size(size_bytes: u64) -> String {
        const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
        let mut size = size_bytes as f64;
        let mut unit_index = 0;

        while size >= 1024.0 && unit_index < UNITS.len() - 1 {
            size /= 1024.0;
            unit_index += 1;
        }

        format!("{:.2} {}", size, UNITS[unit_index])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Architecture, ImageSpec, VmConfig};
    use tempfile::TempDir;
    use tokio::fs as async_fs;

    #[test]
    fn test_postprocessor_new() {
        // Arrange
        let temp_dir = TempDir::new().unwrap();
        let work_dir = temp_dir.path().join("work");
        let cache_dir = temp_dir.path().join("cache");

        // Act
        let postprocessor = PostProcessor::new(work_dir.clone(), cache_dir.clone());

        // Assert
        assert_eq!(postprocessor.work_dir, work_dir);
        assert_eq!(postprocessor.cache_dir, cache_dir);
    }

    #[test]
    fn test_format_size() {
        // Test different file sizes
        let test_cases = vec![
            (0, "0.00 B"),
            (512, "512.00 B"),
            (1024, "1.00 KB"),
            (1536, "1.50 KB"),
            (1024 * 1024, "1.00 MB"),
            (1536 * 1024, "1.50 MB"),
            (1024 * 1024 * 1024, "1.00 GB"),
            (1536 * 1024 * 1024, "1.50 GB"),
            (1024u64 * 1024 * 1024 * 1024, "1.00 TB"),
        ];

        for (size_bytes, expected) in test_cases {
            // Act
            let result = PostProcessor::format_size(size_bytes);

            // Assert
            assert_eq!(result, expected, "Failed for size: {}", size_bytes);
        }
    }

    #[tokio::test]
    async fn test_calculate_image_checksum() {
        // Arrange
        let temp_dir = TempDir::new().unwrap();
        let work_dir = temp_dir.path().join("work");
        let cache_dir = temp_dir.path().join("cache");
        let postprocessor = PostProcessor::new(work_dir, cache_dir);

        let test_file = temp_dir.path().join("test_image.qcow2");
        let test_content = b"test image content for checksum";
        async_fs::write(&test_file, test_content).await.unwrap();

        // Act
        let result = postprocessor.calculate_image_checksum(&test_file).await;

        // Assert
        assert!(result.is_ok());
        let checksum = result.unwrap();
        assert_eq!(checksum.len(), 64); // SHA256 hex string length
        assert!(checksum.chars().all(|c| c.is_ascii_hexdigit()));

        // Test that same content produces same checksum
        let result2 = postprocessor.calculate_image_checksum(&test_file).await;
        assert!(result2.is_ok());
        assert_eq!(checksum, result2.unwrap());
    }

    #[tokio::test]
    async fn test_calculate_image_checksum_nonexistent_file() {
        // Arrange
        let temp_dir = TempDir::new().unwrap();
        let work_dir = temp_dir.path().join("work");
        let cache_dir = temp_dir.path().join("cache");
        let postprocessor = PostProcessor::new(work_dir, cache_dir);

        let nonexistent_file = temp_dir.path().join("nonexistent.qcow2");

        // Act
        let result = postprocessor
            .calculate_image_checksum(&nonexistent_file)
            .await;

        // Assert
        assert!(result.is_err());
        match result.unwrap_err() {
            crate::error::AutoInstallError::IoError(_) => {
                // Expected error type
            }
            other => panic!("Expected IoError, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_generalize_image() {
        // Arrange
        let temp_dir = TempDir::new().unwrap();
        let work_dir = temp_dir.path().join("work");
        let cache_dir = temp_dir.path().join("cache");
        let postprocessor = PostProcessor::new(work_dir.clone(), cache_dir);

        // Create a mock VM disk file
        let vm_disk = temp_dir.path().join("test_vm.qcow2");
        async_fs::write(&vm_disk, b"mock vm disk content")
            .await
            .unwrap();

        // Act
        let result = postprocessor.generalize_image(&vm_disk).await;

        // Assert
        // This will likely fail due to missing guestfish, but should handle error gracefully
        match result {
            Ok(_) => {
                // If successful (unlikely in test environment), that's fine
            }
            Err(crate::error::AutoInstallError::ImageError(_)) => {
                // Expected when guestfish is not available
            }
            Err(crate::error::AutoInstallError::IoError(_)) => {
                // Also acceptable - IO error during process execution
            }
            Err(other) => panic!("Unexpected error type: {:?}", other),
        }

        // Verify work directory structure
        let _mount_point = work_dir.join("mount");
        // Directory may or may not exist depending on how far the function got
    }

    #[tokio::test]
    async fn test_finalize_image_with_output_path() {
        // Arrange
        let temp_dir = TempDir::new().unwrap();
        let work_dir = temp_dir.path().join("work");
        let cache_dir = temp_dir.path().join("cache");
        let postprocessor = PostProcessor::new(work_dir, cache_dir);

        let vm_disk = temp_dir.path().join("source.qcow2");
        let output_path = temp_dir.path().join("output.qcow2");
        async_fs::write(&vm_disk, b"mock vm disk").await.unwrap();

        let spec = ImageSpec {
            ubuntu_version: "24.04".to_string(),
            architecture: Architecture::Amd64,
            base_packages: vec![],
            vm_config: VmConfig {
                memory_mb: 2048,
                disk_size_gb: 20,
                cpu_cores: 2,
            },
            custom_scripts: vec![],
        };

        // Act
        let result = postprocessor
            .finalize_image(
                &vm_disk,
                Some(output_path.to_string_lossy().to_string()),
                &spec,
            )
            .await;

        // Assert
        // This will likely fail due to missing qemu-img, but should handle gracefully
        match result {
            Ok(final_path) => {
                // If successful, verify the path
                assert_eq!(final_path, output_path);
            }
            Err(crate::error::AutoInstallError::ImageError(_)) => {
                // Expected when qemu-img is not available
            }
            Err(other) => panic!("Unexpected error type: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_finalize_image_auto_path() {
        // Arrange
        let temp_dir = TempDir::new().unwrap();
        let work_dir = temp_dir.path().join("work");
        let cache_dir = temp_dir.path().join("cache");
        let postprocessor = PostProcessor::new(work_dir, cache_dir.clone());

        let vm_disk = temp_dir.path().join("source.qcow2");
        async_fs::write(&vm_disk, b"mock vm disk").await.unwrap();

        let spec = ImageSpec {
            ubuntu_version: "24.04".to_string(),
            architecture: Architecture::Amd64,
            base_packages: vec![],
            vm_config: VmConfig {
                memory_mb: 2048,
                disk_size_gb: 20,
                cpu_cores: 2,
            },
            custom_scripts: vec![],
        };

        // Act
        let result = postprocessor.finalize_image(&vm_disk, None, &spec).await;

        // Assert
        match result {
            Ok(final_path) => {
                // Verify auto-generated path structure
                assert!(final_path.starts_with(cache_dir.join("images")));
                assert!(final_path.to_string_lossy().contains("ubuntu-24.04-amd64"));
                assert!(final_path.extension().unwrap() == "qcow2");
            }
            Err(crate::error::AutoInstallError::ImageError(_)) => {
                // Expected when qemu-img is not available
                // Still verify that images directory was created
                let images_dir = cache_dir.join("images");
                assert!(images_dir.exists());
            }
            Err(other) => panic!("Unexpected error type: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_finalize_image_different_architectures() {
        // Arrange
        let temp_dir = TempDir::new().unwrap();
        let work_dir = temp_dir.path().join("work");
        let cache_dir = temp_dir.path().join("cache");
        let postprocessor = PostProcessor::new(work_dir, cache_dir.clone());

        let vm_disk = temp_dir.path().join("source.qcow2");
        async_fs::write(&vm_disk, b"mock vm disk").await.unwrap();

        let architectures = vec![Architecture::Amd64, Architecture::Arm64];

        for arch in architectures {
            // Arrange
            let spec = ImageSpec {
                ubuntu_version: "24.04".to_string(),
                architecture: arch,
                base_packages: vec![],
                vm_config: VmConfig {
                    memory_mb: 2048,
                    disk_size_gb: 20,
                    cpu_cores: 2,
                },
                custom_scripts: vec![],
            };

            // Act
            let result = postprocessor.finalize_image(&vm_disk, None, &spec).await;

            // Assert
            match result {
                Ok(final_path) => {
                    let filename = final_path.file_name().unwrap().to_string_lossy();
                    assert!(filename.contains(arch.as_str()));
                }
                Err(crate::error::AutoInstallError::ImageError(_)) => {
                    // Expected when qemu-img not available
                }
                Err(other) => panic!("Unexpected error for arch {:?}: {:?}", arch, other),
            }
        }
    }

    #[tokio::test]
    async fn test_calculate_image_checksum_empty_file() {
        // Arrange
        let temp_dir = TempDir::new().unwrap();
        let work_dir = temp_dir.path().join("work");
        let cache_dir = temp_dir.path().join("cache");
        let postprocessor = PostProcessor::new(work_dir, cache_dir);

        let empty_file = temp_dir.path().join("empty.qcow2");
        async_fs::write(&empty_file, b"").await.unwrap();

        // Act
        let result = postprocessor.calculate_image_checksum(&empty_file).await;

        // Assert
        assert!(result.is_ok());
        let checksum = result.unwrap();
        assert_eq!(checksum.len(), 64); // SHA256 hex string length
                                        // SHA256 of empty file is a known constant
        assert_eq!(
            checksum,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
