// file: crates/uaa-core/src/security/validation.rs
// version: 1.0.2
// guid: r8s9t0u1-v2w3-4567-8901-234567rstuvw

//! Input validation utilities

use crate::Result;
use std::net::IpAddr;

/// Utility functions for input validation
pub struct ValidationUtils;

impl ValidationUtils {
    /// Validate hostname format
    pub fn validate_hostname(hostname: &str) -> Result<()> {
        if hostname.is_empty() {
            return Err(crate::error::AutoInstallError::ValidationError(
                "Hostname cannot be empty".to_string(),
            ));
        }

        if hostname.len() > 253 {
            return Err(crate::error::AutoInstallError::ValidationError(
                "Hostname cannot exceed 253 characters".to_string(),
            ));
        }

        // Check for valid characters
        if !hostname
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
        {
            return Err(crate::error::AutoInstallError::ValidationError(
                "Hostname contains invalid characters".to_string(),
            ));
        }

        // Cannot start or end with hyphen
        if hostname.starts_with('-') || hostname.ends_with('-') {
            return Err(crate::error::AutoInstallError::ValidationError(
                "Hostname cannot start or end with hyphen".to_string(),
            ));
        }

        Ok(())
    }

    /// Validate IP address format
    pub fn validate_ip_address(ip: &str) -> Result<()> {
        ip.parse::<IpAddr>().map_err(|_| {
            crate::error::AutoInstallError::ValidationError(format!(
                "Invalid IP address format: {}",
                ip
            ))
        })?;
        Ok(())
    }

    /// Validate disk device path
    pub fn validate_disk_device(device: &str) -> Result<()> {
        if !device.starts_with("/dev/") {
            return Err(crate::error::AutoInstallError::ValidationError(format!(
                "Invalid disk device path: {}",
                device
            )));
        }

        // Check for common device patterns
        let valid_patterns = ["/dev/sd", "/dev/nvme", "/dev/hd", "/dev/vd", "/dev/xvd"];

        if !valid_patterns
            .iter()
            .any(|pattern| device.starts_with(pattern))
        {
            return Err(crate::error::AutoInstallError::ValidationError(format!(
                "Unrecognized disk device type: {}",
                device
            )));
        }

        Ok(())
    }

    /// Validate SSH public key format (basic check)
    pub fn validate_ssh_key(key: &str) -> Result<()> {
        if key.is_empty() {
            return Err(crate::error::AutoInstallError::ValidationError(
                "SSH key cannot be empty".to_string(),
            ));
        }

        let parts: Vec<&str> = key.split_whitespace().collect();
        if parts.len() < 2 {
            return Err(crate::error::AutoInstallError::ValidationError(
                "SSH key format is invalid".to_string(),
            ));
        }

        // Check key type
        let valid_types = [
            "ssh-rsa",
            "ssh-dss",
            "ssh-ed25519",
            "ecdsa-sha2-nistp256",
            "ecdsa-sha2-nistp384",
            "ecdsa-sha2-nistp521",
        ];
        if !valid_types.contains(&parts[0]) {
            return Err(crate::error::AutoInstallError::ValidationError(format!(
                "Unsupported SSH key type: {}",
                parts[0]
            )));
        }

        // Basic base64 validation for key data
        if parts[1].len() < 10 {
            return Err(crate::error::AutoInstallError::ValidationError(
                "SSH key data appears too short".to_string(),
            ));
        }

        Ok(())
    }

    /// Validate username format
    pub fn validate_username(username: &str) -> Result<()> {
        if username.is_empty() {
            return Err(crate::error::AutoInstallError::ValidationError(
                "Username cannot be empty".to_string(),
            ));
        }

        if username.len() > 32 {
            return Err(crate::error::AutoInstallError::ValidationError(
                "Username cannot exceed 32 characters".to_string(),
            ));
        }

        // Must start with letter or underscore
        if !username.chars().next().unwrap().is_ascii_lowercase() && !username.starts_with('_') {
            return Err(crate::error::AutoInstallError::ValidationError(
                "Username must start with lowercase letter or underscore".to_string(),
            ));
        }

        // Only lowercase letters, digits, hyphens, and underscores
        if !username
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
        {
            return Err(crate::error::AutoInstallError::ValidationError(
                "Username contains invalid characters".to_string(),
            ));
        }

        // Reserved usernames
        let reserved = [
            "root", "bin", "daemon", "sys", "sync", "games", "man", "lp", "mail", "news", "uucp",
            "proxy", "www-data", "backup", "list", "irc", "gnats", "nobody",
        ];
        if reserved.contains(&username) {
            return Err(crate::error::AutoInstallError::ValidationError(format!(
                "Username '{}' is reserved",
                username
            )));
        }

        Ok(())
    }

