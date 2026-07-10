// file: crates/uaa-core/src/power/mod.rs
// version: 1.1.0
// guid: 76330c27-93dd-46ab-8265-8b8e9b6a4dc1
// last-edited: 2026-07-10

//! Remote power control for fleet hosts (`uaa power <hostname> on|off|status`).
//!
//! Only the IPMI mechanism (Supermicro-style BMC via `ipmitool -I lanplus`) is
//! implemented today. The IPMI command is built here but is ALWAYS executed on
//! the server `POWER_SERVER` (172.16.2.30) over SSH via the existing
//! `CommandExecutor` seam — never locally. macOS `ipmitool` crashes silently
//! (empty output, no error) against the Supermicro X10DSC+ BMC, which cost a
//! full debugging day on 2026-07-09; this module exists specifically to make
//! that mistake structurally impossible (no `std::process::Command("ipmitool")`
//! appears anywhere in this crate).
//!
//! AMD DASH (Lenovo M715q), Intel AMT, and Wake-on-LAN are representable as
//! `PowerMechanism` variants but are UNIMPLEMENTED stubs — see
//! `docs/agent-tasks/DEFERRED.md`.

pub mod amt_wol;
pub mod dash;

use crate::error::AutoInstallError;
use crate::network::CommandExecutor;
use crate::Result;

/// Power action requested on the CLI. Exactly three variants — reset/cycle
/// are intentionally UNREPRESENTABLE (unreliable on the X10DSC+).
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum PowerAction {
    On,
    Off,
    Status,
}

impl PowerAction {
    /// The literal `chassis power` verb for this action. These three strings
    /// are the ONLY verbs this module may ever emit — no `reset`, no `cycle`.
    fn verb(self) -> &'static str {
        match self {
            PowerAction::On => "on",
            PowerAction::Off => "off",
            PowerAction::Status => "status",
        }
    }
}

/// How a given machine is power-controlled. Only Ipmi is implemented;
/// the other mechanisms return a NotImplemented-style error naming
/// docs/agent-tasks/DEFERRED.md.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PowerMechanism {
    /// Supermicro-style BMC via ipmitool lanplus — executed ON THE SERVER
    /// (172.16.2.30) over SSH, never locally (macOS ipmitool crashes
    /// silently against Supermicro BMCs).
    Ipmi {
        /// BMC IP as reachable from the server.
        bmc_host: &'static str,
        /// BMC username. The password is NEVER stored here — it arrives
        /// at runtime via --ipmi-password / UAA_IPMI_PASSWORD.
        username: &'static str,
    },
    /// AMD DASH (Lenovo M715q) — UNIMPLEMENTED stub.
    AmdDash,
    /// Intel AMT — UNIMPLEMENTED stub.
    IntelAmt,
    /// Wake-on-LAN — UNIMPLEMENTED stub.
    WakeOnLan,
}

/// The host that runs ipmitool for us. Deliberately NOT the same constant
/// as COCKROACH_SERVER_IP (same value, different role).
///
/// DEFAULT sourced by `crate::fleet::FleetConfig::power_server` — the live
/// value used at runtime (`run_power_action`) is read through
/// `crate::fleet::fleet()`, not this const directly.
pub const POWER_SERVER: &str = "172.16.2.30";

/// Host registry, backed by `crate::fleet::fleet().power_hosts` (default:
/// today's hardcoded v1 table). Returns `None` for unknown hostnames, and for
/// entries whose `mechanism` string doesn't match a known
/// [`PowerMechanism`] (logged via `tracing::warn!`).
pub fn lookup_host(hostname: &str) -> Option<PowerMechanism> {
    let entry = crate::fleet::fleet()
        .power_hosts
        .iter()
        .find(|e| e.hostname == hostname)?;

    match entry.mechanism.as_str() {
        "ipmi" => match (entry.bmc_host.as_deref(), entry.username.as_deref()) {
            (Some(bmc_host), Some(username)) => Some(PowerMechanism::Ipmi { bmc_host, username }),
            _ => {
                tracing::warn!(
                    "power host '{hostname}' has mechanism 'ipmi' but is missing bmc_host/username"
                );
                None
            }
        },
        "amd-dash" => Some(PowerMechanism::AmdDash),
        "intel-amt" => Some(PowerMechanism::IntelAmt),
        "wol" => Some(PowerMechanism::WakeOnLan),
        other => {
            tracing::warn!("power host '{hostname}' has unknown mechanism '{other}'");
            None
        }
    }
}

