// file: crates/uaa-core/src/utils/qemu.rs
// version: 1.0.3
// guid: h9i0j1k2-l3m4-5678-9012-345678hijklm

//! QEMU image utilities

use crate::Result;
use std::path::Path;
use tokio::process::Command;
use tracing::{debug, info};

/// QEMU image utilities
pub struct QemuUtils;

impl QemuUtils {
    /// Get image information using qemu-img info
    pub async fn get_image_info<P: AsRef<Path>>(image_path: P) -> Result<ImageInfo> {
        let output = Command::new("qemu-img")
            .args([
                "info",
                "--output=json",
                image_path.as_ref().to_str().unwrap(),
            ])
            .output()
            .await
            .map_err(|e| {
                crate::error::AutoInstallError::ImageError(format!(
                    "Failed to get image info: {}",
                    e
                ))
            })?;

        if !output.status.success() {
            return Err(crate::error::AutoInstallError::ImageError(format!(
                "qemu-img info failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        let info_str = String::from_utf8_lossy(&output.stdout);
        let info_json: serde_json::Value = serde_json::from_str(&info_str).map_err(|e| {
            crate::error::AutoInstallError::ImageError(format!(
                "Failed to parse qemu-img output: {}",
                e
            ))
        })?;

        Ok(ImageInfo {
            format: info_json["format"]
                .as_str()
                .unwrap_or("unknown")
                .to_string(),
            virtual_size: info_json["virtual-size"].as_u64().unwrap_or(0),
            actual_size: info_json["actual-size"].as_u64().unwrap_or(0),
            cluster_size: info_json["cluster-size"].as_u64(),
            compressed: info_json["compressed"].as_bool().unwrap_or(false),
        })
    }

    /// Convert image to raw format for extraction
    pub async fn convert_to_raw<P: AsRef<Path>>(qcow2_path: P, raw_path: P) -> Result<()> {
        info!("Converting QCOW2 image to raw format for extraction");

        let output = Command::new("qemu-img")
            .args([
                "convert",
                "-f",
                "qcow2",
                "-O",
                "raw",
                qcow2_path.as_ref().to_str().unwrap(),
                raw_path.as_ref().to_str().unwrap(),
            ])
            .output()
            .await
            .map_err(|e| {
                crate::error::AutoInstallError::ImageError(format!(
                    "Failed to convert image: {}",
                    e
                ))
            })?;

        if !output.status.success() {
            return Err(crate::error::AutoInstallError::ImageError(format!(
                "Image conversion failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        debug!("Image conversion completed");
        Ok(())
    }

    /// Mount raw image using loop device
    pub async fn mount_raw_image<P: AsRef<Path>>(raw_path: P, mount_point: P) -> Result<String> {
        // Create loop device
        let output = Command::new("losetup")
            .args(["-P", "-f", "--show", raw_path.as_ref().to_str().unwrap()])
            .output()
            .await
            .map_err(|e| {
                crate::error::AutoInstallError::ImageError(format!(
                    "Failed to create loop device: {}",
                    e
                ))
            })?;

        if !output.status.success() {
            return Err(crate::error::AutoInstallError::ImageError(format!(
                "Loop device creation failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        let loop_device = String::from_utf8_lossy(&output.stdout).trim().to_string();

        // Mount the filesystem (usually partition 1)
        let partition = format!("{}p1", loop_device);
        let output = Command::new("mount")
            .args([&partition, mount_point.as_ref().to_str().unwrap()])
            .output()
            .await
            .map_err(|e| {
                crate::error::AutoInstallError::ImageError(format!("Failed to mount image: {}", e))
            })?;

        if !output.status.success() {
            // Clean up loop device on mount failure
            let _ = Command::new("losetup")
                .args(["-d", &loop_device])
                .output()
                .await;
            return Err(crate::error::AutoInstallError::ImageError(format!(
                "Mount failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        info!(
            "Image mounted at {} via {}",
            mount_point.as_ref().display(),
            loop_device
        );
        Ok(loop_device)
    }

    /// Unmount image and clean up loop device
    pub async fn unmount_image<P: AsRef<Path>>(mount_point: P, loop_device: &str) -> Result<()> {
        // Unmount
        let output = Command::new("umount")
            .args([mount_point.as_ref().to_str().unwrap()])
            .output()
            .await
            .map_err(|e| {
                crate::error::AutoInstallError::ImageError(format!("Failed to unmount: {}", e))
            })?;

        if !output.status.success() {
            return Err(crate::error::AutoInstallError::ImageError(format!(
                "Unmount failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        // Remove loop device
        let output = Command::new("losetup")
            .args(["-d", loop_device])
            .output()
            .await
            .map_err(|e| {
                crate::error::AutoInstallError::ImageError(format!(
                    "Failed to remove loop device: {}",
                    e
                ))
            })?;

        if !output.status.success() {
            return Err(crate::error::AutoInstallError::ImageError(format!(
                "Loop device removal failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        debug!("Image unmounted and loop device cleaned up");
        Ok(())
    }

    /// Extract image contents to target directory
    pub async fn extract_image_contents<P: AsRef<Path>>(
        qcow2_path: P,
        target_dir: P,
    ) -> Result<()> {
        let temp_dir = tempfile::tempdir().map_err(crate::error::AutoInstallError::IoError)?;

        let raw_path = temp_dir.path().join("image.raw");
        let mount_point = temp_dir.path().join("mount");

        tokio::fs::create_dir_all(&mount_point)
            .await
            .map_err(crate::error::AutoInstallError::IoError)?;

        // Convert to raw
        Self::convert_to_raw(qcow2_path.as_ref(), &raw_path).await?;

        // Mount image
        let loop_device = Self::mount_raw_image(&raw_path, &mount_point).await?;

        // Copy contents
        let output = Command::new("rsync")
            .args([
                "-av",
                "--numeric-ids",
                &format!("{}/", mount_point.display()),
                target_dir.as_ref().to_str().unwrap(),
            ])
            .output()
            .await
            .map_err(|e| {
                crate::error::AutoInstallError::ImageError(format!(
                    "Failed to copy image contents: {}",
                    e
                ))
            })?;

        // Cleanup
        let _ = Self::unmount_image(&mount_point, &loop_device).await;

        if !output.status.success() {
            return Err(crate::error::AutoInstallError::ImageError(format!(
                "Content extraction failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        info!("Image contents extracted successfully");
        Ok(())
    }
}

/// QEMU image information
#[derive(Debug)]
pub struct ImageInfo {
    pub format: String,
    pub virtual_size: u64,
    pub actual_size: u64,
    pub cluster_size: Option<u64>,
    pub compressed: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use tokio::fs as async_fs;

    #[test]
    fn test_image_info_creation() {
        // Arrange & Act
        let image_info = ImageInfo {
            format: "qcow2".to_string(),
            virtual_size: 21474836480, // 20GB
            actual_size: 1073741824,   // 1GB
            cluster_size: Some(65536), // 64KB
            compressed: false,
        };

        // Assert
        assert_eq!(image_info.format, "qcow2");
        assert_eq!(image_info.virtual_size, 21474836480);
        assert_eq!(image_info.actual_size, 1073741824);
        assert_eq!(image_info.cluster_size, Some(65536));
        assert!(!image_info.compressed);
    }

    #[test]
    fn test_image_info_with_compression() {
        // Arrange & Act
        let image_info = ImageInfo {
            format: "qcow2".to_string(),
            virtual_size: 10737418240, // 10GB
            actual_size: 536870912,    // 512MB
            cluster_size: None,
            compressed: true,
        };

        // Assert
        assert_eq!(image_info.format, "qcow2");
        assert_eq!(image_info.virtual_size, 10737418240);
        assert_eq!(image_info.actual_size, 536870912);
        assert_eq!(image_info.cluster_size, None);
        assert!(image_info.compressed);
    }

    #[tokio::test]
    async fn test_get_image_info_nonexistent_file() {
        // Arrange
        let temp_dir = TempDir::new().unwrap();
        let nonexistent_file = temp_dir.path().join("nonexistent.qcow2");

        // Act
        let result = QemuUtils::get_image_info(&nonexistent_file).await;

        // Assert
        assert!(result.is_err());
        match result.unwrap_err() {
            crate::error::AutoInstallError::ImageError(_) => {
                // Expected error type when qemu-img fails
            }
            other => panic!("Expected ImageError, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_convert_to_raw_invalid_source() {
        // Arrange
        let temp_dir = TempDir::new().unwrap();
        let source = temp_dir.path().join("nonexistent.qcow2");
        let dest = temp_dir.path().join("output.raw");

        // Act
        let result = QemuUtils::convert_to_raw(&source, &dest).await;

        // Assert
        // Should fail gracefully when source doesn't exist
        assert!(result.is_err());
        match result.unwrap_err() {
            crate::error::AutoInstallError::ImageError(_) => {
                // Expected when qemu-img conversion fails
            }
            other => panic!("Expected ImageError, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_convert_to_raw_invalid_destination() {
        // Arrange
        let temp_dir = TempDir::new().unwrap();
        let source = temp_dir.path().join("test.qcow2");
        let invalid_dest = temp_dir.path().join("invalid").join("output.raw");

        // Create a mock source file
        async_fs::write(&source, b"mock qcow2 content")
            .await
            .unwrap();

        // Act
        let result = QemuUtils::convert_to_raw(&source, &invalid_dest).await;

        // Assert
        // Should fail when destination path is invalid
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_mount_raw_image_invalid_path() {
        // Arrange
        let temp_dir = TempDir::new().unwrap();
        let invalid_raw = temp_dir.path().join("nonexistent.raw");
        let mount_point = temp_dir.path().join("mount");
        async_fs::create_dir_all(&mount_point).await.unwrap();

        // Act
        let result = QemuUtils::mount_raw_image(&invalid_raw, &mount_point).await;

        // Assert
        // Should fail when trying to mount nonexistent file
        assert!(result.is_err());
        match result.unwrap_err() {
            crate::error::AutoInstallError::ImageError(_) => {
                // Expected when losetup fails
            }
            other => panic!("Expected ImageError, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_unmount_image_invalid_loop_device() {
        // Arrange
        let temp_dir = TempDir::new().unwrap();
        let mount_point = temp_dir.path().join("mount");
        async_fs::create_dir_all(&mount_point).await.unwrap();
        let invalid_loop_device = "/dev/loop999"; // Likely non-existent

        // Act
        let result = QemuUtils::unmount_image(&mount_point, invalid_loop_device).await;

        // Assert
        // Should fail when loop device doesn't exist
        assert!(result.is_err());
        match result.unwrap_err() {
            crate::error::AutoInstallError::ImageError(_) => {
                // Expected when umount/losetup fails
            }
            other => panic!("Expected ImageError, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_extract_image_contents_invalid_source() {
        // Arrange
        let temp_dir = TempDir::new().unwrap();
        let invalid_qcow2 = temp_dir.path().join("nonexistent.qcow2");
        let target_dir = temp_dir.path().join("target");
        async_fs::create_dir_all(&target_dir).await.unwrap();

        // Act
        let result = QemuUtils::extract_image_contents(&invalid_qcow2, &target_dir).await;

        // Assert
        // Should fail when source image doesn't exist
        assert!(result.is_err());
        match result.unwrap_err() {
            crate::error::AutoInstallError::ImageError(_) => {
                // Expected when conversion fails
            }
            crate::error::AutoInstallError::IoError(_) => {
                // Also acceptable - might fail at directory creation
            }
            other => panic!("Expected ImageError or IoError, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_extract_image_contents_invalid_target() {
        // Arrange
        let temp_dir = TempDir::new().unwrap();
        let mock_qcow2 = temp_dir.path().join("test.qcow2");
        let invalid_target = temp_dir
            .path()
            .join("invalid")
            .join("read-only")
            .join("path");

        // Create mock source file
        async_fs::write(&mock_qcow2, b"mock qcow2").await.unwrap();

        // Act
        let result = QemuUtils::extract_image_contents(&mock_qcow2, &invalid_target).await;

        // Assert
        // Should fail when target directory is invalid/read-only
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_get_image_info_with_mock_json() {
        // This test verifies JSON parsing logic without requiring qemu-img
        // Note: We can't easily mock Command execution, but we can test
        // the error handling paths

        // Arrange
        let temp_dir = TempDir::new().unwrap();
        let mock_image = temp_dir.path().join("mock.qcow2");
        async_fs::write(&mock_image, b"not a real qcow2")
            .await
            .unwrap();

        // Act
        let result = QemuUtils::get_image_info(&mock_image).await;

        // Assert
        // Should fail because it's not a real qcow2 file
        assert!(result.is_err());
        // The error should be an ImageError
        match result.unwrap_err() {
            crate::error::AutoInstallError::ImageError(_) => {
                // Expected - qemu-img will fail on invalid file
            }
            other => panic!("Expected ImageError, got: {:?}", other),
        }
    }

    #[test]
    fn test_image_info_debug_format() {
        // Arrange
        let image_info = ImageInfo {
            format: "qcow2".to_string(),
            virtual_size: 1073741824,
            actual_size: 536870912,
            cluster_size: Some(65536),
            compressed: false,
        };

        // Act
        let debug_str = format!("{:?}", image_info);

        // Assert
        assert!(debug_str.contains("qcow2"));
        assert!(debug_str.contains("1073741824"));
        assert!(debug_str.contains("536870912"));
        assert!(debug_str.contains("65536"));
        assert!(debug_str.contains("false"));
    }

    #[test]
    fn test_image_info_different_formats() {
        // Test different image format scenarios
        let formats = vec![
            ("qcow2", true),
            ("raw", false),
            ("vmdk", false),
            ("vdi", false),
        ];

        for (format, should_have_cluster) in formats {
            // Arrange & Act
            let image_info = ImageInfo {
                format: format.to_string(),
                virtual_size: 1073741824,
                actual_size: 536870912,
                cluster_size: if should_have_cluster {
                    Some(65536)
                } else {
                    None
                },
                compressed: format == "qcow2",
            };

            // Assert
            assert_eq!(image_info.format, format);
            if should_have_cluster {
                assert!(image_info.cluster_size.is_some());
            } else {
                assert!(image_info.cluster_size.is_none());
            }
            assert_eq!(image_info.compressed, format == "qcow2");
        }
    }
}
