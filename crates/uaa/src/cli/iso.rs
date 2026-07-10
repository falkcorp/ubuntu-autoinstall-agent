// file: crates/uaa/src/cli/iso.rs
// version: 1.1.0
// guid: 8e37b417-6ec5-4f85-b0be-aacaeddfbed7
// last-edited: 2026-07-10

//! `uaa iso` — port of `scripts/make-ssh-ready-iso.sh` into `uaa iso remaster`
//! (tooling-port/TASK-01, TP-01).

use std::path::PathBuf;

use uaa_core::iso::remaster::{remaster, OnDone, RemasterOptions};
use uaa_core::network::LocalClient;
use uaa_core::{AutoInstallError, Result};

#[derive(Debug, clap::Args)]
pub struct IsoArgs {
    #[command(subcommand)]
    pub command: IsoSubcommand,
}

#[derive(Debug, clap::Subcommand)]
pub enum IsoSubcommand {
    /// Re-master a stock Ubuntu Server ISO into an auto-SSH-ready installer
    /// USB (port of `scripts/make-ssh-ready-iso.sh`).
    Remaster {
        /// Opt-in: bake the uaa.autoinstall token so the seed's runcmd gate
        /// auto-runs uaa-usb-bootstrap.sh on boot (env UAA_AUTOINSTALL=1).
        #[arg(long)]
        autoinstall: bool,

        /// What to do once the install finishes: poweroff | reboot | shell
        /// (env UAA_ON_DONE). Only meaningful with --autoinstall.
        #[arg(long, value_enum)]
        on_done: Option<OnDone>,

        /// NoCloud seed dir override (env UAA_SEED_DIR; defaults to
        /// installer-image/nocloud relative to the current directory).
        #[arg(long)]
        seed_dir: Option<PathBuf>,

        /// Input .iso file or block device (e.g. /dev/sdc).
        input: String,

        /// Output .iso path (defaults to <input minus .iso>-ssh-ready.iso).
        output: Option<String>,
    },
}

/// Parse `UAA_ON_DONE`; empty/unset -> None, otherwise must be one of the
/// three valid actions (invalid value = hard error, mirroring the script's
/// `case "$ON_DONE" in ""|poweroff|reboot|shell) ;; *) exit 1 ;; esac`).
fn parse_on_done_env() -> Result<Option<OnDone>> {
    match std::env::var("UAA_ON_DONE") {
        Ok(raw) if raw.is_empty() => Ok(None),
        Ok(raw) => match raw.as_str() {
            "poweroff" => Ok(Some(OnDone::Poweroff)),
            "reboot" => Ok(Some(OnDone::Reboot)),
            "shell" => Ok(Some(OnDone::Shell)),
            other => Err(AutoInstallError::ConfigError(format!(
                "UAA_ON_DONE must be poweroff|reboot|shell (got: {other})"
            ))),
        },
        Err(_) => Ok(None),
    }
}

fn env_flag_set(name: &str) -> bool {
    std::env::var(name).as_deref() == Ok("1")
}

pub async fn iso_command(args: IsoArgs) -> Result<()> {
    match args.command {
        IsoSubcommand::Remaster {
            autoinstall,
            on_done,
            seed_dir,
            input,
            output,
        } => {
            let autoinstall = autoinstall || env_flag_set("UAA_AUTOINSTALL");
            let on_done = match on_done {
                Some(v) => Some(v),
                None => parse_on_done_env()?,
            };
            let seed_dir = seed_dir
                .or_else(|| std::env::var("UAA_SEED_DIR").ok().map(PathBuf::from))
                .unwrap_or_else(|| PathBuf::from("installer-image/nocloud"));

            let opts = RemasterOptions {
                input,
                output,
                seed_dir,
                autoinstall,
                on_done,
            };

            let mut executor = LocalClient::new();
            let out = remaster(&mut executor, &opts).await?;
            println!("\nDONE: {out}");
            println!(
                "Write it to the USB, e.g.:  sudo dd if='{out}' of=/dev/sdX bs=4M status=progress conv=fsync"
            );
            Ok(())
        }
    }
}
