// file: crates/uaa-core/src/config/loader.rs
// version: 1.0.1
// guid: d4e5f6g7-h8i9-0123-4567-890123defghi

//! Configuration file loading and environment variable substitution

use super::{ImageSpec, TargetConfig};
use crate::Result;
use regex::Regex;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

/// Configuration loader with environment variable substitution
pub struct ConfigLoader {
    env_vars: HashMap<String, String>,
}

impl ConfigLoader {
    /// Create a new config loader
    pub fn new() -> Self {
        Self {
            env_vars: std::env::vars().collect(),
        }
    }

    /// Load target configuration from YAML file
    pub fn load_target_config<P: AsRef<Path>>(&self, path: P) -> Result<TargetConfig> {
        let content = fs::read_to_string(&path).map_err(|e| {
            crate::error::AutoInstallError::ConfigError(format!(
                "Failed to read target config file {}: {}",
                path.as_ref().display(),
                e
            ))
        })?;

        let expanded = self.expand_env_vars(&content)?;
        let config: TargetConfig = serde_yaml::from_str(&expanded)?;

        // Validate configuration
        config.validate()?;

        Ok(config)
    }

    /// Load image specification from YAML file
    pub fn load_image_spec<P: AsRef<Path>>(&self, path: P) -> Result<ImageSpec> {
        let content = fs::read_to_string(&path).map_err(|e| {
            crate::error::AutoInstallError::ConfigError(format!(
                "Failed to read image spec file {}: {}",
                path.as_ref().display(),
                e
            ))
        })?;

        let expanded = self.expand_env_vars(&content)?;
        let spec: ImageSpec = serde_yaml::from_str(&expanded)?;

        // Validate specification
        spec.validate()?;

        Ok(spec)
    }

    /// Expand environment variables in configuration content
    fn expand_env_vars(&self, content: &str) -> Result<String> {
        let re = Regex::new(r"\$\{([^}]+)\}").map_err(|e| {
            crate::error::AutoInstallError::ConfigError(format!("Invalid regex pattern: {}", e))
        })?;

        let mut result = content.to_string();
        let mut missing_vars = Vec::new();

        for cap in re.captures_iter(content) {
            let var_name = &cap[1];
            let placeholder = &cap[0];

            if let Some(value) = self.env_vars.get(var_name) {
                result = result.replace(placeholder, value);
            } else {
                missing_vars.push(var_name.to_string());
            }
        }

        if !missing_vars.is_empty() {
            return Err(crate::error::AutoInstallError::ConfigError(format!(
                "Missing environment variables: {}",
                missing_vars.join(", ")
            )));
        }

        Ok(result)
    }

    /// Set environment variable for substitution
    pub fn set_env_var(&mut self, key: String, value: String) {
        self.env_vars.insert(key, value);
    }

    /// Check if required environment variables are set
    pub fn check_required_env_vars(&self, config_content: &str) -> Result<Vec<String>> {
        let re = Regex::new(r"\$\{([^}]+)\}").map_err(|e| {
            crate::error::AutoInstallError::ConfigError(format!("Invalid regex pattern: {}", e))
        })?;

        let mut required_vars = Vec::new();
        let mut missing_vars = Vec::new();

        for cap in re.captures_iter(config_content) {
            let var_name = cap[1].to_string();
            if !required_vars.contains(&var_name) {
                required_vars.push(var_name.clone());
                if !self.env_vars.contains_key(&var_name) {
                    missing_vars.push(var_name);
                }
            }
        }

        if !missing_vars.is_empty() {
            return Err(crate::error::AutoInstallError::ConfigError(format!(
                "Missing required environment variables: {}",
                missing_vars.join(", ")
            )));
        }

        Ok(required_vars)
    }
}

impl Default for ConfigLoader {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_env_var_expansion() {
        let mut loader = ConfigLoader::new();
        loader.set_env_var("TEST_VAR".to_string(), "test_value".to_string());

        let content = "key: ${TEST_VAR}";
        let result = loader.expand_env_vars(content).unwrap();
        assert_eq!(result, "key: test_value");
    }

    #[test]
    fn test_missing_env_var() {
        let loader = ConfigLoader::new();
        let content = "key: ${MISSING_VAR}";

        let result = loader.expand_env_vars(content);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Missing environment variables"));
    }

    #[test]
    fn test_load_target_config() -> Result<()> {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"
hostname: test-server
architecture: amd64
disk_device: /dev/sda
timezone: UTC
network:
  interface: eth0
  dhcp: true
  dns_servers:
    - 1.1.1.1
users:
  - name: admin
    sudo: true
    ssh_keys:
      - ssh-rsa AAAAB3...
luks_config:
  passphrase: test_passphrase
  cipher: aes-xts-plain64
  key_size: 512
  hash: sha256
packages:
  - openssh-server
"#
        )
        .unwrap();

        let loader = ConfigLoader::new();
        let config = loader.load_target_config(file.path())?;

        assert_eq!(config.hostname, "test-server");
        assert_eq!(config.users.len(), 1);
        assert!(config.users[0].sudo);

        Ok(())
    }
}
