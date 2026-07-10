// file: crates/uaa/src/cli/enroll.rs
// version: 1.1.0
// guid: ff6b7df8-50d5-47a6-bfe5-3063d426ffe9
// last-edited: 2026-07-10

//! `uaa enroll` — agent-side enrollment CLI (spec Decision 7 / C6).
//!
//! Generates (or reuses) a P-256 keypair + CSR, pins the install CA from
//! `--ca`, and drives [`uaa_core::pki::enroll_poll`] to submit/poll until a
//! certificate is issued (or the process is held at a rejected/revoked
//! terminal state and keeps retrying hourly). A missing/unreadable `--ca` file
//! is fail-closed: this command prints a clear message naming the path and
//! exits non-zero — it NEVER falls back to system roots or plain HTTP.

use std::path::PathBuf;

use uaa_core::pki::{self, AgentIdentity};

#[derive(Debug, clap::Args)]
pub struct EnrollArgs {
    /// Enrollment plane endpoint (uaa-control `:7444`), e.g. https://172.16.2.30:7444
    #[arg(long)]
    pub endpoint: String,

    /// Path to the pinned install CA certificate (baked into the ISO/PXE seed
    /// by PK-04). Missing/unreadable = fail-closed: NEVER falls back to system
    /// roots or plain HTTP.
    #[arg(long, default_value = "/etc/uaa/install-ca.crt")]
    pub ca: PathBuf,

    /// Directory for persisted key/CSR/claim/cert state.
    #[arg(long, default_value = "/var/lib/uaa")]
    pub state_dir: PathBuf,

    /// Hostname claimed in the CSR (defaults to the local system hostname —
    /// this command runs ON the host being enrolled).
    #[arg(long)]
    pub hostname: Option<String>,

    /// MAC address claimed in the CSR (SAN `uaa-mac:<mac>`).
    #[arg(long)]
    pub mac: String,
}

pub async fn enroll_command(args: EnrollArgs) -> uaa_core::Result<()> {
    let hostname = match args.hostname {
        Some(h) => h,
        None => pki::local_hostname()?,
    };
    let identity = AgentIdentity {
        hostname: hostname.clone(),
        mac: args.mac.clone(),
    };

    println!(
        "uaa enroll: starting for hostname={} mac={} endpoint={} state_dir={} ca={}",
        identity.hostname,
        identity.mac,
        args.endpoint,
        args.state_dir.display(),
        args.ca.display(),
    );

    match pki::enroll_poll(&identity, &args.endpoint, &args.ca, &args.state_dir).await {
        Ok(credential) => {
            println!(
                "uaa enroll: ISSUED — spki={} cert persisted under {}",
                credential.spki_fingerprint,
                args.state_dir.display(),
            );
            Ok(())
        }
        Err(e) => {
            eprintln!(
                "uaa enroll: FAILED — could not complete enrollment using pinned CA {} \
                 (fail-closed: refusing to fall back to system roots or plain HTTP): {e}",
                args.ca.display(),
            );
            Err(e)
        }
    }
}
