// file: crates/uaa/src/cli/image.rs
// version: 1.1.0
// guid: 8acd1978-c4a6-4525-99ed-5b622f3fe532
// last-edited: 2026-07-10

//! `uaa image build` — port of `scripts/build-installer-image.sh` into a
//! `CommandExecutor`-backed pipeline (tooling-port/TASK-03, TP-03).

use std::path::PathBuf;

use uaa_core::iso::image_build::{image_build, ImageBuildOptions};
use uaa_core::network::LocalClient;
use uaa_core::Result;

#[derive(Debug, clap::Args)]
pub struct ImageArgs {
    #[command(subcommand)]
    pub command: ImageSubcommand,
}

#[derive(Debug, clap::Subcommand)]
pub enum ImageSubcommand {
    /// Overlay a live-server squashfs with the static `uaa` agent + boot
    /// automation and repack it with `mksquashfs -comp zstd` (port of
    /// `scripts/build-installer-image.sh`).
    Build {
        /// Source live-server squashfs to unpack (must exist).
        #[arg(long)]
        src_squashfs: PathBuf,

        /// Static `uaa` agent binary to inject (must exist).
        #[arg(long)]
        agent: PathBuf,

        /// Output squashfs path (overwritten if it already exists).
        #[arg(long)]
        out: PathBuf,

        /// Overlay assets dir (`uaa-autoinstall.sh`/`.service`). Defaults to
        /// `installer-image/` relative to the current directory.
        #[arg(long)]
        overlay_dir: Option<PathBuf>,
    },
}

pub async fn image_command(args: ImageArgs) -> Result<()> {
    match args.command {
        ImageSubcommand::Build {
            src_squashfs,
            agent,
            out,
            overlay_dir,
        } => {
            let overlay_dir = overlay_dir.unwrap_or_else(|| PathBuf::from("installer-image"));

            println!("==> Unpacking {}", src_squashfs.display());
            let opts = ImageBuildOptions {
                src_squashfs,
                agent_bin: agent,
                out,
                overlay_dir,
            };

            let mut executor = LocalClient::new();
            let report = image_build(&mut executor, &opts).await?;

            println!("==> Injecting agent + boot automation");
            println!("==> Enabling uaa-autoinstall.service (multi-user.target.wants)");
            println!("==> Masking stock installer autostart (VERIFY unit name on VM)");
            println!("==> Checking live-rootfs install tools");
            for tool in &report.missing_tools {
                println!("  WARN: '{tool}' not found in live rootfs — bake it into the overlay");
            }
            println!("==> Repacking squashfs -> {}", report.out.display());

            let size = std::fs::metadata(&report.out)
                .map(|m| human_size(m.len()))
                .unwrap_or_else(|_| "?".to_string());
            println!("==> Done: {} ({size})", report.out.display());
            println!(
                "    Point iPXE at this squashfs and add: uaa.autoinstall uaa.config=<host-yaml-url>"
            );

            Ok(())
        }
    }
}

/// Minimal human-readable byte formatter (`du -h`-style), used only for the
/// CLI's final progress line — the core `image_build` report carries no
/// pre-formatted size, only the real output path.
fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "K", "M", "G", "T"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes}{}", UNITS[unit])
    } else {
        format!("{size:.1}{}", UNITS[unit])
    }
}
