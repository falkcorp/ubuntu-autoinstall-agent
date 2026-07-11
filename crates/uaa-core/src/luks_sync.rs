// file: crates/uaa-core/src/luks_sync.rs
// version: 1.1.0
// guid: 4421bc7a-fab4-49a1-bf33-8fcc78113fff
// last-edited: 2026-07-10

//! LUKS key registry sync (luks-keys/TASK-03, ws7-luks).
//!
//! Reads the local FIDO2 credential state file written by `luks_keys.rs`'s
//! `enroll_fido2` (LK-01) / `revoke_fido2` (LK-02) and POSTs it to
//! uaa-control's `luks_credentials` endpoint (control/TASK-02) so the
//! registry mirrors what is actually enrolled on this host's LUKS header
//! (spec `docs/specs/constellation-design.md` §data model `CREATE TABLE
//! luks_credentials` + §C8 "Registry sync to `luks_credentials`").
//!
//! **YubiKeys are for LUKS disk unlock, NOT auth** (spec Decision 14). This
//! module reports keyslot bookkeeping to the registry; it grants nothing and
//! authenticates nothing.
//!
//! This module is read-only with respect to local state: it never writes,
//! creates, or moves the state file — only `luks_keys.rs` owns that file.

use crate::error::{AutoInstallError, Result};
use crate::luks_keys::LuksCredentialRecord;

// ── Payload types ────────────────────────────────────────────────────────────

/// What one host reports to control's `luks_credentials` endpoint.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LuksSyncPayload {
    /// Lowercase `aa:bb:cc:dd:ee:ff`; keys the registry row to `machines.mac`.
    pub mac: String,
    pub records: Vec<LuksCredentialRecord>,
}

/// Outcome of a successful sync POST.
#[derive(Debug, Clone, PartialEq)]
pub struct LuksSyncOutcome {
    pub records_sent: usize,
    pub message: String,
}

// ── Pure helpers (unit-testable, no I/O beyond a single file read) ──────────

/// Normalize + validate a MAC address: lowercase, must be exactly 6
/// colon-separated hex pairs. Checked BEFORE any file/HTTP work so a bad MAC
/// never triggers a partial or misdirected sync.
pub fn normalize_mac(mac: &str) -> Result<String> {
    let lower = mac.to_ascii_lowercase();
    let parts: Vec<&str> = lower.split(':').collect();

    let valid = parts.len() == 6
        && parts
            .iter()
            .all(|p| p.len() == 2 && p.chars().all(|c| c.is_ascii_hexdigit()));

    if !valid {
        return Err(AutoInstallError::ConfigError(format!(
            "invalid MAC address '{mac}': expected 6 colon-separated hex pairs"
        )));
    }

    Ok(lower)
}

/// Read the LK-01/LK-02 local state file at `state_path`.
///
/// Missing file => `Ok(vec![])` — NOT an error; a freshly-installed host with
/// nothing enrolled yet is correct data, and callers must be able to send an
/// empty sync (`records_sent: 0`) rather than skip syncing entirely.
///
/// Present-but-malformed JSON => hard `SystemError` naming the path — never
/// "send what parsed"; a half-report would let control silently drop
/// revocations that were present in the untouched part of the file.
pub fn read_local_state(state_path: &std::path::Path) -> Result<Vec<LuksCredentialRecord>> {
    if !state_path.exists() {
        return Ok(Vec::new());
    }

    let contents = std::fs::read_to_string(state_path)?;
    if contents.trim().is_empty() {
        return Ok(Vec::new());
    }

    serde_json::from_str(&contents).map_err(|e| {
        AutoInstallError::SystemError(format!(
            "malformed LUKS credential state file at {}: {e}",
            state_path.display()
        ))
    })
}

/// Build the sync payload from a MAC + already-read records. Validates the
/// MAC via [`normalize_mac`]; does no I/O itself.
pub fn build_payload(mac: &str, records: Vec<LuksCredentialRecord>) -> Result<LuksSyncPayload> {
    let mac = normalize_mac(mac)?;
    Ok(LuksSyncPayload { mac, records })
}

// ── HTTP seam ─────────────────────────────────────────────────────────────────

/// POST the payload as JSON to `<control_url>/luks-credentials` (CT-02's
/// endpoint). `control_url` is caller-supplied — never hardcoded here; the
/// CLI resolves control's address.
///
/// Mirrors `call_flip_api`'s response handling: 2xx with a JSON body whose
/// `ok` field is `true` => `Ok(LuksSyncOutcome)`; anything else (non-2xx
/// status, or `ok:false`) => `SystemError` including the status and body
/// message.
///
/// Read-only w.r.t. local state: this function never touches the state file.
pub async fn post_sync(control_url: &str, payload: &LuksSyncPayload) -> Result<LuksSyncOutcome> {
    let url = format!("{}/luks-credentials", control_url.trim_end_matches('/'));

    let response = reqwest::Client::new().post(&url).json(payload).send().await?;
    let status = response.status();
    let body: serde_json::Value = response.json().await?;

    let ok = body.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
    let message = body
        .get("message")
        .or_else(|| body.get("error"))
        .and_then(|v| v.as_str())
        .unwrap_or("no message")
        .to_string();

    if !status.is_success() || !ok {
        return Err(AutoInstallError::SystemError(format!(
            "luks_credentials sync to {url} failed: status={status}, message={message}"
        )));
    }

    Ok(LuksSyncOutcome {
        records_sent: payload.records.len(),
        message,
    })
}

