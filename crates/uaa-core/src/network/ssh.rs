// file: crates/uaa-core/src/network/ssh.rs
// version: 1.6.0
// guid: t0u1v2w3-x4y5-6789-0123-456789tuvwxy
// last-edited: 2026-07-13

//! SSH client for remote deployment operations

use crate::Result;
use regex::Regex;
use ssh2::Session;
use std::net::TcpStream;
use std::sync::OnceLock;
use tracing::{debug, error, info};

/// Redact known secret-bearing argv shapes before a command reaches any log
/// line or error value. Two shapes exist in this codebase's power-control
/// callers today: an `IPMI_PASSWORD='...'` env-var prefix
/// (`power::build_ipmi_command`) and a `-p '...'` argv flag (dashcli/wsman —
/// `power::dash`, `power::amt_wol`). Every builder of those commands rejects
/// a password containing `'`, so `[^']*` always matches exactly the secret
/// value, never spilling past the closing quote. Applied unconditionally
/// here (not left to call-site discipline) because a future caller could
/// easily forget to redact before logging — this is the one seam every
/// command this client runs passes through.
fn redact_command(command: &str) -> String {
    static PATTERNS: OnceLock<[(Regex, &str); 2]> = OnceLock::new();
    let patterns = PATTERNS.get_or_init(|| {
        [
            (
                Regex::new(r"IPMI_PASSWORD='[^']*'").expect("static pattern is valid regex"),
                "IPMI_PASSWORD='***'",
            ),
            (
                Regex::new(r"-p '[^']*'").expect("static pattern is valid regex"),
                "-p '***'",
            ),
        ]
    });
    let mut redacted = command.to_string();
    for (pattern, replacement) in patterns.iter() {
        redacted = pattern.replace_all(&redacted, *replacement).into_owned();
    }
    redacted
}

/// SSH client for remote operations
pub struct SshClient {
    session: Option<Session>,
    host: String,
    /// When true, every command is run through `sudo -n bash -c` so a non-root
    /// login user (e.g. `ubuntu-server` in a live installer env with NOPASSWD
    /// sudo) can execute the root-only install steps. Set automatically in
    /// `connect()` based on the login username.
    sudo: bool,
}

/// Wrap a command so it runs as root via passwordless sudo when `sudo` is set.
///
/// The command is passed as a single-quoted argument to `bash -c`; embedded
/// single quotes are escaped as `'\''` so any content — pipes, heredocs, nested
/// `bash -lc '...'` — is reconstructed verbatim by the outer shell and executed
/// unchanged. `sudo -n` never prompts (fails fast if sudo would need a password).
fn wrap_sudo(sudo: bool, command: &str) -> String {
    if sudo {
        format!("sudo -n bash -c '{}'", command.replace('\'', "'\\''"))
    } else {
        command.to_string()
    }
}

impl SshClient {
    /// Create a new SSH client
    pub fn new() -> Self {
        Self {
            session: None,
            host: String::new(),
            sudo: false,
        }
    }

