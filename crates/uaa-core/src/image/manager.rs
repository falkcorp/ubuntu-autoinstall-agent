// file: crates/uaa-core/src/image/manager.rs
// version: 1.0.3
// guid: n4o5p6q7-r8s9-0123-4567-890123nopqrs

//! Image lifecycle management

use crate::{
    config::{Architecture, ImageInfo},
    Result,
};
use std::path::{Path, PathBuf};
use tokio::fs;
use tracing::{debug, info, warn};

/// Manager for golden image lifecycle
pub struct ImageManager {
    images_dir: PathBuf,
}

impl ImageManager {
    /// Create a new image manager
    pub fn new() -> Self {
        Self {
            images_dir: PathBuf::from("/var/lib/ubuntu-autoinstall/images"),
        }
    }

    /// Create image manager with custom directory
    pub fn with_images_dir<P: Into<PathBuf>>(images_dir: P) -> Self {
        Self {
            images_dir: images_dir.into(),
        }
    }

    /// List all available images
    pub async fn list_images(&self, filter_arch: Option<Architecture>) -> Result<Vec<ImageInfo>> {
        self.ensure_images_dir().await?;

        let mut images = Vec::new();
        let mut dir = fs::read_dir(&self.images_dir)
            .await
            .map_err(crate::error::AutoInstallError::IoError)?;

        while let Some(entry) = dir
            .next_entry()
            .await
            .map_err(crate::error::AutoInstallError::IoError)?
        {
            if let Some(name) = entry.file_name().to_str() {
                if name.ends_with(".json") {
                    if let Ok(image_info) = self.load_image_info(&entry.path()).await {
                        if let Some(arch_filter) = filter_arch {
                            if image_info.architecture == arch_filter {
                                images.push(image_info);
                            }
                        } else {
                            images.push(image_info);
                        }
                    }
                }
            }
        }

        // Sort by creation date (newest first)
        images.sort_by_key(|b| std::cmp::Reverse(b.created_at));

        Ok(images)
    }

    /// Find images older than specified days
    pub async fn find_old_images(&self, days: u32) -> Result<Vec<ImageInfo>> {
        let all_images = self.list_images(None).await?;
        let cutoff = chrono::Utc::now() - chrono::Duration::days(days as i64);

        Ok(all_images
            .into_iter()
            .filter(|img| img.created_at < cutoff)
            .collect())
    }

    /// Validate image integrity
    pub async fn validate_image<P: AsRef<Path>>(&self, image_path: P) -> Result<bool> {
        let path = image_path.as_ref();

        // Check if file exists
        if !path.exists() {
            warn!("Image file does not exist: {}", path.display());
            return Ok(false);
        }

        // Check file size
        let metadata = fs::metadata(path)
            .await
            .map_err(crate::error::AutoInstallError::IoError)?;

        if metadata.len() == 0 {
            warn!("Image file is empty: {}", path.display());
            return Ok(false);
        }

        // Validate QEMU image format
        let output = tokio::process::Command::new("qemu-img")
            .args(["info", path.to_str().unwrap()])
            .output()
            .await
            .map_err(|e| {
                crate::error::AutoInstallError::ImageError(format!(
                    "Failed to validate image with qemu-img: {}",
                    e
                ))
            })?;

        if !output.status.success() {
            warn!("qemu-img validation failed for: {}", path.display());
            return Ok(false);
        }

        // Check for basic image structure
        let info_output = String::from_utf8_lossy(&output.stdout);
        if !info_output.contains("file format: qcow2") {
            warn!("Image is not in qcow2 format: {}", path.display());
            return Ok(false);
        }

        info!("Image validation successful: {}", path.display());
        Ok(true)
    }

    /// Register a new image
    pub async fn register_image(&self, image_info: ImageInfo) -> Result<()> {
        self.ensure_images_dir().await?;

        let info_path = self.images_dir.join(format!("{}.json", image_info.id));
        let json_data = serde_json::to_string_pretty(&image_info)?;

        fs::write(&info_path, json_data)
            .await
            .map_err(crate::error::AutoInstallError::IoError)?;

        info!(
            "Registered image: {} ({})",
            image_info.id, image_info.ubuntu_version
        );
        Ok(())
    }

    /// Remove image and its metadata
    pub async fn remove_image(&self, image_id: &str) -> Result<()> {
        let info_path = self.images_dir.join(format!("{}.json", image_id));

        if let Ok(image_info) = self.load_image_info(&info_path).await {
            // Remove image file
            if image_info.path.exists() {
                fs::remove_file(&image_info.path)
                    .await
                    .map_err(crate::error::AutoInstallError::IoError)?;
                debug!("Removed image file: {}", image_info.path.display());
            }

            // Remove metadata file
            fs::remove_file(&info_path)
                .await
                .map_err(crate::error::AutoInstallError::IoError)?;
            debug!("Removed image metadata: {}", info_path.display());

            info!("Removed image: {}", image_id);
        } else {
            warn!("Image metadata not found: {}", image_id);
        }

        Ok(())
    }