/// Convenience: read the local state, build the payload, and POST it. All
/// validation errors (bad MAC, malformed state file) fire before any HTTP
/// call.
pub async fn sync_credentials(
    control_url: &str,
    mac: &str,
    state_path: &std::path::Path,
) -> Result<LuksSyncOutcome> {
    let mac = normalize_mac(mac)?;
    let records = read_local_state(state_path)?;
    let payload = LuksSyncPayload { mac, records };
    post_sync(control_url, &payload).await
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::luks_keys::CredentialRole;

    fn sample_record(role: CredentialRole, revoked: bool) -> LuksCredentialRecord {
        LuksCredentialRecord {
            yubikey_serial: "12345678".to_string(),
            role,
            luks_keyslot: Some(3),
            enrolled_at: "2026-07-10T00:00:00+00:00".to_string(),
            revoked_at: if revoked {
                Some("2026-07-10T01:00:00+00:00".to_string())
            } else {
                None
            },
        }
    }

    #[test]
    fn test_normalize_mac() {
        assert_eq!(
            normalize_mac("AA:BB:CC:DD:EE:F0").unwrap(),
            "aa:bb:cc:dd:ee:f0"
        );

        assert!(matches!(
            normalize_mac("aabbcc"),
            Err(AutoInstallError::ConfigError(_))
        ));
        assert!(matches!(
            normalize_mac(""),
            Err(AutoInstallError::ConfigError(_))
        ));
        assert!(matches!(
            normalize_mac("aa:bb:cc:dd:ee"),
            Err(AutoInstallError::ConfigError(_))
        ));
    }

    #[test]
    fn test_read_missing_state_is_empty() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state_path = dir.path().join("does-not-exist.json");

        let records = read_local_state(&state_path).expect("missing file is not an error");
        assert!(records.is_empty());
    }

    #[test]
    fn test_read_malformed_state_errors() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state_path = dir.path().join("luks-credentials.json");
        std::fs::write(&state_path, "{not json").expect("write malformed state");

        let err = read_local_state(&state_path).expect_err("malformed JSON must be a hard error");
        match err {
            AutoInstallError::SystemError(msg) => {
                assert!(msg.contains(&state_path.display().to_string()));
            }
            other => panic!("expected SystemError naming the path, got {other:?}"),
        }
    }

    #[test]
    fn test_read_roundtrip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state_path = dir.path().join("luks-credentials.json");

        let records = vec![
            sample_record(CredentialRole::Primary, false),
            sample_record(CredentialRole::Backup1, true),
        ];
        std::fs::write(&state_path, serde_json::to_string_pretty(&records).unwrap())
            .expect("write state");

        let read_back = read_local_state(&state_path).expect("read roundtrip");
        assert_eq!(read_back, records);
    }

    #[test]
    fn test_build_payload_bad_mac_no_io() {
        // Path does NOT exist — if build_payload tried to read it first, the
        // error would be an IO/Json error instead of the MAC ConfigError.
        let bogus_path = std::path::Path::new("/definitely/does/not/exist/state.json");
        assert!(!bogus_path.exists());

        let err = build_payload("not-a-mac", Vec::new()).expect_err("bad mac must error");
        assert!(matches!(err, AutoInstallError::ConfigError(_)));

        // build_payload never touches state_path at all (it takes records,
        // not a path) — confirm no file was created as a side effect either.
        assert!(!bogus_path.exists());
    }

    #[test]
    fn test_payload_serializes_roles_lowercase() {
        let payload = build_payload(
            "AA:BB:CC:DD:EE:F0",
            vec![sample_record(CredentialRole::Backup1, false)],
        )
        .expect("valid mac + records");

        let json = serde_json::to_string(&payload).expect("serialize payload");
        assert!(json.contains("\"role\":\"backup1\""));
        assert!(json.contains("\"mac\":\"aa:bb:cc:dd:ee:f0\""));
    }

    #[test]
    fn test_build_payload_happy() {
        let records = vec![
            sample_record(CredentialRole::Primary, false),
            sample_record(CredentialRole::Backup1, false),
            sample_record(CredentialRole::Backup2, false),
        ];

        let payload =
            build_payload("aa:bb:cc:dd:ee:f0", records).expect("valid mac + records survive");
        assert_eq!(payload.records.len(), 3);
    }
}
