// file: crates/uaa-core/src/network/ssh_installer/status.rs
// version: 1.0.1
// guid: sshstat1-2345-6789-abcd-ef0123456789
// last-edited: 2026-07-10

//! Optional install status reporting to the server webhook.
//!
//! Enabled per-run via the `--report-url` flag. Mirrors the JSON schema the
//! server's `autoinstall-agent.py` `/api/webhook` endpoint consumes (the same
//! fields cloud-init's `reporting.sh` `send_status_update` posts), so SSH-driven
//! installs show up in the same status feed as the cloud-init path.
//!
//! All reporting is best-effort: a failed/slow POST is logged and ignored, never
//! propagated, so status reporting can never fail an install.

use serde_json::json;
use std::time::Duration;
use tracing::{debug, warn};

/// POST a single status update to `url` (e.g. `http://172.16.2.30:25000/api/webhook`).
///
/// `progress` is 0-100. Never returns an error — reporting is advisory.
pub async fn post_status(
    url: &str,
    hostname: &str,
    source_ip: &str,
    status: &str,
    progress: u8,
    message: &str,
) {
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let payload = json!({
        "source_ip": source_ip,
        "timestamp": timestamp,
        "origin": "uaa-ssh-installer",
        "description": message,
        "name": hostname,
        "result": "",
        "event_type": "status_update",
        "status": status,
        "progress": progress,
        "message": message,
        "files": [],
    });

    let client = reqwest::Client::new();
    match client
        .post(url)
        .header("Content-Type", "application/json")
        .timeout(Duration::from_secs(8))
        .json(&payload)
        .send()
        .await
    {
        Ok(resp) => debug!(
            "status report {}% '{}' -> {} (HTTP {})",
            progress,
            status,
            url,
            resp.status()
        ),
        Err(e) => warn!("status report to {} failed (non-fatal): {}", url, e),
    }
}
