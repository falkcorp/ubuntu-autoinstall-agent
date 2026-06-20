// file: src/main.rs
// version: 2.0.0
// guid: h8i9j0k1-l2m3-4567-8901-234567hijklm
// last-edited: 2026-06-20

//! Ubuntu AutoInstall Agent - Main entry point

use clap::Parser;
use tokio::signal;
use tracing::{info, warn};
use ubuntu_autoinstall_agent::{
    cli::{args::Cli, commands::*},
    logging::logger,
    Result,
};

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Initialize logging
    logger::init_logger(cli.verbose, cli.quiet)?;

    // Set up signal handling for graceful shutdown
    let shutdown_signal = async {
        signal::ctrl_c()
            .await
            .expect("Failed to install Ctrl+C handler");
        warn!("Received Ctrl+C, initiating graceful shutdown...");
        cleanup_on_exit().await;
    };

    // Execute command with signal handling
    let command_future = async {
        match cli.command {
            ubuntu_autoinstall_agent::cli::args::Commands::CreateImage {
                arch,
                version,
                output,
                spec,
                cache_dir,
            } => create_image_command(arch.into(), &version, output, spec, cache_dir).await,
            ubuntu_autoinstall_agent::cli::args::Commands::Deploy {
                target,
                config,
                image,
                via_ssh,
                dry_run,
            } => deploy_command(&target, &config, &image, via_ssh, dry_run).await,
            ubuntu_autoinstall_agent::cli::args::Commands::Validate { image } => {
                validate_command(&image).await
            }
            ubuntu_autoinstall_agent::cli::args::Commands::CheckPrereqs => {
                check_prerequisites_command().await
            }
            ubuntu_autoinstall_agent::cli::args::Commands::ListImages { filter_arch, json } => {
                list_images_command(filter_arch.map(Into::into), json).await
            }
            ubuntu_autoinstall_agent::cli::args::Commands::Cleanup {
                older_than_days,
                dry_run,
            } => cleanup_command(older_than_days, dry_run).await,
            ubuntu_autoinstall_agent::cli::args::Commands::Install {
                remote,
                username,
                config,
                investigate_only,
                dry_run,
                hold_on_failure,
                pause_after_storage,
                force,
            } => {
                install_command(
                    remote,
                    username,
                    config,
                    investigate_only,
                    dry_run,
                    hold_on_failure,
                    pause_after_storage,
                    force,
                )
                .await
            }
            ubuntu_autoinstall_agent::cli::args::Commands::SshInstall {
                host,
                hostname,
                username,
                config,
                investigate_only,
                dry_run,
                hold_on_failure,
                pause_after_storage,
            } => {
                ssh_install_command(
                    &host,
                    hostname,
                    username,
                    config,
                    investigate_only,
                    dry_run,
                    hold_on_failure,
                    pause_after_storage,
                )
                .await
            }
            ubuntu_autoinstall_agent::cli::args::Commands::LocalInstall {
                hostname,
                investigate_only,
                dry_run,
                hold_on_failure,
                pause_after_storage,
                force,
            } => {
                local_install_command(
                    hostname,
                    investigate_only,
                    dry_run,
                    hold_on_failure,
                    pause_after_storage,
                    force,
                )
                .await
            }
        }
    };

    // Run command with signal handling
    tokio::select! {
        result = command_future => result,
        _ = shutdown_signal => {
            warn!("Application interrupted by user");
            std::process::exit(130); // Standard exit code for Ctrl+C
        }
    }
}

/// Cleanup function called on exit
async fn cleanup_on_exit() {
    info!("Performing cleanup on exit...");

    // Kill any running QEMU processes
    let _ = tokio::process::Command::new("pkill")
        .args(["-f", "qemu-system"])
        .output()
        .await;

    // Cleanup temporary files
    let cleanup_files = [
        "/tmp/qemu-serial.log",
        "/tmp/qemu-uefi.log",
        "/tmp/qemu-monitor.sock",
        "/tmp/OVMF_VARS.fd",
    ];

    for file in &cleanup_files {
        let _ = tokio::fs::remove_file(file).await;
    }

    info!("Cleanup completed");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_cleanup_on_exit() {
        // Arrange
        let temp_dir = TempDir::new().unwrap();
        let test_file = temp_dir.path().join("test_cleanup_file");
        tokio::fs::write(&test_file, "test content").await.unwrap();

        // Verify file exists
        assert!(test_file.exists());

        // Act
        cleanup_on_exit().await;

        // Assert
        // The cleanup function should complete without panicking
        // Note: We can't easily test the pkill command or file cleanup
        // without mocking, but we can ensure the function runs
        // Test passes if function completes without panic
    }

    #[test]
    fn test_main_module_structure() {
        // This test ensures the main module compiles and has the expected structure
        // Arrange & Act & Assert
        // If this compiles, the module structure is correct
        // Test passes if compilation succeeds
    }

    #[test]
    fn test_cleanup_file_paths() {
        // Arrange
        let expected_cleanup_files = [
            "/tmp/qemu-serial.log",
            "/tmp/qemu-uefi.log",
            "/tmp/qemu-monitor.sock",
            "/tmp/OVMF_VARS.fd",
        ];

        // Act & Assert
        // Verify these are valid path strings
        for file_path in expected_cleanup_files {
            assert!(Path::new(file_path).is_absolute());
            assert!(file_path.starts_with("/tmp/"));
        }
    }

    #[tokio::test]
    async fn test_cleanup_on_exit_safe_execution() {
        // Arrange
        // Create a controlled test environment

        // Act
        let cleanup_task = tokio::spawn(cleanup_on_exit());

        // Assert
        // The cleanup function should not panic or hang indefinitely
        let result = tokio::time::timeout(std::time::Duration::from_secs(5), cleanup_task).await;

        assert!(result.is_ok()); // Should complete within timeout
        assert!(result.unwrap().is_ok()); // Should not panic
    }

    #[test]
    fn test_signal_handling_setup() {
        // This test verifies that the module includes the necessary signal handling imports
        // If this compiles, signal handling is properly imported

        // Arrange & Act & Assert
        // Just verify the signal handling module is available at compile time
        // Test passes if compilation succeeds
    }
}
