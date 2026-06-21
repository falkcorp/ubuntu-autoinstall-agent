// file: src/cli/args.rs
// version: 2.1.0
// guid: f6g7h8i9-j0k1-2345-6789-012345fghijk
// last-edited: 2026-06-21

//! Command line argument definitions

use crate::config::Architecture;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "ubuntu-autoinstall-agent")]
#[command(about = "Automated Ubuntu server deployment with golden images")]
#[command(version = env!("CARGO_PKG_VERSION"))]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,

    #[arg(short, long, global = true)]
    pub verbose: bool,

    #[arg(short, long, global = true)]
    pub quiet: bool,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Create a golden Ubuntu image
    CreateImage {
        #[arg(short, long, value_enum, default_value = "amd64")]
        arch: ArchArg,

        #[arg(long, default_value = "24.04")]
        version: String,

        #[arg(short, long)]
        output: Option<String>,

        #[arg(short, long)]
        spec: Option<String>,

        #[arg(short, long, help = "Directory for caching ISOs and temporary files")]
        cache_dir: Option<String>,
    },

    /// Deploy image to target machine
    Deploy {
        #[arg(short, long)]
        target: String,

        #[arg(short, long)]
        config: String,

        #[arg(short = 'i', long)]
        image: String,

        #[arg(long)]
        via_ssh: bool,

        #[arg(long)]
        dry_run: bool,
    },

    /// Validate image integrity
    Validate {
        #[arg(short, long)]
        image: String,
    },

    /// Check system prerequisites
    CheckPrereqs,

    /// List available images
    ListImages {
        #[arg(short, long)]
        filter_arch: Option<ArchArg>,

        #[arg(short, long)]
        json: bool,
    },

    /// Cleanup old images
    Cleanup {
        #[arg(long, default_value = "30")]
        older_than_days: u32,

        #[arg(long)]
        dry_run: bool,
    },

    /// Unified install subcommand: local when no --remote, SSH when --remote is given.
    Install {
        #[arg(long, help = "SSH into this host and install there (omit for local)")]
        remote: Option<String>,

        #[arg(short, long, default_value = "ubuntu", help = "SSH username (remote only)")]
        username: Option<String>,

        #[arg(short, long, help = "Path to YAML installation config file")]
        config: Option<String>,

        #[arg(long, help = "Only investigate system, don't install")]
        investigate_only: bool,

        #[arg(long, help = "Show what would be done without executing")]
        dry_run: bool,

        #[arg(long, help = "Hold on failure for debugging (keep session open)")]
        hold_on_failure: bool,

        #[arg(long, help = "Pause after storage setup for manual verification")]
        pause_after_storage: bool,

        #[arg(long, help = "Force local install even outside a live environment")]
        force: bool,
    },

    /// Install Ubuntu via SSH to target machine
    SshInstall {
        #[arg(short = 'H', long, help = "Target machine IP address or hostname")]
        host: String,

        #[arg(short = 'n', long, help = "Target hostname for the installation")]
        hostname: Option<String>,

        #[arg(short, long, default_value = "ubuntu", help = "SSH username")]
        username: Option<String>,

        #[arg(short, long, help = "Path to YAML installation config file")]
        config: Option<String>,

        #[arg(long, help = "Only investigate system, don't install")]
        investigate_only: bool,

        #[arg(long, help = "Show what would be done without actually doing it")]
        dry_run: bool,

        #[arg(
            long,
            help = "On failure: do not cleanup/unmount; dump logs and open an interactive shell on the target (keep connection alive)"
        )]
        hold_on_failure: bool,

        #[arg(
            long,
            help = "Pause after storage setup (partitioning, formatting, LUKS, ZFS pools/datasets) and print next commands to run manually"
        )]
        pause_after_storage: bool,
    },

    /// Install Ubuntu locally (on current live system)
    LocalInstall {
        #[arg(short = 'n', long, help = "Hostname for the new installation")]
        hostname: Option<String>,

        #[arg(long, help = "Only investigate system, don't install")]
        investigate_only: bool,

        #[arg(long, help = "Show what would be done without executing")]
        dry_run: bool,

        #[arg(long, help = "Hold on failure for debugging")]
        hold_on_failure: bool,

        #[arg(long, help = "Pause after storage setup for manual verification")]
        pause_after_storage: bool,

        #[arg(
            long,
            help = "Force installation even when not in live environment (use with caution)"
        )]
        force: bool,
    },

    /// Render a subiquity autoinstall user-data from the proven template + per-host values
    RenderUserData {
        #[arg(short = 'n', long, help = "Target hostname, e.g. len-serv-003")]
        hostname: String,

        #[arg(long, help = "Host IP address with CIDR, e.g. 172.16.3.96/23")]
        address: String,

        #[arg(
            long,
            help = "Path to a custom template file (defaults to the embedded len-serv template)"
        )]
        template: Option<String>,

        #[arg(long, help = "Write rendered user-data here (defaults to stdout)")]
        output: Option<String>,
    },
}

