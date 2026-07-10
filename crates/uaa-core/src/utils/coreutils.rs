// file: crates/uaa-core/src/utils/coreutils.rs
// version: 1.0.2
// guid: a8b9c0d1-e2f3-4567-8901-234567890abc

//! Cross-platform coreutils integration
//!
//! This module provides utilities to use uutils/coreutils implementations
//! when available, falling back to system commands when not available.
//! See: https://github.com/uutils/coreutils

use crate::Result;
use std::path::Path;
use tokio::process::Command;
use tracing::debug;

/// Coreutils command executor with fallback to system commands
pub struct CoreUtils;

impl CoreUtils {
    /// Get the best available implementation of a command
    /// Prefers uutils implementation if available, falls back to system command
    pub async fn get_command(utility: &str) -> String {
        // Check for uutils implementation first
        let uutils_cmd = format!("uu_{}", utility);
        if crate::utils::system::SystemUtils::command_exists(&uutils_cmd).await {
            debug!("Using uutils implementation: {}", uutils_cmd);
            return uutils_cmd;
        }

        // Fall back to system command
        debug!("Using system command: {}", utility);
        utility.to_string()
    }

    /// Execute df command to get disk space information
    pub async fn df<P: AsRef<Path>>(path: P) -> Result<String> {
        let cmd = Self::get_command("df").await;
        let output = Command::new(&cmd)
            .args(["-BG", path.as_ref().to_str().unwrap()])
            .output()
            .await
            .map_err(|e| {
                crate::error::AutoInstallError::SystemError(format!(
                    "Failed to execute {} command: {}",
                    cmd, e
                ))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(crate::error::AutoInstallError::SystemError(format!(
                "{} command failed: {}",
                cmd, stderr
            )));
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    /// Execute ls command to list directory contents
    pub async fn ls<P: AsRef<Path>>(path: P) -> Result<String> {
        let cmd = Self::get_command("ls").await;
        let output = Command::new(&cmd)
            .args(["-la", path.as_ref().to_str().unwrap()])
            .output()
            .await
            .map_err(|e| {
                crate::error::AutoInstallError::SystemError(format!(
                    "Failed to execute {} command: {}",
                    cmd, e
                ))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(crate::error::AutoInstallError::SystemError(format!(
                "{} command failed: {}",
                cmd, stderr
            )));
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    /// Execute cat command to read file contents
    pub async fn cat<P: AsRef<Path>>(path: P) -> Result<String> {
        let cmd = Self::get_command("cat").await;
        let output = Command::new(&cmd)
            .arg(path.as_ref().to_str().unwrap())
            .output()
            .await
            .map_err(|e| {
                crate::error::AutoInstallError::SystemError(format!(
                    "Failed to execute {} command: {}",
                    cmd, e
                ))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(crate::error::AutoInstallError::SystemError(format!(
                "{} command failed: {}",
                cmd, stderr
            )));
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    /// Execute cp command to copy files
    pub async fn cp<P: AsRef<Path>, Q: AsRef<Path>>(src: P, dst: Q) -> Result<()> {
        let cmd = Self::get_command("cp").await;
        let output = Command::new(&cmd)
            .args([
                src.as_ref().to_str().unwrap(),
                dst.as_ref().to_str().unwrap(),
            ])
            .output()
            .await
            .map_err(|e| {
                crate::error::AutoInstallError::SystemError(format!(
                    "Failed to execute {} command: {}",
                    cmd, e
                ))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(crate::error::AutoInstallError::SystemError(format!(
                "{} command failed: {}",
                cmd, stderr
            )));
        }

        Ok(())
    }

    /// Execute mkdir command to create directories
    pub async fn mkdir<P: AsRef<Path>>(path: P, parents: bool) -> Result<()> {
        let cmd = Self::get_command("mkdir").await;
        let mut command = Command::new(&cmd);

        if parents {
            command.arg("-p");
        }

        let output = command
            .arg(path.as_ref().to_str().unwrap())
            .output()
            .await
            .map_err(|e| {
                crate::error::AutoInstallError::SystemError(format!(
                    "Failed to execute {} command: {}",
                    cmd, e
                ))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(crate::error::AutoInstallError::SystemError(format!(
                "{} command failed: {}",
                cmd, stderr
            )));
        }

        Ok(())
    }

    /// Execute rm command to remove files or directories
    pub async fn rm<P: AsRef<Path>>(path: P, recursive: bool, force: bool) -> Result<()> {
        let cmd = Self::get_command("rm").await;
        let mut command = Command::new(&cmd);

        if recursive {
            command.arg("-r");
        }

        if force {
            command.arg("-f");
        }

        let output = command
            .arg(path.as_ref().to_str().unwrap())
            .output()
            .await
            .map_err(|e| {
                crate::error::AutoInstallError::SystemError(format!(
                    "Failed to execute {} command: {}",
                    cmd, e
                ))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(crate::error::AutoInstallError::SystemError(format!(
                "{} command failed: {}",
                cmd, stderr
            )));
        }

        Ok(())
    }

    /// Check if uutils implementations are available
    pub async fn check_uutils_availability() -> Result<Vec<String>> {
        let common_utils = [
            "cat", "cp", "df", "ls", "mkdir", "rm", "mv", "chmod", "chown",
        ];
        let mut available = Vec::new();
        let mut missing = Vec::new();

        for util in &common_utils {
            let uutils_cmd = format!("uu_{}", util);
            if crate::utils::system::SystemUtils::command_exists(&uutils_cmd).await {
                available.push(uutils_cmd);
            } else {
                missing.push(util.to_string());
            }
        }

        debug!("Available uutils commands: {:?}", available);
        debug!("Missing uutils commands (will use system): {:?}", missing);

        Ok(available)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_get_command() {
        // Test that we get some command back (either uutils or system)
        let cmd = CoreUtils::get_command("ls").await;
        assert!(!cmd.is_empty());

        // Should be either "ls" or "uu_ls"
        assert!(cmd == "ls" || cmd == "uu_ls");
    }

    #[tokio::test]
    async fn test_check_uutils_availability() {
        let result = CoreUtils::check_uutils_availability().await;
        assert!(result.is_ok());

        let _available = result.unwrap();
        // Should return a list (might be empty if no uutils installed)
        // This just verifies the function doesn't panic
    }

    #[tokio::test]
    async fn test_ls_command() {
        // Test ls on a directory that should exist
        let temp_dir = TempDir::new().unwrap();
        let result = CoreUtils::ls(temp_dir.path()).await;

        // Should succeed (either with uutils or system ls)
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_mkdir_and_rm() {
        let temp_dir = TempDir::new().unwrap();
        let test_path = temp_dir.path().join("test_mkdir");

        // Test mkdir
        let mkdir_result = CoreUtils::mkdir(&test_path, false).await;
        assert!(mkdir_result.is_ok());
        assert!(test_path.exists());

        // Test rm (need recursive flag for directories)
        let rm_result = CoreUtils::rm(&test_path, true, false).await;
        assert!(rm_result.is_ok());
    }

    #[tokio::test]
    async fn test_cat_nonexistent_file() {
        let result = CoreUtils::cat("/nonexistent/file/path").await;
        // Should fail for nonexistent file
        assert!(result.is_err());
    }
}
