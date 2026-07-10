// file: crates/uaa-core/src/security/luks.rs
// version: 1.0.2
// guid: q7r8s9t0-u1v2-3456-7890-123456qrstuv

//! LUKS encryption operations

use crate::{config::LuksConfig, network::ssh::SshClient, Result};
use tracing::{debug, info};

/// Manager for LUKS encryption operations
pub struct LuksManager;

impl LuksManager {
    /// Create a new LUKS manager
    pub fn new() -> Self {
        Self
    }

    /// Create LUKS encrypted partition on target device
    pub async fn create_luks_partition(
        &self,
        ssh: &mut SshClient,
        device: &str,
        config: &LuksConfig,
    ) -> Result<()> {
        info!("Creating LUKS partition on {}", device);

        // Validate passphrase is not a template
        if config.passphrase.starts_with("${") {
            return Err(crate::error::AutoInstallError::LuksError(
                "LUKS passphrase contains unresolved environment variable".to_string(),
            ));
        }

        // Create LUKS partition
        let luks_format_cmd = format!(
            "echo '{}' | cryptsetup luksFormat --cipher {} --key-size {} --hash {} --use-random {}",
            config.passphrase, config.cipher, config.key_size, config.hash, device
        );

        ssh.execute(&luks_format_cmd).await?;

        // Open LUKS partition
        let luks_open_cmd = format!(
            "echo '{}' | cryptsetup luksOpen {} ubuntu-root",
            config.passphrase, device
        );

        ssh.execute(&luks_open_cmd).await?;

        info!("LUKS partition created and opened successfully");
        Ok(())
    }

    /// Close LUKS partition
    pub async fn close_luks_partition(&self, ssh: &mut SshClient) -> Result<()> {
        debug!("Closing LUKS partition");
        ssh.execute("cryptsetup luksClose ubuntu-root").await?;
        Ok(())
    }

    /// Check if LUKS is properly configured
    pub async fn verify_luks_setup(&self, ssh: &mut SshClient, device: &str) -> Result<bool> {
        debug!("Verifying LUKS setup on {}", device);

        let output = ssh
            .execute_with_output(&format!("cryptsetup isLuks {}", device))
            .await?;
        Ok(output.trim().is_empty()) // cryptsetup isLuks returns empty on success
    }

    /// Generate secure LUKS passphrase
    pub fn generate_passphrase(&self, length: usize) -> String {
        use ring::rand::{SecureRandom, SystemRandom};

        const CHARSET: &[u8] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789!@#$%^&*";
        let rng = SystemRandom::new();
        let mut passphrase = vec![0u8; length];

        for byte in passphrase.iter_mut() {
            let mut random_byte = [0u8; 1];
            rng.fill(&mut random_byte).unwrap();
            *byte = CHARSET[random_byte[0] as usize % CHARSET.len()];
        }

        String::from_utf8(passphrase).unwrap()
    }

    /// Validate LUKS configuration
    pub fn validate_config(&self, config: &LuksConfig) -> Result<()> {
        // Check cipher
        let valid_ciphers = ["aes-xts-plain64", "aes-cbc-essiv:sha256"];
        if !valid_ciphers.contains(&config.cipher.as_str()) {
            return Err(crate::error::AutoInstallError::ValidationError(format!(
                "Invalid LUKS cipher: {}",
                config.cipher
            )));
        }

        // Check key size
        let valid_key_sizes = [128, 256, 512];
        if !valid_key_sizes.contains(&config.key_size) {
            return Err(crate::error::AutoInstallError::ValidationError(format!(
                "Invalid LUKS key size: {}",
                config.key_size
            )));
        }

        // Check hash
        let valid_hashes = ["sha1", "sha256", "sha512"];
        if !valid_hashes.contains(&config.hash.as_str()) {
            return Err(crate::error::AutoInstallError::ValidationError(format!(
                "Invalid LUKS hash: {}",
                config.hash
            )));
        }

        // Check passphrase strength (basic check)
        if config.passphrase.len() < 8 && !config.passphrase.starts_with("${") {
            return Err(crate::error::AutoInstallError::ValidationError(
                "LUKS passphrase must be at least 8 characters".to_string(),
            ));
        }

        Ok(())
    }
}

impl Default for LuksManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_passphrase() {
        let manager = LuksManager::new();
        let passphrase = manager.generate_passphrase(32);

        assert_eq!(passphrase.len(), 32);
        assert!(passphrase.is_ascii());
    }

    #[test]
    fn test_validate_config() {
        let manager = LuksManager::new();

        let valid_config = LuksConfig {
            passphrase: "securepassword123!".to_string(),
            cipher: "aes-xts-plain64".to_string(),
            key_size: 512,
            hash: "sha256".to_string(),
        };

        assert!(manager.validate_config(&valid_config).is_ok());

        let invalid_config = LuksConfig {
            passphrase: "weak".to_string(),
            cipher: "invalid-cipher".to_string(),
            key_size: 123,
            hash: "invalid-hash".to_string(),
        };

        assert!(manager.validate_config(&invalid_config).is_err());
    }

    #[test]
    fn test_env_var_passphrase() {
        let manager = LuksManager::new();

        let env_config = LuksConfig {
            passphrase: "${LUKS_PASSPHRASE}".to_string(),
            cipher: "aes-xts-plain64".to_string(),
            key_size: 512,
            hash: "sha256".to_string(),
        };

        // Should pass validation even with env var template
        assert!(manager.validate_config(&env_config).is_ok());
    }
}
