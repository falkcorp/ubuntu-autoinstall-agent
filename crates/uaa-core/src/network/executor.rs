// file: crates/uaa-core/src/network/executor.rs
// version: 1.0.1
// guid: exec0001-2345-6789-abcd-ef0123456789

//! Command execution trait for SSH and local execution

use crate::Result;

/// Trait for executing commands either locally or remotely
#[async_trait::async_trait]
pub trait CommandExecutor {
    /// Connect to target (no-op for local)
    async fn connect(&mut self, host: &str, username: &str) -> Result<()>;

    /// Execute command
    async fn execute(&mut self, command: &str) -> Result<()>;

    /// Execute command and return output
    async fn execute_with_output(&mut self, command: &str) -> Result<String>;

    /// Execute command with detailed error reporting
    async fn execute_with_error_collection(
        &mut self,
        command: &str,
        description: &str,
    ) -> Result<(i32, String, String)>;

    /// Execute a command intended as a boolean check
    async fn check_silent(&mut self, command: &str) -> Result<bool>;

    /// Collect system information for debugging
    async fn collect_debug_info(&mut self) -> Result<String>;

    /// Upload file
    async fn upload_file(&mut self, local_path: &str, remote_path: &str) -> Result<()>;

    /// Download file
    async fn download_file(&mut self, remote_path: &str, local_path: &str) -> Result<()>;

    /// Disconnect
    fn disconnect(&mut self);
}

#[async_trait::async_trait]
impl CommandExecutor for crate::network::SshClient {
    async fn connect(&mut self, host: &str, username: &str) -> Result<()> {
        self.connect(host, username).await
    }

    async fn execute(&mut self, command: &str) -> Result<()> {
        self.execute(command).await
    }

    async fn execute_with_output(&mut self, command: &str) -> Result<String> {
        self.execute_with_output(command).await
    }

    async fn execute_with_error_collection(
        &mut self,
        command: &str,
        description: &str,
    ) -> Result<(i32, String, String)> {
        self.execute_with_error_collection(command, description)
            .await
    }

    async fn check_silent(&mut self, command: &str) -> Result<bool> {
        self.check_silent(command).await
    }

    async fn collect_debug_info(&mut self) -> Result<String> {
        self.collect_debug_info().await
    }

    async fn upload_file(&mut self, local_path: &str, remote_path: &str) -> Result<()> {
        self.upload_file(local_path, remote_path).await
    }

    async fn download_file(&mut self, remote_path: &str, local_path: &str) -> Result<()> {
        self.download_file(remote_path, local_path).await
    }

    fn disconnect(&mut self) {
        self.disconnect()
    }
}

#[async_trait::async_trait]
impl CommandExecutor for crate::network::LocalClient {
    async fn connect(&mut self, host: &str, username: &str) -> Result<()> {
        self.connect(host, username).await
    }

    async fn execute(&mut self, command: &str) -> Result<()> {
        self.execute(command).await
    }

    async fn execute_with_output(&mut self, command: &str) -> Result<String> {
        self.execute_with_output(command).await
    }

    async fn execute_with_error_collection(
        &mut self,
        command: &str,
        description: &str,
    ) -> Result<(i32, String, String)> {
        self.execute_with_error_collection(command, description)
            .await
    }

    async fn check_silent(&mut self, command: &str) -> Result<bool> {
        self.check_silent(command).await
    }

    async fn collect_debug_info(&mut self) -> Result<String> {
        self.collect_debug_info().await
    }

    async fn upload_file(&mut self, local_path: &str, remote_path: &str) -> Result<()> {
        self.upload_file(local_path, remote_path).await
    }

    async fn download_file(&mut self, remote_path: &str, local_path: &str) -> Result<()> {
        self.download_file(remote_path, local_path).await
    }

    fn disconnect(&mut self) {
        self.disconnect()
    }
}