    /// Connect to remote host via SSH
    pub async fn connect(&mut self, host: &str, username: &str) -> Result<()> {
        info!("Connecting to {} as {}", host, username);

        let tcp = TcpStream::connect(format!("{}:22", host)).map_err(|e| {
            crate::error::AutoInstallError::SshError(format!(
                "Failed to connect to {}: {}",
                host, e
            ))
        })?;

        let mut session = Session::new().map_err(|e| {
            crate::error::AutoInstallError::SshError(format!("Failed to create SSH session: {}", e))
        })?;

        session.set_tcp_stream(tcp);
        session.handshake().map_err(|e| {
            crate::error::AutoInstallError::SshError(format!("SSH handshake failed: {}", e))
        })?;

        // Try SSH agent first, then fall back to key files.
        let mut authed = session.userauth_agent(username).is_ok();
        if !authed {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
            for key in &[
                format!("{}/.ssh/id_ed25519", home),
                format!("{}/.ssh/id_rsa", home),
                format!("{}/.ssh/id_ecdsa", home),
            ] {
                if !std::path::Path::new(key).exists() {
                    continue;
                }
                if session
                    .userauth_pubkey_file(username, None, std::path::Path::new(key), None)
                    .is_ok()
                {
                    info!("Authenticated using key file {}", key);
                    authed = true;
                    break;
                }
            }
        }
        if !authed {
            return Err(crate::error::AutoInstallError::SshError(
                "SSH authentication failed — no agent key or key file worked".to_string(),
            ));
        }

        if !session.authenticated() {
            return Err(crate::error::AutoInstallError::SshError(
                "SSH authentication failed".to_string(),
            ));
        }

        self.session = Some(session);
        self.host = host.to_string();
        // A non-root login user must escalate to run the install steps; the
        // live installer env grants NOPASSWD sudo to that user. Root needs none.
        self.sudo = username != "root";
        if self.sudo {
            info!("Non-root login '{}': commands will run via sudo -n", username);
        }

        info!("SSH connection established to {}", host);
        Ok(())
    }

    /// Execute command on remote host
    pub async fn execute(&mut self, command: &str) -> Result<()> {
        debug!("Executing command: {}", redact_command(command));

        let session = self.session.as_mut().ok_or_else(|| {
            crate::error::AutoInstallError::SshError("No active SSH session".to_string())
        })?;

        let mut channel = session.channel_session().map_err(|e| {
            crate::error::AutoInstallError::SshError(format!("Failed to create SSH channel: {}", e))
        })?;

        channel.exec(&wrap_sudo(self.sudo, command)).map_err(|e| {
            crate::error::AutoInstallError::SshError(format!("Failed to execute command: {}", e))
        })?;

        let mut stdout = String::new();
        let mut stderr = String::new();

        // Read stdout and stderr
        channel.read_to_string(&mut stdout).map_err(|e| {
            crate::error::AutoInstallError::SshError(format!("Failed to read stdout: {}", e))
        })?;
        channel.stderr().read_to_string(&mut stderr).map_err(|e| {
            crate::error::AutoInstallError::SshError(format!("Failed to read stderr: {}", e))
        })?;

        channel.wait_close().map_err(|e| {
            crate::error::AutoInstallError::SshError(format!("Failed to close SSH channel: {}", e))
        })?;

        let exit_status = channel.exit_status().map_err(|e| {
            crate::error::AutoInstallError::SshError(format!("Failed to get exit status: {}", e))
        })?;

        if exit_status != 0 {
            error!("Command failed with exit code {}", exit_status);
            if !stdout.trim().is_empty() {
                error!("STDOUT: {}", stdout);
            }
            if !stderr.trim().is_empty() {
                error!("STDERR: {}", stderr);
            }
            return Err(crate::error::AutoInstallError::ProcessError {
                command: redact_command(command),
                exit_code: Some(exit_status),
                stderr: if stderr.is_empty() { stdout } else { stderr },
            });
        }

        debug!("Command executed successfully");
        Ok(())
    }

