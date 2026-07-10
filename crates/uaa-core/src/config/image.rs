// file: crates/uaa-core/src/config/image.rs
// version: 1.0.2
// guid: c3d4e5f6-g7h8-9012-3456-789012cdefgh

//! Image specification and metadata structures

use super::Architecture;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use tokio::io::AsyncReadExt;

/// Golden image specification for building Ubuntu images
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageSpec {
    /// Ubuntu version (e.g., "24.04", "22.04")
    pub ubuntu_version: String,
    /// Target architecture
    pub architecture: Architecture,
    /// Base packages to install in the image
    pub base_packages: Vec<String>,
    /// Custom scripts to run during image creation
    pub custom_scripts: Vec<PathBuf>,
    /// VM configuration for image building
    pub vm_config: VmConfig,
}

/// Virtual machine configuration for image building
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VmConfig {
    /// Memory allocation in MB
    pub memory_mb: u32,
    /// Disk size in GB
    pub disk_size_gb: u32,
    /// Number of CPU cores
    pub cpu_cores: u32,
}

/// Metadata for a created golden image
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageInfo {
    /// Unique image identifier
    pub id: String,
    /// Ubuntu version
    pub ubuntu_version: String,
    /// Architecture
    pub architecture: Architecture,
    /// Creation timestamp
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// Image size in bytes
    pub size_bytes: u64,
    /// Image checksum (SHA256)
    pub checksum: String,
    /// Path to image file
    pub path: PathBuf,
}

impl Default for VmConfig {
    fn default() -> Self {
        Self {
            memory_mb: 2048,
            disk_size_gb: 20,
            cpu_cores: 2,
        }
    }
}

impl ImageSpec {
    /// Validate image specification
    pub fn validate(&self) -> crate::Result<()> {
        // Validate Ubuntu version format (basic check)
        // Expect a single dot, e.g., "24.04"; more rigorous validation could be added later
        if self.ubuntu_version.matches('.').count() != 1 {
            return Err(crate::error::AutoInstallError::ValidationError(format!(
                "Invalid Ubuntu version format: {}",
                self.ubuntu_version
            )));
        }

        // Validate VM configuration
        if self.vm_config.memory_mb < 1024 {
            return Err(crate::error::AutoInstallError::ValidationError(
                "VM memory must be at least 1024 MB".to_string(),
            ));
        }

        if self.vm_config.disk_size_gb < 10 {
            return Err(crate::error::AutoInstallError::ValidationError(
                "VM disk size must be at least 10 GB".to_string(),
            ));
        }

        if self.vm_config.cpu_cores == 0 {
            return Err(crate::error::AutoInstallError::ValidationError(
                "VM must have at least 1 CPU core".to_string(),
            ));
        }

        // Validate custom scripts exist
        for script in &self.custom_scripts {
            if !script.exists() {
                return Err(crate::error::AutoInstallError::ValidationError(format!(
                    "Custom script not found: {}",
                    script.display()
                )));
            }
        }

        Ok(())
    }

    /// Create a minimal Ubuntu image specification
    pub fn minimal(ubuntu_version: String, architecture: Architecture) -> Self {
        Self {
            ubuntu_version,
            architecture,
            base_packages: vec![
                "openssh-server".to_string(),
                "curl".to_string(),
                "wget".to_string(),
                "htop".to_string(),
                "vim".to_string(),
            ],
            custom_scripts: vec![],
            vm_config: VmConfig::default(),
        }
    }
}

impl ImageInfo {
    /// Create new image info with generated ID
    pub fn new(
        ubuntu_version: String,
        architecture: Architecture,
        size_bytes: u64,
        checksum: String,
        path: PathBuf,
    ) -> Self {
        let id = format!(
            "ubuntu-{}-{}-{}",
            ubuntu_version,
            architecture.as_str(),
            &checksum[..checksum.len().min(8)]
        );
        Self {
            id,
            ubuntu_version,
            architecture,
            size_bytes,
            checksum,
            path,
            created_at: chrono::Utc::now(),
        }
    }

    /// Get human-readable size
    pub fn size_human(&self) -> String {
        let size = self.size_bytes as f64;
        if size >= 1_073_741_824.0 {
            format!("{:.1} GB", size / 1_073_741_824.0)
        } else if size >= 1_048_576.0 {
            format!("{:.1} MB", size / 1_048_576.0)
        } else if size >= 1024.0 {
            format!("{:.1} KB", size / 1024.0)
        } else {
            format!("{} bytes", self.size_bytes)
        }
    }

    /// Check if image file exists
    pub fn exists(&self) -> bool {
        self.path.exists()
    }

    /// Get file extension
    pub fn extension(&self) -> Option<&str> {
        self.path.extension()?.to_str()
    }

    /// Validate image file integrity
    pub async fn validate_integrity(&self) -> Result<bool, std::io::Error> {
        if !self.exists() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("Image file not found: {:?}", self.path),
            ));
        }

        // Calculate checksum of the file
        let mut file = tokio::fs::File::open(&self.path).await?;
        let mut hasher = Sha256::new();
        let mut buffer = vec![0; 8192];

        loop {
            let bytes_read = file.read(&mut buffer).await?;
            if bytes_read == 0 {
                break;
            }
            hasher.update(&buffer[..bytes_read]);
        }

        let calculated_checksum = format!("{:x}", hasher.finalize());
        Ok(calculated_checksum == self.checksum)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_validate_image_spec_valid() {
        let spec = ImageSpec {
            ubuntu_version: "24.04".to_string(),
            architecture: Architecture::Amd64,
            base_packages: vec!["openssh-server".to_string()],
            custom_scripts: vec![],
            vm_config: VmConfig {
                memory_mb: 2048,
                disk_size_gb: 20,
                cpu_cores: 2,
            },
        };
        assert!(spec.validate().is_ok());
    }

    #[test]
    fn test_validate_image_spec_invalid_version() {
        let spec = ImageSpec {
            ubuntu_version: "2404".to_string(), // missing dot
            architecture: Architecture::Amd64,
            base_packages: vec![],
            custom_scripts: vec![],
            vm_config: VmConfig::default(),
        };
        let err = spec.validate().unwrap_err();
        assert!(err.to_string().contains("Invalid Ubuntu version format"));
    }

    #[test]
    fn test_validate_image_spec_invalid_vm_config() {
        let spec = ImageSpec {
            ubuntu_version: "24.04".to_string(),
            architecture: Architecture::Amd64,
            base_packages: vec![],
            custom_scripts: vec![],
            vm_config: VmConfig {
                memory_mb: 512,
                disk_size_gb: 5,
                cpu_cores: 0,
            },
        };
        // Any of the constraints can fail; ensure we get an error
        assert!(spec.validate().is_err());
    }

    #[test]
    fn test_image_info_size_human() {
        let info = ImageInfo::new(
            "24.04".to_string(),
            Architecture::Amd64,
            1024 * 1024 * 3, // 3 MiB
            "deadbeef".to_string(),
            PathBuf::from("/tmp/x.qcow2"),
        );
        let h = info.size_human();
        assert!(h.ends_with("MB") || h.ends_with("MiB"));
    }
}
