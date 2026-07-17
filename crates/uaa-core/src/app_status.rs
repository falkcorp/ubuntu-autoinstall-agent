// file: crates/uaa-core/src/app_status.rs
// version: 1.0.1
// guid: ae423e29-5000-40a6-9061-381b5e94baaa
// last-edited: 2026-07-17

//! Application status reporter.
//!
//! Collects the status of installed applications on this host via
//! `systemctl is-active <unit>` and POSTs it to uaa-control's
//! `app-status` endpoint so the operator can see whether a machine's
//! workloads are actually healthy — not merely that it registered.
//!
//! This module is read-only with respect to local state: it never writes,
//! creates, or modifies systemd state or any files.

use crate::error::{AutoInstallError, Result};
use crate::network::CommandExecutor;

// ── Payload types ────────────────────────────────────────────────────────────

/// One application's status report.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AppStatusReport {
    /// Application kind/tag from ApplicationSpec (e.g. "cockroach").
    pub kind: String,
    /// Systemd unit name (e.g. "cockroach.service").
    pub unit: String,
    /// Whether the unit is currently active.
    pub active: bool,
    /// Raw `systemctl is-active` output, trimmed.
    pub detail: String,
}

/// What one host reports to control's `app-status` endpoint.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AppStatusPayload {
    /// Lowercase `aa:bb:cc:dd:ee:ff`; keys the registry row to `machines.mac`.
    pub mac: String,
    pub reports: Vec<AppStatusReport>,
}

/// Outcome of a successful status POST.
#[derive(Debug, Clone, PartialEq)]
pub struct AppStatusOutcome {
    pub reports_sent: usize,
}

// ── Pure helpers (unit-testable, no I/O beyond command execution) ──────────

/// Normalize + validate a MAC address: lowercase, must be exactly 6
/// colon-separated hex pairs. Checked BEFORE any command/HTTP work so a bad MAC
/// never triggers a partial or misdirected status report.
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

/// Collect status for the given units via `systemctl is-active`.
///
/// Each unit in `units` is a (kind, unit) pair. For each, this function
/// executes `systemctl is-active <unit>` and records the result.
///
/// A non-zero exit from `systemctl is-active` is the NORMAL "inactive/failed"
/// answer — NOT a transport error. The report is still produced with
/// `active: false` and the command output in `detail`, because treating it as
/// an error would mean a dead service reports nothing at all, creating the
/// exact blindness this module exists to remove.
///
/// Empty `units` slice => `Ok(vec![])` — no applications is correct data,
/// and callers must be able to send an empty status report rather than skip
/// reporting entirely.
pub async fn collect_status(
    runner: &mut dyn CommandExecutor,
    units: &[(String, String)],
) -> Result<Vec<AppStatusReport>> {
    let mut reports = Vec::new();

    for (kind, unit) in units {
        let cmd = format!("systemctl is-active '{}'", unit.replace("'", "'\\''"));
        let (exit_code, stdout, _stderr) =
            runner.execute_with_error_collection(&cmd, "systemctl is-active").await?;

        let active = exit_code == 0;
        let detail = stdout.trim().to_string();

        reports.push(AppStatusReport {
            kind: kind.clone(),
            unit: unit.clone(),
            active,
            detail,
        });
    }

    Ok(reports)
}

/// Build the status payload from a MAC + already-collected reports. Validates
/// the MAC via [`normalize_mac`]; does no I/O itself.
pub fn build_payload(mac: &str, reports: Vec<AppStatusReport>) -> Result<AppStatusPayload> {
    let mac = normalize_mac(mac)?;
    Ok(AppStatusPayload { mac, reports })
}

// ── HTTP seam ─────────────────────────────────────────────────────────────────

/// POST the payload as JSON to `<control_url>/app-status`. `control_url` is
/// caller-supplied — never hardcoded here; the CLI resolves control's address.
///
/// Mirrors `luks_sync`'s response handling: 2xx with a JSON body whose
/// `ok` field is `true` => `Ok(AppStatusOutcome)`; anything else (non-2xx
/// status, or `ok:false`) => `SystemError` including the status and body
/// message.
///
/// Read-only w.r.t. local state: this function never touches systemd state
/// or any files.
pub async fn post_status(control_url: &str, payload: &AppStatusPayload) -> Result<AppStatusOutcome> {
    let url = format!("{}/app-status", control_url.trim_end_matches('/'));

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
            "app_status POST to {url} failed: status={status}, message={message}"
        )));
    }

    Ok(AppStatusOutcome {
        reports_sent: payload.reports.len(),
    })
}

