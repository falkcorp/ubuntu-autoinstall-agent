// file: crates/uaa-core/src/network/local.rs
// version: 1.1.1
// guid: local001-2345-6789-abcd-ef0123456789
// last-edited: 2026-07-10

//! Local command execution for on-machine installation

use crate::Result;
use std::process::Command;
use tracing::{debug, error, info};

/// Local command executor that mimics SshClient interface
pub struct LocalClient {
    #[allow(dead_code)]
    host: String,
}

impl LocalClient {
    /// Create a new local client
    pub fn new() -> Self {
        Self {
            host: "localhost".to_string(),
        }
    }

    /// Connect (no-op for local execution)
    pub async fn connect(&mut self, _host: &str, _username: &str) -> Result<()> {
        info!("Local execution mode - no SSH connection needed");
        Ok(())
    }

    /// Execute command locally
    pub async fn execute(&mut self, command: &str) -> Result<()> {
        debug!("Executing local command: {}", command);

        let output = Command::new("bash")
            .arg("-c")
            .arg(command)
            .output()
            .map_err(|e| crate::error::AutoInstallError::ProcessError {
                command: command.to_string(),
                exit_code: None,
                stderr: format!("Failed to execute command: {}", e),
            })?;

        if !output.status.success() {
            let exit_code = output.status.code();
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);

            error!("Command failed with exit code {:?}", exit_code);
            if !stdout.trim().is_empty() {
                error!("STDOUT: {}", stdout);
            }
            if !stderr.trim().is_empty() {
                error!("STDERR: {}", stderr);
            }

            return Err(crate::error::AutoInstallError::ProcessError {
                command: command.to_string(),
                exit_code,
                stderr: if stderr.is_empty() {
                    stdout.to_string()
                } else {
                    stderr.to_string()
                },
            });
        }

        debug!("Command executed successfully");
        Ok(())
    }

    /// Execute command and return output
    pub async fn execute_with_output(&mut self, command: &str) -> Result<String> {
        debug!("Executing local command with output: {}", command);

        let output = Command::new("bash")
            .arg("-c")
            .arg(command)
            .output()
            .map_err(|e| crate::error::AutoInstallError::ProcessError {
                command: command.to_string(),
                exit_code: None,
                stderr: format!("Failed to execute command: {}", e),
            })?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr);

        if !output.status.success() {
            let exit_code = output.status.code();
            error!("Command failed with exit code {:?}", exit_code);
            if !stdout.trim().is_empty() {
                error!("STDOUT: {}", stdout);
            }
            if !stderr.trim().is_empty() {
                error!("STDERR: {}", stderr);
            }

            return Err(crate::error::AutoInstallError::ProcessError {
                command: command.to_string(),
                exit_code,
                stderr: if stderr.is_empty() {
                    stdout
                } else {
                    stderr.to_string()
                },
            });
        }

        debug!("Command executed successfully: {}", stdout.len());
        Ok(stdout)
    }

    /// Execute command with detailed error reporting
    pub async fn execute_with_error_collection(
        &mut self,
        command: &str,
        description: &str,
    ) -> Result<(i32, String, String)> {
        info!("Executing: {} -> {}", description, command);

        let output = Command::new("bash")
            .arg("-c")
            .arg(command)
            .output()
            .map_err(|e| crate::error::AutoInstallError::ProcessError {
                command: command.to_string(),
                exit_code: None,
                stderr: format!("Failed to execute command: {}", e),
            })?;

        let exit_status = output.status.code().unwrap_or(-1);
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        if exit_status != 0 {
            error!(
                "Command '{}' failed with exit code {}",
                description, exit_status
            );
            error!("STDOUT: {}", stdout);
            error!("STDERR: {}", stderr);
        } else {
            info!("Command '{}' completed successfully", description);
            debug!("STDOUT: {}", stdout);
        }

        Ok((exit_status, stdout, stderr))
    }

    /// Execute a command intended as a boolean check without emitting error logs
    pub async fn check_silent(&mut self, command: &str) -> Result<bool> {
        let output = Command::new("bash")
            .arg("-c")
            .arg(command)
            .output()
            .map_err(|e| crate::error::AutoInstallError::ProcessError {
                command: command.to_string(),
                exit_code: None,
                stderr: format!("Failed to execute command: {}", e),
            })?;

        Ok(output.status.success())
    }

    /// Collect system information for debugging
    pub async fn collect_debug_info(&mut self) -> Result<String> {
        let mut debug_info = String::new();

        let debug_commands = [
            ("System Info", "uname -a"),
            ("Memory Info", "free -h"),
            ("Disk Usage", "df -h"),
            ("Block Devices", "lsblk"),
            ("Mount Points", "mount"),
            (
                "ZFS Pools",
                "zpool status 2>/dev/null || echo 'No ZFS pools'",
            ),
            (
                "LUKS Status",
                "cryptsetup status luks 2>/dev/null || echo 'No LUKS device named luks'",
            ),
            ("Recent Logs", "journalctl --no-pager -n 50"),
            ("Dmesg Errors", "dmesg | tail -20"),
            ("Process List", "ps aux | head -20"),
        ];

        for (desc, cmd) in debug_commands {
            debug_info.push_str(&format!("=== {} ===\n", desc));
            match self.execute_with_output(cmd).await {
                Ok(output) => debug_info.push_str(&output),
                Err(_) => debug_info.push_str("Command failed or not available"),
            }
            debug_info.push_str("\n\n");
        }

        Ok(debug_info)
    }

    /// Upload file (no-op for local - file is already local)
    pub async fn upload_file(&mut self, local_path: &str, remote_path: &str) -> Result<()> {
        info!("Local mode: copying {} to {}", local_path, remote_path);

        let copy_cmd = format!("cp '{}' '{}'", local_path, remote_path);
        self.execute(&copy_cmd).await
    }

    /// Download file (no-op for local - just copy)
    pub async fn download_file(&mut self, remote_path: &str, local_path: &str) -> Result<()> {
        info!("Local mode: copying {} to {}", remote_path, local_path);

        let copy_cmd = format!("cp '{}' '{}'", remote_path, local_path);
        self.execute(&copy_cmd).await
    }

    /// Disconnect (no-op for local)
    pub fn disconnect(&mut self) {
        debug!("Local mode: no disconnect needed");
    }
}

