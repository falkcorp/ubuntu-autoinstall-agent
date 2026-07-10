// file: crates/uaa-core/src/config/mod.rs
// version: 1.0.2
// guid: a1b2c3d4-e5f6-7a8b-9c0d-1e2f3a4b5c6d

//! Configuration module for Ubuntu AutoInstall Agent
//!
//! Handles loading and validation of target configurations and image specifications.

pub mod image;
pub mod loader;
pub mod target;

pub use image::{ImageInfo, ImageSpec, VmConfig};
pub use target::{LuksConfig, NetworkConfig, TargetConfig, UserConfig};

use serde::{Deserialize, Serialize};

/// Supported system architectures for Ubuntu deployment
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Architecture {
    #[serde(rename = "amd64")]
    Amd64,
    #[serde(rename = "arm64")]
    Arm64,
}

impl Architecture {
    /// Get the architecture as a string
    pub fn as_str(&self) -> &'static str {
        match self {
            Architecture::Amd64 => "amd64",
            Architecture::Arm64 => "arm64",
        }
    }

    /// Get the QEMU architecture string
    pub fn qemu_arch(&self) -> &'static str {
        match self {
            Architecture::Amd64 => "x86_64",
            Architecture::Arm64 => "aarch64",
        }
    }
}

impl std::str::FromStr for Architecture {
    type Err = crate::error::AutoInstallError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "amd64" | "x86_64" => Ok(Architecture::Amd64),
            "arm64" | "aarch64" => Ok(Architecture::Arm64),
            _ => Err(crate::error::AutoInstallError::ValidationError(format!(
                "Unknown architecture: {}",
                s
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Architecture;
    use std::str::FromStr;

    #[test]
    fn test_arch_as_str_and_qemu() {
        assert_eq!(Architecture::Amd64.as_str(), "amd64");
        assert_eq!(Architecture::Amd64.qemu_arch(), "x86_64");
        assert_eq!(Architecture::Arm64.as_str(), "arm64");
        assert_eq!(Architecture::Arm64.qemu_arch(), "aarch64");
    }

    #[test]
    fn test_arch_from_str() {
        assert!(matches!(
            Architecture::from_str("amd64"),
            Ok(Architecture::Amd64)
        ));
        assert!(matches!(
            Architecture::from_str("x86_64"),
            Ok(Architecture::Amd64)
        ));
        assert!(matches!(
            Architecture::from_str("arm64"),
            Ok(Architecture::Arm64)
        ));
        assert!(matches!(
            Architecture::from_str("aarch64"),
            Ok(Architecture::Arm64)
        ));
        assert!(Architecture::from_str("mips").is_err());
    }
}