    /// Cleanup old images
    pub async fn cleanup_images(&self, images_to_delete: Vec<ImageInfo>) -> Result<usize> {
        let mut deleted_count = 0;

        for image in images_to_delete {
            match self.remove_image(&image.id).await {
                Ok(()) => deleted_count += 1,
                Err(e) => warn!("Failed to delete image {}: {}", image.id, e),
            }
        }

        Ok(deleted_count)
    }

    /// Calculate total disk usage of all images
    pub async fn calculate_total_usage(&self) -> Result<u64> {
        let images = self.list_images(None).await?;
        Ok(images.iter().map(|img| img.size_bytes).sum())
    }

    /// Get image by ID
    pub async fn get_image(&self, image_id: &str) -> Result<Option<ImageInfo>> {
        let info_path = self.images_dir.join(format!("{}.json", image_id));

        if info_path.exists() {
            Ok(Some(self.load_image_info(&info_path).await?))
        } else {
            Ok(None)
        }
    }

    /// Load image info from JSON file
    async fn load_image_info<P: AsRef<Path>>(&self, path: P) -> Result<ImageInfo> {
        let content = fs::read_to_string(path)
            .await
            .map_err(crate::error::AutoInstallError::IoError)?;

        let image_info: ImageInfo = serde_json::from_str(&content).map_err(|e| {
            crate::error::AutoInstallError::ImageError(format!(
                "Failed to parse image metadata: {}",
                e
            ))
        })?;

        Ok(image_info)
    }

    /// Ensure images directory exists
    async fn ensure_images_dir(&self) -> Result<()> {
        if !self.images_dir.exists() {
            fs::create_dir_all(&self.images_dir)
                .await
                .map_err(crate::error::AutoInstallError::IoError)?;
            debug!("Created images directory: {}", self.images_dir.display());
        }
        Ok(())
    }

    /// Calculate checksum of image file
    pub async fn calculate_checksum<P: AsRef<Path>>(&self, path: P) -> Result<String> {
        use sha2::{Digest, Sha256};
        use tokio::io::AsyncReadExt;

        let mut file = fs::File::open(path)
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
}

impl Default for ImageManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Architecture;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_image_manager_creation() {
        let temp_dir = TempDir::new().unwrap();
        let manager = ImageManager::with_images_dir(temp_dir.path());

        let images = manager.list_images(None).await.unwrap();
        assert_eq!(images.len(), 0);
    }

    #[tokio::test]
    async fn test_register_and_list_images() -> Result<()> {
        let temp_dir = TempDir::new().unwrap();
        let manager = ImageManager::with_images_dir(temp_dir.path());

        let image_info = ImageInfo::new(
            "24.04".to_string(),
            Architecture::Amd64,
            1024 * 1024, // 1MB
            "abc123".to_string(),
            PathBuf::from("/tmp/test.qcow2"),
        );

        manager.register_image(image_info.clone()).await?;

        let images = manager.list_images(None).await?;
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].ubuntu_version, "24.04");
        assert_eq!(images[0].architecture, Architecture::Amd64);

        Ok(())
    }

    #[tokio::test]
    async fn test_filter_by_architecture() -> Result<()> {
        let temp_dir = TempDir::new().unwrap();
        let manager = ImageManager::with_images_dir(temp_dir.path());

        // Register amd64 image
        let amd64_image = ImageInfo::new(
            "24.04".to_string(),
            Architecture::Amd64,
            1024 * 1024,
            "abc123".to_string(),
            PathBuf::from("/tmp/amd64.qcow2"),
        );
        manager.register_image(amd64_image).await?;

        // Register arm64 image
        let arm64_image = ImageInfo::new(
            "24.04".to_string(),
            Architecture::Arm64,
            1024 * 1024,
            "def456".to_string(),
            PathBuf::from("/tmp/arm64.qcow2"),
        );
        manager.register_image(arm64_image).await?;

        // Test filtering
        let all_images = manager.list_images(None).await?;
        assert_eq!(all_images.len(), 2);

        let amd64_images = manager.list_images(Some(Architecture::Amd64)).await?;
        assert_eq!(amd64_images.len(), 1);
        assert_eq!(amd64_images[0].architecture, Architecture::Amd64);

        let arm64_images = manager.list_images(Some(Architecture::Arm64)).await?;
        assert_eq!(arm64_images.len(), 1);
        assert_eq!(arm64_images[0].architecture, Architecture::Arm64);

        Ok(())
    }
}
