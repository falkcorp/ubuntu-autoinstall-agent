// file: crates/uaa-core/src/utils/disk.rs
// version: 1.0.2
// guid: x4y5z6a7-b8c9-0123-4567-890123xyzabc

//! Disk utility functions

use crate::Result;
use std::path::Path;
use tokio::process::Command;
use tracing::{debug, warn};

/// Disk utility functions
pub struct DiskUtils;

impl DiskUtils {
    /// Check if a disk device exists and is accessible
    pub async fn device_exists(device: &str) -> bool {
        Path::new(device).exists()
    }

    /// Get disk size in GB
    pub async fn get_disk_size(device: &str) -> Result<u64> {
        let output = Command::new("lsblk")
            .args(["-bno", "SIZE", device])
            .output()
            .await
            .map_err(|e| {
                crate::error::AutoInstallError::DiskError(format!(
                    "Failed to get disk size for {}: {}",
                    device, e
                ))
            })?;

        if !output.status.success() {
            return Err(crate::error::AutoInstallError::DiskError(format!(
                "lsblk failed for device {}",
                device
            )));
        }

        let size_str = String::from_utf8_lossy(&output.stdout);
        let size_bytes: u64 = size_str.trim().parse().map_err(|_| {
            crate::error::AutoInstallError::DiskError(format!(
                "Failed to parse disk size: {}",
                size_str
            ))
        })?;

        Ok(size_bytes / (1024 * 1024 * 1024)) // Convert to GB
    }

    /// Check if device is mounted
    pub async fn is_mounted(device: &str) -> Result<bool> {
        let output = Command::new("findmnt")
            .args(["-S", device])
            .output()
            .await
            .map_err(|e| {
                crate::error::AutoInstallError::DiskError(format!(
                    "Failed to check mount status for {}: {}",
                    device, e
                ))
            })?;

        Ok(output.status.success())
    }

    /// Unmount device if mounted
    pub async fn unmount_device(device: &str) -> Result<()> {
        if Self::is_mounted(device).await? {
            debug!("Unmounting device: {}", device);

            let output = Command::new("umount")
                .arg(device)
                .output()
                .await
                .map_err(|e| {
                    crate::error::AutoInstallError::DiskError(format!(
                        "Failed to unmount {}: {}",
                        device, e
                    ))
                })?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(crate::error::AutoInstallError::DiskError(format!(
                    "Failed to unmount {}: {}",
                    device, stderr
                )));
            }
        }