/// Architecture argument for CLI
#[derive(clap::ValueEnum, Clone, Debug)]
pub enum ArchArg {
    Amd64,
    Arm64,
}

impl From<ArchArg> for Architecture {
    fn from(arch: ArchArg) -> Self {
        match arch {
            ArchArg::Amd64 => Architecture::Amd64,
            ArchArg::Arm64 => Architecture::Arm64,
        }
    }
}

impl From<Architecture> for ArchArg {
    fn from(arch: Architecture) -> Self {
        match arch {
            Architecture::Amd64 => ArchArg::Amd64,
            Architecture::Arm64 => ArchArg::Arm64,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn test_arch_arg_from_architecture() {
        // Arrange
        let amd64_arch = Architecture::Amd64;
        let arm64_arch = Architecture::Arm64;

        // Act
        let amd64_arg: ArchArg = amd64_arch.into();
        let arm64_arg: ArchArg = arm64_arch.into();

        // Assert
        assert!(matches!(amd64_arg, ArchArg::Amd64));
        assert!(matches!(arm64_arg, ArchArg::Arm64));
    }

    #[test]
    fn test_architecture_from_arch_arg() {
        // Arrange
        let amd64_arg = ArchArg::Amd64;
        let arm64_arg = ArchArg::Arm64;

        // Act
        let amd64_arch: Architecture = amd64_arg.into();
        let arm64_arch: Architecture = arm64_arg.into();

        // Assert
        assert_eq!(amd64_arch, Architecture::Amd64);
        assert_eq!(arm64_arch, Architecture::Arm64);
    }

    #[test]
    fn test_cli_parsing_create_image_minimal() {
        // Arrange
        let args = vec!["ubuntu-autoinstall-agent", "create-image"];

        // Act
        let cli = Cli::try_parse_from(args).unwrap();

        // Assert
        match cli.command {
            Commands::CreateImage {
                arch,
                version,
                output,
                spec,
                cache_dir,
            } => {
                assert!(matches!(arch, ArchArg::Amd64));
                assert_eq!(version, "24.04");
                assert!(output.is_none());
                assert!(spec.is_none());
                assert!(cache_dir.is_none());
            }
            _ => panic!("Expected CreateImage command"),
        }
    }

    #[test]
    fn test_cli_parsing_create_image_full() {
        // Arrange
        let args = vec![
            "ubuntu-autoinstall-agent",
            "create-image",
            "--arch",
            "arm64",
            "--version",
            "22.04",
            "--output",
            "/tmp/output.iso",
            "--spec",
            "spec.yaml",
            "--cache-dir",
            "/tmp/cache",
        ];

        // Act
        let cli = Cli::try_parse_from(args).unwrap();

        // Assert
        match cli.command {
            Commands::CreateImage {
                arch,
                version,
                output,
                spec,
                cache_dir,
            } => {
                assert!(matches!(arch, ArchArg::Arm64));
                assert_eq!(version, "22.04");
                assert_eq!(output.as_deref(), Some("/tmp/output.iso"));
                assert_eq!(spec.as_deref(), Some("spec.yaml"));
                assert_eq!(cache_dir.as_deref(), Some("/tmp/cache"));
            }
            _ => panic!("Expected CreateImage command"),
        }
    }

    #[test]
    fn test_cli_parsing_deploy() {
        // Arrange
        let args = vec![
            "ubuntu-autoinstall-agent",
            "deploy",
            "--target",
            "192.168.1.100",
            "--config",
            "config.yaml",
            "--image",
            "image.iso",
            "--via-ssh",
            "--dry-run",
        ];

        // Act
        let cli = Cli::try_parse_from(args).unwrap();

        // Assert
        match cli.command {
            Commands::Deploy {
                target,
                config,
                image,
                via_ssh,
                dry_run,
            } => {
                assert_eq!(target, "192.168.1.100");
                assert_eq!(config, "config.yaml");
                assert_eq!(image, "image.iso");
                assert!(via_ssh);
                assert!(dry_run);
            }
            _ => panic!("Expected Deploy command"),
        }
    }

    #[test]
    fn test_cli_parsing_validate() {
        // Arrange
        let args = vec![
            "ubuntu-autoinstall-agent",
            "validate",
            "--image",
            "test.iso",
        ];

        // Act
        let cli = Cli::try_parse_from(args).unwrap();

        // Assert
        match cli.command {
            Commands::Validate { image } => {
                assert_eq!(image, "test.iso");
            }
            _ => panic!("Expected Validate command"),
        }
    }

    #[test]
    fn test_cli_parsing_check_prereqs() {
        // Arrange
        let args = vec!["ubuntu-autoinstall-agent", "check-prereqs"];

        // Act
        let cli = Cli::try_parse_from(args).unwrap();

        // Assert
        assert!(matches!(cli.command, Commands::CheckPrereqs));
    }

    #[test]
    fn test_cli_parsing_list_images() {
        // Arrange
        let args = vec![
            "ubuntu-autoinstall-agent",
            "list-images",
            "--filter-arch",
            "amd64",
            "--json",
        ];

        // Act
        let cli = Cli::try_parse_from(args).unwrap();

        // Assert
        match cli.command {
            Commands::ListImages { filter_arch, json } => {
                assert!(matches!(filter_arch, Some(ArchArg::Amd64)));
                assert!(json);
            }
            _ => panic!("Expected ListImages command"),
        }
    }

    #[test]
    fn test_cli_parsing_cleanup() {
        // Arrange
        let args = vec![
            "ubuntu-autoinstall-agent",
            "cleanup",
            "--older-than-days",
            "60",
            "--dry-run",
        ];

        // Act
        let cli = Cli::try_parse_from(args).unwrap();

        // Assert
        match cli.command {
            Commands::Cleanup {
                older_than_days,
                dry_run,
            } => {
                assert_eq!(older_than_days, 60);
                assert!(dry_run);
            }
            _ => panic!("Expected Cleanup command"),
        }
    }

    #[test]
    fn test_cli_parsing_ssh_install_minimal() {
        // Arrange
        let args = vec![
            "ubuntu-autoinstall-agent",
            "ssh-install",
            "--host",
            "10.0.0.5",
        ];

        // Act
        let cli = Cli::try_parse_from(args).unwrap();

        // Assert
        match cli.command {
            Commands::SshInstall {
                host,
                hostname,
                username,
                config,
                investigate_only,
                dry_run,
                hold_on_failure,
                pause_after_storage,
            } => {
                assert_eq!(host, "10.0.0.5");
                assert!(hostname.is_none());
                assert_eq!(username.as_deref(), Some("ubuntu"));
                assert!(config.is_none());
                assert!(!investigate_only);
                assert!(!dry_run);
                assert!(!hold_on_failure);
                assert!(!pause_after_storage);
            }
            _ => panic!("Expected SshInstall command"),
        }
    }

    #[test]
    fn test_cli_parsing_ssh_install_full() {
        // Arrange
        let args = vec![
            "ubuntu-autoinstall-agent",
            "ssh-install",
            "--host",
            "server.example.com",
            "--hostname",
            "prod-web-01",
            "--username",
            "admin",
            "--investigate-only",
            "--dry-run",
            "--hold-on-failure",
            "--pause-after-storage",
        ];

        // Act
        let cli = Cli::try_parse_from(args).unwrap();

        // Assert
        match cli.command {
            Commands::SshInstall {
                host,
                hostname,
                username,
                config,
                investigate_only,
                dry_run,
                hold_on_failure,
                pause_after_storage,
            } => {
                assert_eq!(host, "server.example.com");
                assert_eq!(hostname.as_deref(), Some("prod-web-01"));
                assert_eq!(username.as_deref(), Some("admin"));
                assert!(config.is_none());
                assert!(investigate_only);
                assert!(dry_run);
                assert!(hold_on_failure);
                assert!(pause_after_storage);
            }
            _ => panic!("Expected SshInstall command"),
        }
    }

    #[test]
    fn test_cli_global_flags() {
        // Arrange
        let args = vec![
            "ubuntu-autoinstall-agent",
            "--verbose",
            "--quiet",
            "check-prereqs",
        ];

        // Act
        let cli = Cli::try_parse_from(args).unwrap();

        // Assert
        assert!(cli.verbose);
        assert!(cli.quiet);
        assert!(matches!(cli.command, Commands::CheckPrereqs));
    }

    #[test]
    fn test_cli_parsing_local_install_minimal() {
        // Arrange
        let args = vec!["ubuntu-autoinstall-agent", "local-install"];

        // Act
        let cli = Cli::try_parse_from(args).unwrap();

        // Assert
        match cli.command {
            Commands::LocalInstall {
                hostname,
                investigate_only,
                dry_run,
                hold_on_failure,
                pause_after_storage,
                force,
            } => {
                assert!(hostname.is_none());
                assert!(!investigate_only);
                assert!(!dry_run);
                assert!(!hold_on_failure);
                assert!(!pause_after_storage);
                assert!(!force);
                assert!(!hold_on_failure);
                assert!(!pause_after_storage);
            }
            _ => panic!("Expected LocalInstall command"),
        }
    }

    #[test]
    fn test_cli_parsing_local_install_full() {
        // Arrange
        let args = vec![
            "ubuntu-autoinstall-agent",
            "local-install",
            "--hostname",
            "local-server",
            "--investigate-only",
            "--dry-run",
            "--hold-on-failure",
            "--pause-after-storage",
        ];

        // Act
        let cli = Cli::try_parse_from(args).unwrap();

        // Assert
        match cli.command {
            Commands::LocalInstall {
                hostname,
                investigate_only,
                dry_run,
                hold_on_failure,
                pause_after_storage,
                force,
            } => {
                assert_eq!(hostname.as_deref(), Some("local-server"));
                assert!(investigate_only);
                assert!(dry_run);
                assert!(hold_on_failure);
                assert!(pause_after_storage);
                assert!(!force);
            }
            _ => panic!("Expected LocalInstall command"),
        }
    }

    #[test]
    fn test_cli_parsing_render_user_data() {
        // Arrange
        let args = vec![
            "ubuntu-autoinstall-agent",
            "render-user-data",
            "--hostname",
            "len-serv-003",
            "--address",
            "172.16.3.96/23",
            "--template",
            "/tmp/custom.tmpl",
            "--output",
            "/tmp/out.user-data",
        ];

        // Act
        let cli = Cli::try_parse_from(args).unwrap();

        // Assert
        match cli.command {
            Commands::RenderUserData {
                hostname,
                address,
                template,
                output,
            } => {
                assert_eq!(hostname, "len-serv-003");
                assert_eq!(address, "172.16.3.96/23");
                assert_eq!(template.as_deref(), Some("/tmp/custom.tmpl"));
                assert_eq!(output.as_deref(), Some("/tmp/out.user-data"));
            }
            _ => panic!("Expected RenderUserData command"),
        }
    }
}
