// file: crates/uaa-core/src/error.rs
// version: 1.0.1
// guid: 57b83a63-07b6-4534-aa6c-51e8797254e0

use thiserror::Error;

/// Result type alias for the application
pub type Result<T> = std::result::Result<T, AutoInstallError>;

/// Error types for the Ubuntu AutoInstall Agent
#[derive(Debug, Error)]
pub enum AutoInstallError {
    #[error("VM operation failed: {0}")]
    VmError(String),

    #[error("Disk operation failed: {0}")]
    DiskError(String),

    #[error("Network operation failed: {0}")]
    NetworkError(String),

    #[error("LUKS operation failed: {0}")]
    LuksError(String),

    #[error("Configuration error: {0}")]
    ConfigError(String),

    #[error("Image operation failed: {0}")]
    ImageError(String),

    #[error("SSH operation failed: {0}")]
    SshError(String),

    #[error("Installation failed: {0}")]
    InstallationError(String),

    #[error("Validation failed: {0}")]
    ValidationError(String),

    #[error("System error: {0}")]
    SystemError(String),

    #[error("Process failed: {command} (exit code: {exit_code:?}): {stderr}")]
    ProcessError {
        command: String,
        exit_code: Option<i32>,
        stderr: String,
    },

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    SerdeError(#[from] serde_yaml::Error),

    #[error("JSON serialization error: {0}")]
    JsonError(#[from] serde_json::Error),

    #[error("HTTP error: {0}")]
    HttpError(#[from] reqwest::Error),
}