/// Full command executed ON THE SERVER. NEVER log this string — it embeds
/// the password. Log redacted_ipmi_command() instead.
///
/// The password travels as an `IPMI_PASSWORD=...` env-var prefix consumed by
/// `ipmitool -E` — NOT as a `-P` argv token (a `-P` token is visible in the
/// server's `ps` output; this is a locked design decision).
pub fn build_ipmi_command(
    bmc_host: &str,
    username: &str,
    password: &str,
    action: PowerAction,
) -> Result<String> {
    if password.is_empty() {
        return Err(AutoInstallError::ConfigError(
            "IPMI password must not be empty".to_string(),
        ));
    }
    if password.contains('\'') {
        // Fail-closed: reject rather than escape — no shell-injection surface.
        return Err(AutoInstallError::ConfigError(
            "IPMI password must not contain a single-quote character".to_string(),
        ));
    }
    Ok(format!(
        "IPMI_PASSWORD='{password}' ipmitool -E -I lanplus -H {bmc_host} -U {username} chassis power {}",
        action.verb()
    ))
}

/// Password-free form of the same command, safe for logs and errors.
pub fn redacted_ipmi_command(bmc_host: &str, username: &str, action: PowerAction) -> String {
    format!(
        "ipmitool -E -I lanplus -H {bmc_host} -U {username} chassis power {}",
        action.verb()
    )
}

/// Build the canonical stub error for an unimplemented mechanism.
fn stub_error(mechanism: &str, hostname: &str) -> AutoInstallError {
    AutoInstallError::SystemError(format!(
        "{mechanism} power control for host '{hostname}' is not implemented yet — see docs/agent-tasks/DEFERRED.md"
    ))
}

/// Fail-closed guard shared by `run_power_action` and `power_command`'s local
/// pre-validation pass: resolve the mechanism for `hostname` and confirm it is
/// an `Ipmi` mechanism with a usable password. Returns `(bmc_host, username)`
/// on success. No executor/network call happens before this returns `Ok`.
pub fn validate_ipmi_request(
    hostname: &str,
    ipmi_password: Option<&str>,
) -> Result<(&'static str, &'static str)> {
    match lookup_host(hostname) {
        None => Err(AutoInstallError::ConfigError(format!(
            "unknown host '{hostname}'; known hosts: unimatrixone, len-serv-001, len-serv-002, len-serv-003"
        ))),
        Some(PowerMechanism::AmdDash) => Err(stub_error("AmdDash", hostname)),
        Some(PowerMechanism::IntelAmt) => Err(stub_error("IntelAmt", hostname)),
        Some(PowerMechanism::WakeOnLan) => Err(stub_error("WakeOnLan", hostname)),
        Some(PowerMechanism::Ipmi { bmc_host, username }) => match ipmi_password {
            Some(p) if !p.is_empty() => Ok((bmc_host, username)),
            _ => Err(AutoInstallError::ConfigError(
                "IPMI password required: set UAA_IPMI_PASSWORD or pass --ipmi-password".to_string(),
            )),
        },
    }
}

