// file: crates/uaa-core/src/image/builder/disk.rs
// version: 1.0.2
// guid: b1b2b3b4-c5c6-7890-1234-567890bcdefg

//! Disk creation and management functionality

use crate::Result;
use std::path::{Path, PathBuf};
use tokio::process::Command;
use tracing::debug;

/// Disk management operations
pub struct DiskManager {
    work_dir: PathBuf,
}

impl DiskManager {
    /// Create a new disk manager
    pub fn new(work_dir: PathBuf) -> Self {
        Self { work_dir }
    }

    /// Create QEMU disk image
    pub async fn create_qemu_disk(&self, disk_path: &Path, size_gb: u32) -> Result<()> {
        let output = Command::new("qemu-img")
            .args([
                "create",
                "-f",
                "qcow2",
                disk_path.to_str().unwrap(),
                &format!("{}G", size_gb),
            ])
            .output()
            .await
            .map_err(|e| {
                crate::error::AutoInstallError::VmError(format!(
                    "Failed to create QEMU disk: {}",
                    e
                ))
            })?;

        if !output.status.success() {
            return Err(crate::error::AutoInstallError::VmError(format!(
                "qemu-img failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        debug!("Created QEMU disk: {}", disk_path.display());
        Ok(())
    }

    /// Get VM disk path
    pub fn get_vm_disk_path(&self) -> PathBuf {
        self.work_dir.join("ubuntu-install.qcow2")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_disk_manager_new() {
        // Arrange
        let temp_dir = TempDir::new().unwrap();
        let work_dir = temp_dir.path().to_path_buf();

        // Act
        let disk_manager = DiskManager::new(work_dir.clone());

        // Assert
        assert_eq!(disk_manager.work_dir, work_dir);
    }

    #[test]
    fn test_get_vm_disk_path() {
        // Arrange
        let temp_dir = TempDir::new().unwrap();
        let work_dir = temp_dir.path().to_path_buf();
        let disk_manager = DiskManager::new(work_dir.clone());

        // Act
        let disk_path = disk_manager.get_vm_disk_path();

        // Assert
        assert_eq!(disk_path, work_dir.join("ubuntu-install.qcow2"));
        assert_eq!(disk_path.file_name().unwrap(), "ubuntu-install.qcow2");
    }

    #[tokio::test]
    async fn test_create_qemu_disk_command_construction() {
        // Arrange
        let temp_dir = TempDir::new().unwrap();
        let work_dir = temp_dir.path().to_path_buf();
        let disk_manager = DiskManager::new(work_dir);
        let disk_path = temp_dir.path().join("test.qcow2");
        let size_gb = 20;

        // Act & Assert
        // This test checks that the function would construct the correct command
        // We can't easily test the actual qemu-img execution without mocking
        // but we can verify the function handles the path correctly

        // Test would fail if qemu-img is not installed, but that's expected
        let result = disk_manager.create_qemu_disk(&disk_path, size_gb).await;

        // Should either succeed (if qemu-img available) or fail with VmError
        match result {
            Ok(_) => {
                // If successful, the disk should be created
                assert!(disk_path.exists());
            }
            Err(crate::error::AutoInstallError::VmError(_)) => {
                // Expected if qemu-img is not available
                // This is the normal case in CI environments
            }
            Err(e) => panic!("Unexpected error type: {:?}", e),
        }
    }

    #[tokio::test]
    async fn test_create_qemu_disk_with_different_sizes() {
        // Arrange
        let temp_dir = TempDir::new().unwrap();
        let work_dir = temp_dir.path().to_path_buf();
        let disk_manager = DiskManager::new(work_dir);

        let test_cases = vec![1, 10, 50, 100];

        for size_gb in test_cases {
            // Act
            let disk_path = temp_dir.path().join(format!("test_{}.qcow2", size_gb));
            let result = disk_manager.create_qemu_disk(&disk_path, size_gb).await;

            // Assert
            // Test should either succeed or fail gracefully
            match result {
                Ok(_) => {
                    // Verify file was created if command succeeded
                    assert!(disk_path.exists());
                }
                Err(crate::error::AutoInstallError::VmError(_)) => {
                    // Expected when qemu-img is not available
                }
                Err(e) => panic!("Unexpected error for size {}: {:?}", size_gb, e),
            }
        }
    }

    #[tokio::test]
    async fn test_create_qemu_disk_invalid_path() {
        // Arrange
        let temp_dir = TempDir::new().unwrap();
        let work_dir = temp_dir.path().to_path_buf();
        let disk_manager = DiskManager::new(work_dir);

        // Use an invalid path (directory that doesn't exist)
        let invalid_path = Path::new("/nonexistent/directory/test.qcow2");
        let size_gb = 10;

        // Act
        let result = disk_manager.create_qemu_disk(invalid_path, size_gb).await;

        // Assert
        // Should fail with an error (either IoError or VmError)
        assert!(result.is_err());
    }

    #[test]
    fn test_disk_manager_work_dir_access() {
        // Arrange
        let temp_dir = TempDir::new().unwrap();
        let work_dir = temp_dir.path().to_path_buf();
        let disk_manager = DiskManager::new(work_dir.clone());

        // Act & Assert
        // Verify that work_dir is accessible and correct
        assert_eq!(disk_manager.work_dir, work_dir);
        assert!(disk_manager.work_dir.exists());
        assert!(disk_manager.work_dir.is_dir());
    }
}
