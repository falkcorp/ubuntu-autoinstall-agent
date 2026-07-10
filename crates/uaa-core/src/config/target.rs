// file: crates/uaa-core/src/config/target.rs
// version: 1.0.2
// guid: b2c3d4e5-f6g7-8901-2345-678901bcdefg

//! Target machine configuration structures

use super::Architecture;
use serde::{Deserialize, Serialize};

/// Configuration for target machine deployment
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetConfig {
    /// Target machine hostname
    pub hostname: String,
    /// Target architecture (amd64 or arm64)
    pub architecture: Architecture,
    /// Primary disk device path (e.g., /dev/sda)
    pub disk_device: String,
    /// System timezone
    pub timezone: String,
    /// Network configuration
    pub network: NetworkConfig,
    /// User accounts to create
    pub users: Vec<UserConfig>,
    /// LUKS encryption configuration
    pub luks_config: LuksConfig,
    /// Additional packages to install
    pub packages: Vec<String>,
}

/// Network interface configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkConfig {
    /// Primary network interface name
    pub interface: String,
    /// Static IP address (if not using DHCP)
    pub ip_address: Option<String>,
    /// Gateway IP address (if not using DHCP)
    pub gateway: Option<String>,
    /// DNS server addresses
    pub dns_servers: Vec<String>,
    /// Use DHCP for network configuration
    pub dhcp: bool,
}

/// User account configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserConfig {
    /// Username
    pub name: String,
    /// Grant sudo privileges
    pub sudo: bool,
    /// SSH public keys for authentication
    pub ssh_keys: Vec<String>,
    /// Default shell (defaults to /bin/bash)
    pub shell: Option<String>,
}

/// LUKS encryption configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LuksConfig {
    /// Encryption passphrase (supports environment variable substitution)
    pub passphrase: String,
    /// Encryption cipher (e.g., aes-xts-plain64)
    pub cipher: String,
    /// Key size in bits
    pub key_size: u32,
    /// Hash algorithm (e.g., sha256)
    pub hash: String,
}

impl Default for LuksConfig {
    fn default() -> Self {
        Self {
            passphrase: "${LUKS_PASSPHRASE}".to_string(),
            cipher: "aes-xts-plain64".to_string(),
            key_size: 512,
            hash: "sha256".to_string(),
        }
    }
}

impl TargetConfig {
    /// Validate the target configuration
    pub fn validate(&self) -> crate::Result<()> {
        // Validate hostname
        if self.hostname.is_empty() {
            return Err(crate::error::AutoInstallError::ValidationError(
                "Hostname cannot be empty".to_string(),
            ));
        }

        // Validate disk device
        if !self.disk_device.starts_with("/dev/") {
            return Err(crate::error::AutoInstallError::ValidationError(format!(
                "Invalid disk device: {}",
                self.disk_device
            )));
        }

        // Validate users
        if self.users.is_empty() {
            return Err(crate::error::AutoInstallError::ValidationError(
                "At least one user must be configured".to_string(),
            ));
        }

        // Check if at least one user has sudo privileges
        if !self.users.iter().any(|u| u.sudo) {
            return Err(crate::error::AutoInstallError::ValidationError(
                "At least one user must have sudo privileges".to_string(),
            ));
        }

        // Validate network configuration
        self.network.validate()?;

        Ok(())
    }
}

impl NetworkConfig {
    /// Validate network configuration
    pub fn validate(&self) -> crate::Result<()> {
        if self.interface.is_empty() {
            return Err(crate::error::AutoInstallError::ValidationError(
                "Network interface cannot be empty".to_string(),
            ));
        }

        if !self.dhcp {
            if self.ip_address.is_none() {
                return Err(crate::error::AutoInstallError::ValidationError(
                    "IP address required when DHCP is disabled".to_string(),
                ));
            }
            if self.gateway.is_none() {
                return Err(crate::error::AutoInstallError::ValidationError(
                    "Gateway required when DHCP is disabled".to_string(),
                ));
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Architecture;

    fn valid_target() -> TargetConfig {
        TargetConfig {
            hostname: "host".to_string(),
            architecture: Architecture::Amd64,
            disk_device: "/dev/sda".to_string(),
            timezone: "UTC".to_string(),
            network: NetworkConfig {
                interface: "eth0".to_string(),
                ip_address: None,
                gateway: None,
                dns_servers: vec!["1.1.1.1".to_string()],
                dhcp: true,
            },
            users: vec![UserConfig {
                name: "admin".to_string(),
                sudo: true,
                ssh_keys: vec![],
                shell: None,
            }],
            luks_config: LuksConfig::default(),
            packages: vec![],
        }
    }

    #[test]
    fn test_target_validate_ok() {
        let t = valid_target();
        assert!(t.validate().is_ok());
    }

    #[test]
    fn test_target_validate_empty_hostname() {
        let mut t = valid_target();
        t.hostname = "".to_string();
        assert!(t.validate().is_err());
    }

    #[test]
    fn test_target_validate_invalid_disk() {
        let mut t = valid_target();
        t.disk_device = "sda".to_string();
        assert!(t.validate().is_err());
    }

    #[test]
    fn test_target_validate_users_and_sudo() {
        let mut t = valid_target();
        // No users
        t.users.clear();
        assert!(t.validate().is_err());

        // No sudo user
        t.users = vec![UserConfig {
            name: "u".to_string(),
            sudo: false,
            ssh_keys: vec![],
            shell: None,
        }];
        assert!(t.validate().is_err());
    }

    #[test]
    fn test_network_validate() {
        let mut n = NetworkConfig {
            interface: "eth0".to_string(),
            ip_address: None,
            gateway: None,
            dns_servers: vec![],
            dhcp: true,
        };
        assert!(n.validate().is_ok());

        // Empty interface
        n.interface.clear();
        assert!(n.validate().is_err());

        // Static requires ip and gateway
        let mut n2 = NetworkConfig {
            interface: "eth0".to_string(),
            ip_address: None,
            gateway: None,
            dns_servers: vec![],
            dhcp: false,
        };
        assert!(n2.validate().is_err());
        n2.ip_address = Some("192.168.1.10/24".to_string());
        assert!(n2.validate().is_err());
        n2.gateway = Some("192.168.1.1".to_string());
        assert!(n2.validate().is_ok());
    }
}