/// Validate the MAC, collect status for `units`, POST to `control_url`.
/// `units` is a slice of (kind, unit) pairs, e.g. [("cockroach", "cockroach.service")].
pub async fn report_status(
    runner: &mut dyn CommandExecutor,
    control_url: &str,
    mac: &str,
    units: &[(String, String)],
) -> Result<AppStatusOutcome> {
    let mac = normalize_mac(mac)?;
    let reports = collect_status(runner, units).await?;
    let payload = AppStatusPayload { mac, reports };
    post_status(control_url, &payload).await
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Mock command executor for testing.
    struct MockExecutor {
        commands: Vec<String>,
        responses: Vec<(i32, String)>,
        response_idx: usize,
    }

    impl MockExecutor {
        fn new() -> Self {
            Self {
                commands: Vec::new(),
                responses: Vec::new(),
                response_idx: 0,
            }
        }

        fn add_response(&mut self, exit_code: i32, output: impl Into<String>) {
            self.responses.push((exit_code, output.into()));
        }

        fn record_commands_called(&self) -> usize {
            self.commands.len()
        }
    }

    #[async_trait::async_trait]
    impl CommandExecutor for MockExecutor {
        async fn connect(&mut self, _host: &str, _username: &str) -> Result<()> {
            Ok(())
        }

        async fn execute(&mut self, _command: &str) -> Result<()> {
            Ok(())
        }

        async fn execute_with_output(&mut self, _command: &str) -> Result<String> {
            Ok(String::new())
        }

        async fn execute_with_error_collection(
            &mut self,
            command: &str,
            _description: &str,
        ) -> Result<(i32, String, String)> {
            self.commands.push(command.to_string());

            if self.response_idx >= self.responses.len() {
                panic!("MockExecutor ran out of prepared responses");
            }

            let (exit_code, output) = &self.responses[self.response_idx];
            self.response_idx += 1;

            Ok((*exit_code, output.clone(), String::new()))
        }

        async fn check_silent(&mut self, _command: &str) -> Result<bool> {
            Ok(false)
        }

        async fn collect_debug_info(&mut self) -> Result<String> {
            Ok(String::new())
        }

        async fn upload_file(&mut self, _local_path: &str, _remote_path: &str) -> Result<()> {
            Ok(())
        }

        async fn download_file(&mut self, _remote_path: &str, _local_path: &str) -> Result<()> {
            Ok(())
        }

        fn disconnect(&mut self) {}
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

    #[tokio::test]
    async fn test_no_applications_sends_empty_reports() {
        let mut mock = MockExecutor::new();
        let units: &[(String, String)] = &[];

        let reports = collect_status(&mut mock, units).await.expect("collect empty units");
        assert_eq!(reports.len(), 0);

        let payload = build_payload("aa:bb:cc:dd:ee:f0", reports).expect("valid mac + empty reports");
        assert_eq!(payload.reports.len(), 0);
        assert_eq!(mock.record_commands_called(), 0);
    }

    #[tokio::test]
    async fn test_inactive_unit_reports_false_not_error() {
        let mut mock = MockExecutor::new();
        // systemctl is-active returns 3 for inactive, with output "inactive"
        mock.add_response(3, "inactive");

        let units = vec![("cockroach".to_string(), "cockroach.service".to_string())];

        let reports = collect_status(&mut mock, &units).await
            .expect("non-zero systemctl is-active is not an error");
        assert_eq!(reports.len(), 1);
        assert!(!reports[0].active);
        assert_eq!(reports[0].detail, "inactive");
        assert_eq!(reports[0].kind, "cockroach");
        assert_eq!(reports[0].unit, "cockroach.service");
    }

    #[tokio::test]
    async fn test_active_unit_reports_true() {
        let mut mock = MockExecutor::new();
        // systemctl is-active returns 0 for active, with output "active"
        mock.add_response(0, "active");

        let units = vec![("cockroach".to_string(), "cockroach.service".to_string())];

        let reports = collect_status(&mut mock, &units).await
            .expect("systemctl is-active success");
        assert_eq!(reports.len(), 1);
        assert!(reports[0].active);
        assert_eq!(reports[0].detail, "active");
    }

    #[tokio::test]
    async fn test_invalid_mac_rejected_before_any_command() {
        let mut mock = MockExecutor::new();
        let units = vec![("cockroach".to_string(), "cockroach.service".to_string())];

        let err = report_status(&mut mock, "http://control", "not-a-mac", &units)
            .await
            .expect_err("bad MAC must error");

        assert!(matches!(err, AutoInstallError::ConfigError(_)));
        assert_eq!(
            mock.record_commands_called(),
            0,
            "no commands should have been executed for invalid MAC"
        );
    }

    #[test]
    fn test_build_payload_happy() {
        let reports = vec![AppStatusReport {
            kind: "cockroach".to_string(),
            unit: "cockroach.service".to_string(),
            active: true,
            detail: "active".to_string(),
        }];

        let payload = build_payload("aa:bb:cc:dd:ee:f0", reports).expect("valid mac + reports");
        assert_eq!(payload.reports.len(), 1);
        assert_eq!(payload.mac, "aa:bb:cc:dd:ee:f0");
    }

    #[test]
    fn test_payload_serializes_correctly() {
        let reports = vec![AppStatusReport {
            kind: "cockroach".to_string(),
            unit: "cockroach.service".to_string(),
            active: true,
            detail: "active".to_string(),
        }];

        let payload = build_payload("AA:BB:CC:DD:EE:F0", reports).expect("valid mac + reports");

        let json = serde_json::to_string(&payload).expect("serialize payload");
        assert!(json.contains("\"mac\":\"aa:bb:cc:dd:ee:f0\""));
        assert!(json.contains("\"kind\":\"cockroach\""));
        assert!(json.contains("\"active\":true"));
    }
}
