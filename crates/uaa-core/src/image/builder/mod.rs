// file: crates/uaa-core/src/image/builder/mod.rs
// version: 1.0.2
// guid: e1e2e3e4-f5f6-7890-1234-567890efghij

//! Modular image builder implementation

use crate::config::ImageSpec;
use crate::utils::VmManager;
use crate::Result;
use std::path::PathBuf;
use tokio::fs;
use tracing::{debug, info};

mod cloudinit;
mod disk;
mod iso;
mod postprocess;

use cloudinit::CloudInitManager;
use disk::DiskManager;
use iso::IsoManager;
use postprocess::PostProcessor;

/// Golden image builder using QEMU/KVM
pub struct ImageBuilder {
    vm_manager: VmManager,
    work_dir: PathBuf,
    cache_dir: PathBuf,
}

impl ImageBuilder {
    /// Create a new image builder with default cache directory
    pub fn new() -> Self {
        let default_cache = dirs::cache_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join("ubuntu-autoinstall");

        Self {
            vm_manager: VmManager::new(),
            work_dir: default_cache.join("work"),
            cache_dir: default_cache,
        }
    }

    /// Create a new image builder with custom cache directory
    pub fn with_cache_dir<P: AsRef<std::path::Path>>(cache_dir: P) -> Self {
        let cache_path = cache_dir.as_ref().to_path_buf();
        Self {
            vm_manager: VmManager::new(),
            work_dir: cache_path.join("work"),
            cache_dir: cache_path,
        }
    }

    /// Create a golden image from specification
    pub async fn create_image(
        &mut self,
        spec: ImageSpec,
        output_path: Option<String>,
    ) -> Result<PathBuf> {
        info!(
            "Creating Ubuntu {} image for {}",
            spec.ubuntu_version,
            spec.architecture.as_str()
        );

        // Create working directory
        self.setup_work_dir().await?;

        // Initialize managers
        let iso_manager = IsoManager::new(self.cache_dir.clone());
        let disk_manager = DiskManager::new(self.work_dir.clone());
        let cloudinit_manager = CloudInitManager::new(self.work_dir.clone());
        let postprocessor = PostProcessor::new(self.work_dir.clone(), self.cache_dir.clone());

        // Download Ubuntu netboot files
        let netboot_dir = iso_manager.get_ubuntu_iso(&spec).await?;

        // Create VM disk
        let vm_disk = disk_manager.get_vm_disk_path();
        disk_manager
            .create_qemu_disk(&vm_disk, spec.vm_config.disk_size_gb)
            .await?;

        // Create cloud-init config for automated installation
        let cloud_init_path = cloudinit_manager.create_cloud_init_config(&spec).await?;

        // Start VM and perform installation with signal handling
        info!("Creating VM and installing Ubuntu");

        let vm_installation = self.vm_manager.install_ubuntu_in_vm(
            &vm_disk,
            &netboot_dir,
            &cloud_init_path,
            spec.vm_config.memory_mb,
        );

        let installation_result = tokio::select! {
            result = vm_installation => result,
            _ = tokio::signal::ctrl_c() => {
                // If interrupted, cleanup VM and return error
                let _ = self.vm_manager.kill_qemu().await;
                return Err(crate::error::AutoInstallError::VmError(
                    "VM installation interrupted by user".to_string()
                ));
            }
        };

        installation_result?;

        // Generalize the image (remove machine-specific data)
        postprocessor.generalize_image(&vm_disk).await?;

        // Compress and finalize image
        let final_path = postprocessor
            .finalize_image(&vm_disk, output_path, &spec)
            .await?;

        // Cleanup
        self.cleanup_work_dir().await?;

        info!("Image creation completed: {}", final_path.display());
        Ok(final_path)
    }

    /// Set up working directory
    async fn setup_work_dir(&self) -> Result<()> {
        // Create both work and cache directories
        fs::create_dir_all(&self.work_dir)
            .await
            .map_err(crate::error::AutoInstallError::IoError)?;
        fs::create_dir_all(&self.cache_dir)
            .await
            .map_err(crate::error::AutoInstallError::IoError)?;

        debug!("Work directory created: {}", self.work_dir.display());
        debug!("Cache directory: {}", self.cache_dir.display());
        Ok(())
    }

    /// Cleanup working directory
    async fn cleanup_work_dir(&self) -> Result<()> {
        if self.work_dir.exists() {
            fs::remove_dir_all(&self.work_dir)
                .await
                .map_err(crate::error::AutoInstallError::IoError)?;
            debug!("Cleaned up work directory");
        }
        Ok(())
    }
}

impl Default for ImageBuilder {
    fn default() -> Self {
        Self::new()
    }
}
