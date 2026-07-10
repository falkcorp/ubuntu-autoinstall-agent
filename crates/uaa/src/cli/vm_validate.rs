// file: crates/uaa/src/cli/vm_validate.rs
// version: 1.1.0
// guid: b76cdef1-f7b0-476c-92ea-a538a0315d6f
// last-edited: 2026-07-10

//! `uaa vm-validate` — CLI wiring for the QEMU+swtpm 8-stage validation
//! harness (`uaa_core::vm_validate`). `scripts/vm-validate.sh` stays
//! AUTHORITATIVE until TG-03 proves this port.

use std::path::PathBuf;
use uaa_core::network::LocalClient;
use uaa_core::vm_validate::{vm_validate, VmValidateOptions};

#[derive(Debug, clap::Args)]
pub struct VmValidateArgs {
    /// Path to the SSH-ready ISO (scripts/make-ssh-ready-iso.sh output).
    #[arg(long)]
    pub iso: PathBuf,

    /// Path to the musl `uaa` agent binary to copy into the live session.
    #[arg(long)]
    pub agent: PathBuf,

    /// Install config; a REPLACE_AT_PLACE_TIME placeholder hard-fails stage 0.
    #[arg(long, default_value = "examples/configs/install/vm-test.yaml")]
    pub config: PathBuf,

    /// Scratch directory for the qcow2 disk, swtpm state, and per-stage logs.
    #[arg(long, default_value = "./vm-validate-work")]
    pub workdir: PathBuf,

    /// Target disk size for the qcow2 image, e.g. "40G".
    #[arg(long, default_value = "40G")]
    pub disk_size: String,

    /// Host-forwarded SSH port into the guest.
    #[arg(long, default_value_t = 10022)]
    pub ssh_port: u16,

    /// Seconds to wait for SSH to come up after each boot.
    #[arg(long, default_value_t = 600)]
    pub boot_timeout: u64,

    /// Seconds to wait for `uaa install` to finish inside the guest.
    #[arg(long, default_value_t = 3600)]
    pub install_timeout: u64,
}

/// Run the harness locally (this process IS the Linux host QEMU runs on —
/// never SSH out to a separate machine to do it) and let a `GATE: FAIL`
/// propagate as a nonzero exit via the returned `Err`.
pub async fn vm_validate_command(args: VmValidateArgs) -> uaa_core::Result<()> {
    let opts = VmValidateOptions {
        iso: args.iso,
        agent: args.agent,
        config: args.config,
        workdir: args.workdir,
        disk_size: args.disk_size,
        ssh_port: args.ssh_port,
        boot_timeout: args.boot_timeout,
        install_timeout: args.install_timeout,
    };

    let mut executor = LocalClient::new();
    vm_validate(&mut executor, &opts).await?;
    Ok(())
}