        Ok(())
    }

    /// Wipe disk (remove all partitions and data)
    pub async fn wipe_disk(device: &str) -> Result<()> {
        warn!("Wiping disk: {} - ALL DATA WILL BE LOST", device);

        // Use wipefs to remove filesystem signatures
        let output = Command::new("wipefs")
            .args(["-af", device])
            .output()
            .await
            .map_err(|e| {
                crate::error::AutoInstallError::DiskError(format!(
                    "Failed to wipe {}: {}",
                    device, e
                ))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(crate::error::AutoInstallError::DiskError(format!(
                "Failed to wipe {}: {}",
                device, stderr
            )));
        }

        debug!("Disk wiped successfully: {}", device);
        Ok(())
    }

    /// Create partition table on device
    pub async fn create_partition_table(device: &str, table_type: &str) -> Result<()> {
        debug!("Creating {} partition table on {}", table_type, device);

        let output = Command::new("parted")
            .args(["-s", device, "mklabel", table_type])
            .output()
            .await
            .map_err(|e| {
                crate::error::AutoInstallError::DiskError(format!(
                    "Failed to create partition table on {}: {}",
                    device, e
                ))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(crate::error::AutoInstallError::DiskError(format!(
                "Failed to create partition table on {}: {}",
                device, stderr
            )));
        }

        debug!("Partition table created successfully on {}", device);
        Ok(())
    }

    /// Create partition on device
    pub async fn create_partition(
        device: &str,
        start: &str,
        end: &str,
        fs_type: &str,
    ) -> Result<String> {
        debug!(
            "Creating partition on {} from {} to {} with filesystem {}",
            device, start, end, fs_type
        );

        let output = Command::new("parted")
            .args(["-s", device, "mkpart", "primary", fs_type, start, end])
            .output()
            .await
            .map_err(|e| {
                crate::error::AutoInstallError::DiskError(format!(
                    "Failed to create partition on {}: {}",
                    device, e
                ))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(crate::error::AutoInstallError::DiskError(format!(
                "Failed to create partition on {}: {}",
                device, stderr
            )));
        }

        // Get the new partition device name
        let partition_device = format!("{}1", device);

        debug!("Partition created successfully: {}", partition_device);
        Ok(partition_device)
    }

    /// Format partition with filesystem
    pub async fn format_partition(device: &str, fs_type: &str) -> Result<()> {
        debug!("Formatting {} with {} filesystem", device, fs_type);

        let mkfs_cmd = match fs_type {
            "ext4" => "mkfs.ext4",
            "ext3" => "mkfs.ext3",
            "xfs" => "mkfs.xfs",
            "btrfs" => "mkfs.btrfs",
            _ => {
                return Err(crate::error::AutoInstallError::DiskError(format!(
                    "Unsupported filesystem type: {}",
                    fs_type
                )));
            }
        };

        let output = Command::new(mkfs_cmd)
            .args(["-F", device])
            .output()
            .await
            .map_err(|e| {
                crate::error::AutoInstallError::DiskError(format!(
                    "Failed to format {}: {}",
                    device, e
                ))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(crate::error::AutoInstallError::DiskError(format!(
                "Failed to format {}: {}",
                device, stderr
            )));
        }

        debug!("Partition formatted successfully: {}", device);
        Ok(())
    }

    /// Get disk information
    pub async fn get_disk_info(device: &str) -> Result<DiskInfo> {
        let output = Command::new("lsblk")
            .args(["-bJo", "NAME,SIZE,TYPE,MOUNTPOINT,FSTYPE", device])
            .output()
            .await
            .map_err(|e| {
                crate::error::AutoInstallError::DiskError(format!(
                    "Failed to get disk info for {}: {}",
                    device, e
                ))
            })?;

        if !output.status.success() {
            return Err(crate::error::AutoInstallError::DiskError(format!(
                "lsblk failed for device {}",
                device
            )));
        }

        let json_str = String::from_utf8_lossy(&output.stdout);
        let lsblk_output: serde_json::Value = serde_json::from_str(&json_str).map_err(|e| {
            crate::error::AutoInstallError::DiskError(format!(
                "Failed to parse lsblk output: {}",
                e
            ))
        })?;

        let blockdevices = lsblk_output["blockdevices"].as_array().ok_or_else(|| {
            crate::error::AutoInstallError::DiskError(
                "No block devices found in lsblk output".to_string(),
            )
        })?;

        if blockdevices.is_empty() {
            return Err(crate::error::AutoInstallError::DiskError(format!(
                "No information found for device {}",
                device
            )));
        }

        let device_info = &blockdevices[0];

        Ok(DiskInfo {
            name: device_info["name"]
                .as_str()
                .unwrap_or("unknown")
                .to_string(),
            size_bytes: device_info["size"]
                .as_str()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0),
            device_type: device_info["type"]
                .as_str()
                .unwrap_or("unknown")
                .to_string(),
            mount_point: device_info["mountpoint"].as_str().map(|s| s.to_string()),
            filesystem: device_info["fstype"].as_str().map(|s| s.to_string()),
        })
    }
}

/// Disk information structure
#[derive(Debug, Clone)]
pub struct DiskInfo {
    pub name: String,
    pub size_bytes: u64,
    pub device_type: String,
    pub mount_point: Option<String>,
    pub filesystem: Option<String>,
}

impl DiskInfo {
    /// Get human-readable size
    pub fn size_human(&self) -> String {
        const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
        let mut size = self.size_bytes as f64;
        let mut unit_index = 0;

        while size >= 1024.0 && unit_index < UNITS.len() - 1 {
            size /= 1024.0;
            unit_index += 1;
        }

        format!("{:.2} {}", size, UNITS[unit_index])
    }

    /// Check if device is mounted
    pub fn is_mounted(&self) -> bool {
        self.mount_point.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_device_exists() {
        // Test with a device that should exist
        assert!(DiskUtils::device_exists("/dev/null").await);

        // Test with a device that shouldn't exist
        assert!(!DiskUtils::device_exists("/dev/nonexistent123").await);
    }

    #[test]
    fn test_disk_info_size_human() {
        let info = DiskInfo {
            name: "test".to_string(),
            size_bytes: 1024 * 1024 * 1024, // 1 GB
            device_type: "disk".to_string(),
            mount_point: None,
            filesystem: None,
        };

        assert_eq!(info.size_human(), "1.00 GB");
    }

    #[test]
    fn test_disk_info_is_mounted() {
        let mounted_info = DiskInfo {
            name: "test".to_string(),
            size_bytes: 0,
            device_type: "disk".to_string(),
            mount_point: Some("/mnt/test".to_string()),
            filesystem: Some("ext4".to_string()),
        };

        let unmounted_info = DiskInfo {
            name: "test".to_string(),
            size_bytes: 0,
            device_type: "disk".to_string(),
            mount_point: None,
            filesystem: None,
        };

        assert!(mounted_info.is_mounted());
        assert!(!unmounted_info.is_mounted());
    }
}