impl Default for LocalClient {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::executor::CommandExecutor;
    // All commands are side-effect-free: echo/true/false/cp on tempfiles. NO
    // installer logic, NO root, NO destructive commands.

    #[tokio::test]
    async fn test_connect_is_noop_ok() {
        let mut client = LocalClient::new();
        let result = client.connect("ignored-host", "ignored-user").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_execute_true_succeeds() {
        let mut client = LocalClient::new();
        let result = client.execute("true").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_execute_false_returns_process_error() {
        let mut client = LocalClient::new();
        let result = client.execute("false").await;
        match result {
            Err(crate::error::AutoInstallError::ProcessError { exit_code, .. }) => {
                assert_eq!(exit_code, Some(1));
            }
            other => panic!("expected ProcessError, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_execute_with_output_captures_stdout() {
        let mut client = LocalClient::new();
        let result = client.execute_with_output("echo hello").await.unwrap();
        assert_eq!(result, "hello\n");
    }

    #[tokio::test]
    async fn test_execute_with_output_failure_prefers_stderr() {
        let mut client = LocalClient::new();

        // stderr present: it wins.
        let result = client
            .execute_with_output("echo oops >&2; exit 3")
            .await;
        match result {
            Err(crate::error::AutoInstallError::ProcessError {
                exit_code, stderr, ..
            }) => {
                assert_eq!(exit_code, Some(3));
                assert!(stderr.contains("oops"));
            }
            other => panic!("expected ProcessError, got {:?}", other),
        }

        // stderr empty, stdout has text: fallback puts stdout text into the
        // error's stderr field. This fallback is intentional; do not "fix" it.
        let result = client.execute_with_output("echo out-only; exit 4").await;
        match result {
            Err(crate::error::AutoInstallError::ProcessError {
                exit_code, stderr, ..
            }) => {
                assert_eq!(exit_code, Some(4));
                assert!(stderr.contains("out-only"));
            }
            other => panic!("expected ProcessError, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_execute_with_error_collection_nonzero_is_ok_tuple() {
        let mut client = LocalClient::new();
        // This method returns Ok EVEN on nonzero exit -- asserting Err here
        // would be wrong.
        let result = client
            .execute_with_error_collection("echo so; echo se >&2; exit 5", "desc")
            .await;
        let (exit_code, stdout, stderr) = result.unwrap();
        assert_eq!(exit_code, 5);
        assert!(stdout.contains("so"));
        assert!(stderr.contains("se"));
    }

    #[tokio::test]
    async fn test_check_silent_true_false() {
        let mut client = LocalClient::new();
        assert!(client.check_silent("true").await.unwrap());
        assert!(!client.check_silent("false").await.unwrap());
    }

    #[tokio::test]
    async fn test_upload_download_copy_tempfiles() {
        let dir = tempfile::TempDir::new().unwrap();
        let src = dir.path().join("src.txt");
        let dst = dir.path().join("dst.txt");
        let back = dir.path().join("back.txt");

        std::fs::write(&src, b"hello local client\n").unwrap();

        let mut client = LocalClient::new();
        client
            .upload_file(src.to_str().unwrap(), dst.to_str().unwrap())
            .await
            .unwrap();
        client
            .download_file(dst.to_str().unwrap(), back.to_str().unwrap())
            .await
            .unwrap();

        let original = std::fs::read(&src).unwrap();
        let copied = std::fs::read(&back).unwrap();
        assert_eq!(original, copied);
    }

    #[tokio::test]
    async fn test_trait_object_execute_matches_inherent() {
        let mut boxed: Box<dyn CommandExecutor> = Box::new(LocalClient::new());

        let trait_result = boxed.execute("true").await;
        assert!(trait_result.is_ok());

        let trait_output = boxed.execute_with_output("echo via-trait").await.unwrap();
        assert_eq!(trait_output, "via-trait\n");

        // Verify the inherent methods behave the same way.
        let mut client = LocalClient::new();
        let inherent_result = client.execute("true").await;
        assert!(inherent_result.is_ok());
        let inherent_output = client.execute_with_output("echo via-trait").await.unwrap();
        assert_eq!(inherent_output, "via-trait\n");
    }

    #[test]
    fn test_default_matches_new() {
        // Both constructors must yield a usable client; the `host` field is
        // private, so we assert construction succeeds and defer behavioral
        // proof to the async tests above.
        let _new = LocalClient::new();
        let _default = LocalClient::default();
    }
}