/// Dispatch a power action to the correct mechanism for `hostname` and, for
/// Ipmi, execute the built command via `executor` (which the caller must have
/// already connected to `POWER_SERVER`). Every failure path is FAIL-CLOSED:
/// `validate_ipmi_request` returns before any `executor` call, so the mock
/// records zero commands on unknown-host, stub-mechanism, and missing-password
/// paths.
pub async fn run_power_action(
    executor: &mut dyn CommandExecutor,
    hostname: &str,
    action: PowerAction,
    ipmi_password: Option<&str>,
) -> Result<String> {
    let (bmc_host, username) = validate_ipmi_request(hostname, ipmi_password)?;
    // Non-empty by construction of validate_ipmi_request's Ipmi arm.
    let password = ipmi_password.expect("validate_ipmi_request guarantees Some(non-empty)");

    let cmd = build_ipmi_command(bmc_host, username, password, action)?;
    // NEVER log `cmd` — it embeds the password. Log the redacted twin only.
    tracing::info!(
        "power: running on {}: {}",
        crate::fleet::fleet().power_server,
        redacted_ipmi_command(bmc_host, username, action)
    );

    executor.execute_with_output(&cmd).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::collections::HashMap;

    /// Recording mock executor: returns pre-loaded output strings keyed by
    /// command, and records every command it was asked to run so tests can
    /// assert on fail-closed (zero-command) paths.
    struct MockExecutor {
        responses: HashMap<String, String>,
        recorded: Vec<String>,
    }

    impl MockExecutor {
        fn new(pairs: &[(&str, &str)]) -> Self {
            Self {
                responses: pairs
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
                recorded: Vec::new(),
            }
        }

        fn get(&self, cmd: &str) -> String {
            self.responses.get(cmd).cloned().unwrap_or_default()
        }
    }

    #[async_trait]
    impl CommandExecutor for MockExecutor {
        async fn connect(&mut self, _host: &str, _username: &str) -> Result<()> {
            Ok(())
        }
        async fn execute(&mut self, command: &str) -> Result<()> {
            self.recorded.push(command.to_string());
            Ok(())
        }
        async fn execute_with_output(&mut self, command: &str) -> Result<String> {
            self.recorded.push(command.to_string());
            Ok(self.get(command))
        }
        async fn execute_with_error_collection(
            &mut self,
            command: &str,
            _description: &str,
        ) -> Result<(i32, String, String)> {
            self.recorded.push(command.to_string());
            Ok((0, self.get(command), String::new()))
        }
        async fn check_silent(&mut self, command: &str) -> Result<bool> {
            self.recorded.push(command.to_string());
            Ok(!self.get(command).is_empty())
        }
        async fn collect_debug_info(&mut self) -> Result<String> {
            Ok(String::new())
        }
        async fn upload_file(&mut self, _local: &str, _remote: &str) -> Result<()> {
            Ok(())
        }
        async fn download_file(&mut self, _remote: &str, _local: &str) -> Result<()> {
            Ok(())
        }
        fn disconnect(&mut self) {}
    }

    #[test]
    fn test_lookup_host_registry() {
        assert_eq!(
            lookup_host("unimatrixone"),
            Some(PowerMechanism::Ipmi {
                bmc_host: "172.16.3.150",
                username: "ADMIN",
            })
        );
        assert_eq!(lookup_host("len-serv-001"), Some(PowerMechanism::AmdDash));
        assert_eq!(lookup_host("len-serv-002"), Some(PowerMechanism::AmdDash));
        assert_eq!(lookup_host("len-serv-003"), Some(PowerMechanism::AmdDash));
        assert_eq!(lookup_host("nonexistent"), None);
    }

    #[test]
    fn test_build_ipmi_command_shape() {
        let cmd = build_ipmi_command("172.16.3.150", "ADMIN", "test-secret", PowerAction::On)
            .expect("valid password should build");
        assert!(cmd
            .contains("ipmitool -E -I lanplus -H 172.16.3.150 -U ADMIN chassis power on"));
        assert!(cmd.starts_with("IPMI_PASSWORD="));
        assert!(!cmd.contains("-P "));
    }

    #[test]
    fn test_redacted_command_omits_password() {
        let redacted = redacted_ipmi_command("172.16.3.150", "ADMIN", PowerAction::Status);
        assert!(!redacted.contains("test-secret"));
        assert!(!redacted.contains("IPMI_PASSWORD"));
        assert!(redacted.contains("-H 172.16.3.150"));
        assert!(redacted.contains("chassis power"));
    }

    #[test]
    fn test_build_ipmi_command_never_reset() {
        for action in [PowerAction::On, PowerAction::Off, PowerAction::Status] {
            let cmd = build_ipmi_command("172.16.3.150", "ADMIN", "test-secret", action)
                .expect("valid password should build");
            assert!(!cmd.contains("reset"));
            assert!(!cmd.contains("cycle"));
        }
    }

    #[test]
    fn test_build_ipmi_command_rejects_quote() {
        let result = build_ipmi_command("172.16.3.150", "ADMIN", "a'b", PowerAction::On);
        assert!(matches!(result, Err(AutoInstallError::ConfigError(_))));
    }

    #[tokio::test]
    async fn test_run_power_action_unknown_host() {
        let mut mock = MockExecutor::new(&[]);
        let result = run_power_action(&mut mock, "nonexistent", PowerAction::Status, Some("test-secret")).await;
        let err = result.expect_err("unknown host must fail");
        assert!(matches!(err, AutoInstallError::ConfigError(_)));
        assert!(err.to_string().contains("unimatrixone"));
        assert!(err.to_string().contains("len-serv-001"));
        assert_eq!(mock.recorded.len(), 0);
    }

    #[tokio::test]
    async fn test_run_power_action_dash_stub() {
        let mut mock = MockExecutor::new(&[]);
        let result =
            run_power_action(&mut mock, "len-serv-001", PowerAction::On, Some("test-secret")).await;
        let err = result.expect_err("AmdDash stub must fail");
        assert!(err.to_string().contains("docs/agent-tasks/DEFERRED.md"));
        assert_eq!(mock.recorded.len(), 0);
    }

    #[tokio::test]
    async fn test_run_power_action_missing_password() {
        let mut mock = MockExecutor::new(&[]);
        let result = run_power_action(&mut mock, "unimatrixone", PowerAction::Off, None).await;
        let err = result.expect_err("missing password must fail");
        assert!(err.to_string().contains("UAA_IPMI_PASSWORD"));
        assert_eq!(mock.recorded.len(), 0);
    }

    #[tokio::test]
    async fn test_run_power_action_status_output() {
        let expected_cmd =
            build_ipmi_command("172.16.3.150", "ADMIN", "test-secret", PowerAction::Status)
                .unwrap();
        let mut mock = MockExecutor::new(&[(expected_cmd.as_str(), "Chassis Power is on")]);

        let result = run_power_action(
            &mut mock,
            "unimatrixone",
            PowerAction::Status,
            Some("test-secret"),
        )
        .await;

        assert_eq!(result.unwrap(), "Chassis Power is on");
        assert_eq!(mock.recorded, vec![expected_cmd]);
    }
}