    /// Execute command and return output
    pub async fn execute_with_output(&mut self, command: &str) -> Result<String> {
        debug!("Executing command with output: {}", redact_command(command));

        let session = self.session.as_mut().ok_or_else(|| {
            crate::error::AutoInstallError::SshError("No active SSH session".to_string())
        })?;

        let mut channel = session.channel_session().map_err(|e| {
            crate::error::AutoInstallError::SshError(format!("Failed to create SSH channel: {}", e))
        })?;

        channel.exec(&wrap_sudo(self.sudo, command)).map_err(|e| {
            crate::error::AutoInstallError::SshError(format!("Failed to execute command: {}", e))
        })?;

        let mut stdout = String::new();
        let mut stderr = String::new();

        channel.read_to_string(&mut stdout).map_err(|e| {
            crate::error::AutoInstallError::SshError(format!("Failed to read stdout: {}", e))
        })?;
        channel.stderr().read_to_string(&mut stderr).map_err(|e| {
            crate::error::AutoInstallError::SshError(format!("Failed to read stderr: {}", e))
        })?;

        channel.wait_close().map_err(|e| {
            crate::error::AutoInstallError::SshError(format!("Failed to close SSH channel: {}", e))
        })?;

        let exit_status = channel.exit_status().map_err(|e| {
            crate::error::AutoInstallError::SshError(format!("Failed to get exit status: {}", e))
        })?;

        if exit_status != 0 {
            error!("Command failed with exit code {}", exit_status);
            if !stdout.trim().is_empty() {
                error!("STDOUT: {}", stdout);
            }
            if !stderr.trim().is_empty() {
                error!("STDERR: {}", stderr);
            }
            return Err(crate::error::AutoInstallError::ProcessError {
                command: redact_command(command),
                exit_code: Some(exit_status),
                stderr: if stderr.is_empty() { stdout } else { stderr },
            });
        }

        debug!("Command executed successfully: {}", stdout.len());
        Ok(stdout)
    }

