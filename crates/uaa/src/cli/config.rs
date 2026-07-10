// file: crates/uaa/src/cli/config.rs
// version: 1.1.0
// guid: a0de168b-3b68-4f34-8fe2-c4e513d40d70
// last-edited: 2026-07-10

//! `uaa config` — server-local placement of per-host InstallationConfig files.
//!
//! Ports `scripts/deploy-usb-configs.sh`: `uaa config place` copies
//! `<src>/<host>.yaml` to `<dest>/<hexmac>/uaa.yaml` (mode 0644), optionally
//! injecting place-time secrets from `--inject-from`. Injection is server-local
//! only — there is NO HTTP secret-write API, by design.

use uaa_core::config_place::{place_configs, PlaceOptions, DEFAULT_DEST_BASE, DEFAULT_SRC_DIR};

#[derive(Debug, clap::Args)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub command: ConfigCommand,
}

#[derive(Debug, clap::Subcommand)]
pub enum ConfigCommand {
    /// Place per-host configs server-locally at <dest>/<hexmac>/uaa.yaml (0644).
    Place {
        /// Directory of <host>.yaml source files.
        #[arg(long, default_value = DEFAULT_SRC_DIR)]
        src: String,

        /// Cloud-init web root (files land at <dest>/<hexmac>/uaa.yaml).
        #[arg(long, default_value = DEFAULT_DEST_BASE)]
        dest: String,

        /// Optional per-host secrets file for place-time injection (server-local only).
        #[arg(long)]
        inject_from: Option<String>,

        /// Hosts to place (default: all known hosts).
        hosts: Vec<String>,
    },
}

pub async fn config_command(args: ConfigArgs) -> uaa_core::Result<()> {
    match args.command {
        ConfigCommand::Place {
            src,
            dest,
            inject_from,
            hosts,
        } => {
            let opts = PlaceOptions {
                src_dir: src.into(),
                dest_base: dest.into(),
                inject_from: inject_from.map(Into::into),
                hosts,
            };
            let report = place_configs(&opts)?;

            for placed in &report.placed {
                println!("PLACED  {placed}");
            }
            for (host, reason) in &report.refused {
                eprintln!("REFUSED {host}: {reason}");
            }

            // Exit 1 if any requested host was refused (mirrors the shell script).
            if !report.is_success() {
                std::process::exit(1);
            }
            Ok(())
        }
    }
}