    /// Validate timezone
    pub fn validate_timezone(timezone: &str) -> Result<()> {
        // Basic timezone validation - in a real implementation,
        // this would check against a comprehensive timezone database
        if timezone.is_empty() {
            return Err(crate::error::AutoInstallError::ValidationError(
                "Timezone cannot be empty".to_string(),
            ));
        }

        // Check for common timezone patterns
        let valid_patterns = [
            "UTC",
            "GMT",
            "America/",
            "Europe/",
            "Asia/",
            "Africa/",
            "Australia/",
            "Pacific/",
        ];

        if !valid_patterns
            .iter()
            .any(|pattern| timezone.starts_with(pattern))
        {
            return Err(crate::error::AutoInstallError::ValidationError(format!(
                "Invalid timezone format: {}",
                timezone
            )));
        }

        Ok(())
    }

    /// Validate network interface name
    pub fn validate_interface_name(interface: &str) -> Result<()> {
        if interface.is_empty() {
            return Err(crate::error::AutoInstallError::ValidationError(
                "Interface name cannot be empty".to_string(),
            ));
        }

        if interface.len() > 15 {
            return Err(crate::error::AutoInstallError::ValidationError(
                "Interface name cannot exceed 15 characters".to_string(),
            ));
        }

        // Common interface name patterns
        let valid_patterns = ["eth", "wlan", "lo", "br", "bond", "vlan"];
        if !valid_patterns
            .iter()
            .any(|pattern| interface.starts_with(pattern))
        {
            return Err(crate::error::AutoInstallError::ValidationError(format!(
                "Unrecognized interface name pattern: {}",
                interface
            )));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_hostname() {
        assert!(ValidationUtils::validate_hostname("server01").is_ok());
        assert!(ValidationUtils::validate_hostname("web-server.example.com").is_ok());
        assert!(ValidationUtils::validate_hostname("").is_err());
        assert!(ValidationUtils::validate_hostname("-invalid").is_err());
        assert!(ValidationUtils::validate_hostname("invalid-").is_err());
    }

    #[test]
    fn test_validate_ip_address() {
        assert!(ValidationUtils::validate_ip_address("192.168.1.1").is_ok());
        assert!(ValidationUtils::validate_ip_address("10.0.0.1").is_ok());
        assert!(ValidationUtils::validate_ip_address("2001:db8::1").is_ok());
        assert!(ValidationUtils::validate_ip_address("invalid.ip").is_err());
        assert!(ValidationUtils::validate_ip_address("999.999.999.999").is_err());
    }

    #[test]
    fn test_validate_disk_device() {
        assert!(ValidationUtils::validate_disk_device("/dev/sda").is_ok());
        assert!(ValidationUtils::validate_disk_device("/dev/nvme0n1").is_ok());
        assert!(ValidationUtils::validate_disk_device("/dev/vda").is_ok());
        assert!(ValidationUtils::validate_disk_device("/invalid/path").is_err());
        assert!(ValidationUtils::validate_disk_device("sda").is_err());
    }

    #[test]
    fn test_validate_ssh_key() {
        let valid_key = "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABgQC7... user@example.com";
        assert!(ValidationUtils::validate_ssh_key(valid_key).is_ok());
        assert!(ValidationUtils::validate_ssh_key("").is_err());
        assert!(ValidationUtils::validate_ssh_key("invalid-key").is_err());
    }

    #[test]
    fn test_validate_username() {
        assert!(ValidationUtils::validate_username("admin").is_ok());
        assert!(ValidationUtils::validate_username("user_01").is_ok());
        assert!(ValidationUtils::validate_username("test-user").is_ok());
        assert!(ValidationUtils::validate_username("").is_err());
        assert!(ValidationUtils::validate_username("Root").is_err());
        assert!(ValidationUtils::validate_username("1admin").is_err());
        assert!(ValidationUtils::validate_username("root").is_err());
    }

    #[test]
    fn test_validate_timezone() {
        assert!(ValidationUtils::validate_timezone("UTC").is_ok());
        assert!(ValidationUtils::validate_timezone("America/New_York").is_ok());
        assert!(ValidationUtils::validate_timezone("Europe/London").is_ok());
        assert!(ValidationUtils::validate_timezone("").is_err());
        assert!(ValidationUtils::validate_timezone("Invalid/Timezone").is_err());
    }

    #[test]
    fn test_validate_interface_name() {
        assert!(ValidationUtils::validate_interface_name("eth0").is_ok());
        assert!(ValidationUtils::validate_interface_name("wlan0").is_ok());
        assert!(ValidationUtils::validate_interface_name("br0").is_ok());
        assert!(ValidationUtils::validate_interface_name("").is_err());
        assert!(ValidationUtils::validate_interface_name("invalid123456789").is_err());
    }
}