    /// Execute command with detailed error reporting but don't fail the session
    pub async fn execute_with_error_collection(
        &mut self,
        command: &str,
        description: &str,
    ) -> Result<(i32, String, String)> {
        info!("Executing: {} -> {}", description, redact_command(command));

        let session = self.session.as_mut().ok_or_else(|| {
            crate::error::AutoInstallError::SshError("No active SSH session".to_string())
        })?;

        let mut channel = session.channel_session().map_err(|e| {
            crate::error::AutoInstallError::SshError(format!("Failed to create SSH channel: {}", e))
        })?;

        channel.exec(&wrap_sudo(self.sudo, command)).map_err(|e| {
            crate::error::AutoInstallError::SshError(format!("Failed to execute command: {}", e))
        })?;

        let mut stdout = String::new();
        let mut stderr = String::new();

        // Read stdout
        channel.read_to_string(&mut stdout).map_err(|e| {
            crate::error::AutoInstallError::SshError(format!("Failed to read stdout: {}", e))
        })?;

        // Read stderr
        channel.stderr().read_to_string(&mut stderr).map_err(|e| {
            crate::error::AutoInstallError::SshError(format!("Failed to read stderr: {}", e))
        })?;

        channel.wait_close().map_err(|e| {
            crate::error::AutoInstallError::SshError(format!("Failed to close SSH channel: {}", e))
        })?;

        let exit_status = channel.exit_status().map_err(|e| {
            crate::error::AutoInstallError::SshError(format!("Failed to get exit status: {}", e))
        })?;

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

    /// Execute a command intended as a boolean check without emitting error logs.
    /// Returns Ok(true) if the command exits with 0, Ok(false) if non-zero, Err on transport issues.
    pub async fn check_silent(&mut self, command: &str) -> Result<bool> {
        let session = self.session.as_mut().ok_or_else(|| {
            crate::error::AutoInstallError::SshError("No active SSH session".to_string())
        })?;

        let mut channel = session.channel_session().map_err(|e| {
            crate::error::AutoInstallError::SshError(format!("Failed to create SSH channel: {}", e))
        })?;

        channel.exec(&wrap_sudo(self.sudo, command)).map_err(|e| {
            crate::error::AutoInstallError::SshError(format!("Failed to execute command: {}", e))
        })?;

        // We don't care about output here; just wait for status
        channel.wait_close().map_err(|e| {
            crate::error::AutoInstallError::SshError(format!("Failed to close SSH channel: {}", e))
        })?;

        let exit_status = channel.exit_status().map_err(|e| {
            crate::error::AutoInstallError::SshError(format!("Failed to get exit status: {}", e))
        })?;

        Ok(exit_status == 0)
    }

    /// Collect system information for debugging
    pub async fn collect_debug_info(&mut self) -> Result<String> {
        info!("Collecting system debug information");

        let mut debug_info = String::new();
        debug_info.push_str("=== SYSTEM DEBUG INFORMATION ===\n\n");

        let debug_commands = vec![
            ("System Info", "uname -a"),
            ("Disk Status", "lsblk -a"),
            (
                "ZFS Pools",
                "zpool status 2>/dev/null || echo 'No ZFS pools'",
            ),
            (
                "ZFS Datasets",
                "zfs list 2>/dev/null || echo 'No ZFS datasets'",
            ),
            (
                "LUKS Status",
                "cryptsetup status luks 2>/dev/null || echo 'No LUKS devices'",
            ),
            ("Mount Points", "mount | grep -E '(zfs|luks|mapper)'"),
            ("Disk Space", "df -h"),
            ("Memory Usage", "free -h"),
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

    /// Upload file to remote host
    pub async fn upload_file(&mut self, local_path: &str, remote_path: &str) -> Result<()> {
        info!("Uploading {} to {}:{}", local_path, self.host, remote_path);

        let session = self.session.as_mut().ok_or_else(|| {
            crate::error::AutoInstallError::SshError("No active SSH session".to_string())
        })?;

        // Get file size
        let metadata =
            std::fs::metadata(local_path).map_err(crate::error::AutoInstallError::IoError)?;

        let file_size = metadata.len();

        // Create SCP channel
        let mut remote_file = session
            .scp_send(std::path::Path::new(remote_path), 0o644, file_size, None)
            .map_err(|e| {
                crate::error::AutoInstallError::SshError(format!(
                    "Failed to create SCP channel: {}",
                    e
                ))
            })?;

        // Read and send file
        let file_content =
            std::fs::read(local_path).map_err(crate::error::AutoInstallError::IoError)?;

        remote_file.write_all(&file_content).map_err(|e| {
            crate::error::AutoInstallError::SshError(format!("Failed to write file data: {}", e))
        })?;

        remote_file.send_eof().map_err(|e| {
            crate::error::AutoInstallError::SshError(format!("Failed to send EOF: {}", e))
        })?;

        remote_file.wait_eof().map_err(|e| {
            crate::error::AutoInstallError::SshError(format!("Failed to wait for EOF: {}", e))
        })?;

        remote_file.close().map_err(|e| {
            crate::error::AutoInstallError::SshError(format!("Failed to close remote file: {}", e))
        })?;

        remote_file.wait_close().map_err(|e| {
            crate::error::AutoInstallError::SshError(format!("Failed to wait for close: {}", e))
        })?;

        info!("File upload completed");
        Ok(())
    }

    /// Download file from remote host
    pub async fn download_file(&mut self, remote_path: &str, local_path: &str) -> Result<()> {
        info!(
            "Downloading {}:{} to {}",
            self.host, remote_path, local_path
        );

        let session = self.session.as_mut().ok_or_else(|| {
            crate::error::AutoInstallError::SshError("No active SSH session".to_string())
        })?;

        let (mut remote_file, stat) = session
            .scp_recv(std::path::Path::new(remote_path))
            .map_err(|e| {
                crate::error::AutoInstallError::SshError(format!(
                    "Failed to create SCP receive channel: {}",
                    e
                ))
            })?;

        let mut contents = Vec::new();
        remote_file.read_to_end(&mut contents).map_err(|e| {
            crate::error::AutoInstallError::SshError(format!("Failed to read remote file: {}", e))
        })?;

        // Verify file size
        if contents.len() != stat.size() as usize {
            return Err(crate::error::AutoInstallError::SshError(
                "File size mismatch during download".to_string(),
            ));
        }

        std::fs::write(local_path, contents).map_err(crate::error::AutoInstallError::IoError)?;

        remote_file.send_eof().map_err(|e| {
            crate::error::AutoInstallError::SshError(format!("Failed to send EOF: {}", e))
        })?;

        remote_file.wait_eof().map_err(|e| {
            crate::error::AutoInstallError::SshError(format!("Failed to wait for EOF: {}", e))
        })?;

        remote_file.close().map_err(|e| {
            crate::error::AutoInstallError::SshError(format!("Failed to close remote file: {}", e))
        })?;

        remote_file.wait_close().map_err(|e| {
            crate::error::AutoInstallError::SshError(format!("Failed to wait for close: {}", e))
        })?;

        info!("File download completed");
        Ok(())
    }

    /// Disconnect SSH session
    pub fn disconnect(&mut self) {
        if let Some(session) = self.session.take() {
            let _ = session.disconnect(None, "", None);
            info!("SSH session disconnected");
        }
    }
}

impl Drop for SshClient {
    fn drop(&mut self) {
        self.disconnect();
    }
}

impl Default for SshClient {
    fn default() -> Self {
        Self::new()
    }
}

use std::io::{Read, Write};

#[cfg(test)]
mod tests {
    use super::{redact_command, wrap_sudo};

    #[test]
    fn test_redact_command_hides_ipmi_password() {
        let cmd = "IPMI_PASSWORD='hunter2' ipmitool -E -I lanplus -H 172.16.3.150 -U ADMIN chassis power on";
        let redacted = redact_command(cmd);
        assert!(!redacted.contains("hunter2"));
        assert_eq!(
            redacted,
            "IPMI_PASSWORD='***' ipmitool -E -I lanplus -H 172.16.3.150 -U ADMIN chassis power on"
        );
    }

    #[test]
    fn test_redact_command_hides_dashcli_and_wsman_argv_password() {
        for cmd in [
            "dashcli -h 10.0.0.5:5985 -u admin -p 'hunter2' power on",
            "wsman invoke -h 10.0.0.5 -P 5985 -u admin -p 'hunter2' -a RequestPowerStateChange http://x -k PowerState=2",
        ] {
            let redacted = redact_command(cmd);
            assert!(!redacted.contains("hunter2"), "leaked in: {redacted}");
            assert!(redacted.contains("-p '***'"), "missing redaction in: {redacted}");
        }
    }

    #[test]
    fn test_redact_command_leaves_non_secret_commands_unchanged() {
        let cmd = "zpool create rpool /dev/mapper/luks";
        assert_eq!(redact_command(cmd), cmd);
    }

    #[test]
    fn test_wrap_sudo_disabled_is_passthrough() {
        assert_eq!(wrap_sudo(false, "zpool create rpool /dev/mapper/luks"), "zpool create rpool /dev/mapper/luks");
    }

    #[test]
    fn test_wrap_sudo_wraps_simple_command() {
        assert_eq!(
            wrap_sudo(true, "wipefs -a /dev/md126"),
            "sudo -n bash -c 'wipefs -a /dev/md126'"
        );
    }

    #[test]
    fn test_wrap_sudo_escapes_embedded_single_quotes() {
        // A chroot command with nested single quotes must survive: each ' becomes
        // '\'' so the outer shell reconstructs the original string verbatim.
        let inner = "chroot /mnt/targetos bash -lc 'update-grub'";
        let got = wrap_sudo(true, inner);
        assert_eq!(
            got,
            "sudo -n bash -c 'chroot /mnt/targetos bash -lc '\\''update-grub'\\'''"
        );
    }

    #[test]
    fn test_wrap_sudo_preserves_heredoc_and_pipe() {
        // Heredoc + pipe content must be reconstructed unchanged by the outer shell.
        let inner = "cat > /mnt/targetos/etc/f <<'EOF'\nline\nEOF";
        let got = wrap_sudo(true, inner);
        // Round-trip check: stripping the wrapper and un-escaping yields the original.
        let prefix = "sudo -n bash -c '";
        assert!(got.starts_with(prefix) && got.ends_with('\''));
        let body = &got[prefix.len()..got.len() - 1];
        assert_eq!(body.replace("'\\''", "'"), inner);
    }
}
