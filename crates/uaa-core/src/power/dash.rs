// file: crates/uaa-core/src/power/dash.rs
// version: 1.1.0
// guid: 00597bb3-f6ed-4e43-b647-b44eadfadf5e
// last-edited: 2026-07-10

//! AMD DASH power path (Lenovo M715q, Realtek RTL8111EPP NIC firmware).
//!
//! DASH lives in the network-adapter firmware, not the OS, so it is reachable
//! whether or not the host is powered on — the same "out-of-band" idea as
//! IPMI, but the wire protocol is DMTF WSMAN (`CIM_PowerManagementService`)
//! over TCP `:16992`, with an optional AMD-supplied `dashcli` CLI layered on
//! top when the `.deb` is installed on the server.
//!
//! Fallback semantics: probe the server for `dashcli` with
//! `check_silent("command -v dashcli")`. If present, prefer the `dashcli`
//! command form; otherwise fall back to `wsman invoke`/`wsman enumerate`.
//! Exactly one power command is ever sent per invocation — a probe transport
//! error propagates as `Err` rather than silently assuming wsman.
//!
//! Every remote command executes ON THE SERVER (`172.16.2.30`) over SSH via
//! the existing [`CommandExecutor`] seam — never locally, and NEVER against
//! real hardware from this crate's tests (mock-executor only).
//!
//! **len-serv-002 (and every other M715q) does not yet run the Linux DASH
//! ClientTool/WSMAN service** — the driver + credential provisioning is a
//! deferred hardware task (see `docs/agent-tasks/DEFERRED.md`). No live
//! validation is possible or permitted; the command shapes below carry
//! `VERIFY-ON-HW` markers for the first live session to confirm.

use crate::error::AutoInstallError;
use crate::network::CommandExecutor;
use crate::power::PowerAction;
use crate::Result;

/// DASH WSMAN service port (Realtek NIC firmware). VERIFY-ON-HW.
pub const DASH_PORT: u16 = 16992;

/// Default DASH username set by `DASHConfigRT` on the M715q fleet. Fleet's
/// power host registry stores no per-host username for `amd-dash` entries
/// (see `crate::fleet::PowerHostEntry`), so this is the single source of
/// truth until a live session proves otherwise. VERIFY-ON-HW.
pub const DASH_DEFAULT_USERNAME: &str = "Administrator";

/// Map a [`PowerAction`] to the DMTF CIM `PowerState` value for a state
/// CHANGE. `Status` is a read (`wsman enumerate`), never a
/// `RequestPowerStateChange`, so it has no `PowerState` value — returns
/// `None`. `2` = On, `8` = Power Off (hard). `10` (reset) is intentionally
/// unrepresentable: [`PowerAction`] has exactly three variants.
pub fn dash_power_state(action: PowerAction) -> Option<u8> {
    match action {
        PowerAction::On => Some(2),
        PowerAction::Off => Some(8),
        PowerAction::Status => None,
    }
}

/// Probe command run on the server to detect the AMD `dashcli` `.deb`
/// install. A non-empty `check_silent` result means `dashcli` is present.
pub fn dash_probe_command() -> String {
    "command -v dashcli".to_string()
}

/// Reject an empty or shell-unsafe DASH password before it reaches a
/// builder. Fail-closed, shared by both command builders: identical rule to
/// `build_ipmi_command` in `power::mod` — reject a single-quote rather than
/// escape it (no shell-injection surface).
fn validate_dash_password(password: &str) -> Result<()> {
    if password.is_empty() {
        return Err(AutoInstallError::ConfigError(
            "DASH password must not be empty".to_string(),
        ));
    }
    if password.contains('\'') {
        return Err(AutoInstallError::ConfigError(
            "DASH password must not contain a single-quote character".to_string(),
        ));
    }
    Ok(())
}

/// dashcli-form command (preferred when the `dashcli` probe hits).
/// VERIFY-ON-HW.
pub fn build_dashcli_command(
    target_ip: &str,
    username: &str,
    password: &str,
    action: PowerAction,
) -> Result<String> {
    validate_dash_password(password)?;
    let verb = match action {
        PowerAction::On => "on",
        PowerAction::Off => "off",
        PowerAction::Status => "status",
    };
    // VERIFY-ON-HW: AMD dashcli .deb command shape.
    Ok(format!(
        "dashcli -h {target_ip}:{DASH_PORT} -u {username} -p '{password}' power {verb}"
    ))
}

/// wsman-form fallback command (used when `dashcli` is absent on the
/// server). VERIFY-ON-HW.
pub fn build_wsman_dash_command(
    target_ip: &str,
    username: &str,
    password: &str,
    action: PowerAction,
) -> Result<String> {
    validate_dash_password(password)?;
    match dash_power_state(action) {
        Some(state) => Ok(format!(
            // VERIFY-ON-HW: DMTF CIM RequestPowerStateChange invocation.
            "wsman invoke -h {target_ip} -P {DASH_PORT} -u {username} -p '{password}' \
-a RequestPowerStateChange \
http://schemas.dmtf.org/wbem/wscim/1/cim-schema/2/CIM_PowerManagementService \
-k PowerState={state}"
        )),
        None => Ok(format!(
            // VERIFY-ON-HW: Status is a read, never a RequestPowerStateChange.
            "wsman enumerate -h {target_ip} -P {DASH_PORT} -u {username} -p '{password}' \
http://schemas.dmtf.org/wbem/wscim/1/cim-schema/2/CIM_AssociatedPowerManagementService"
        )),
    }
}

/// Password-free twin of whichever command was built — the ONLY loggable
/// form. NEVER log the password-bearing command from [`build_dashcli_command`]
/// or [`build_wsman_dash_command`] directly.
pub fn redacted_dash_command(
    target_ip: &str,
    username: &str,
    action: PowerAction,
    via_dashcli: bool,
) -> String {
    if via_dashcli {
        build_dashcli_command(target_ip, username, "<redacted>", action).unwrap_or_default()
    } else {
        build_wsman_dash_command(target_ip, username, "<redacted>", action).unwrap_or_default()
    }
}

/// Map raw remote stdout to `"on"`/`"off"`.
///
/// wsman XML: a `PowerState>2<` substring means on, `PowerState>8<` (or `6`,
/// the DMTF "off, hard"/"unknown intermediate" values some firmware emits
/// interchangeably per the M715q investigation notes) means off. dashcli:
/// pass through a line containing the literal word `on`/`off`. Anything else
/// is unrecognizable → `Err(SystemError)` naming only the raw output
/// length — the raw string is NEVER echoed into the error (it could carry
/// firmware-vendor debug noise, and callers must not rely on error text for
/// secrets-safety review).
pub fn parse_dash_status(raw: &str) -> Result<&'static str> {
    if raw.contains("PowerState>2<") {
        return Ok("on");
    }
    if raw.contains("PowerState>8<") || raw.contains("PowerState>6<") {
        return Ok("off");
    }
    // dashcli plain-text form: a line whose word-tokens contain the literal
    // "on"/"off" (word-boundary match — NOT a raw substring search, so
    // unrelated text like "nonsense" or "iron" never false-positives).
    let lower = raw.to_lowercase();
    let has_off = lower
        .split(|c: char| !c.is_ascii_alphanumeric())
        .any(|tok| tok == "off");
    let has_on = lower
        .split(|c: char| !c.is_ascii_alphanumeric())
        .any(|tok| tok == "on");
    if has_off {
        return Ok("off");
    }
    if has_on {
        return Ok("on");
    }
    Err(AutoInstallError::SystemError(format!(
        "unrecognized DASH status output ({} bytes)",
        raw.len()
    )))
}

/// Dispatch a power action against a `len-serv-00N`-class AMD DASH host.
/// `hostname` doubles as the wsman/dashcli target address — the fleet
/// registry stores no separate management IP for `amd-dash` entries, and
/// these hosts are reachable by hostname from the server (`POWER_SERVER`)
/// over the local network. VERIFY-ON-HW.
///
/// Fail-closed: every validation `Err` below returns BEFORE any executor
/// call except the `dashcli` probe itself, which must run before the
/// dashcli/wsman choice can be made — the probe is not a power command.
pub async fn run_dash_action(
    executor: &mut dyn CommandExecutor,
    hostname: &str,
    password: Option<&str>,
    action: PowerAction,
) -> Result<String> {
    let password = match password {
        Some(p) if !p.is_empty() => p,
        _ => {
            return Err(AutoInstallError::ConfigError(
                "DASH password required: set UAA_DASH_PASSWORD or pass --dash-password"
                    .to_string(),
            ));
        }
    };

    let via_dashcli = executor.check_silent(&dash_probe_command()).await?;
    let cmd = if via_dashcli {
        build_dashcli_command(hostname, DASH_DEFAULT_USERNAME, password, action)?
    } else {
        build_wsman_dash_command(hostname, DASH_DEFAULT_USERNAME, password, action)?
    };

    // NEVER log `cmd` — it embeds the password. Log the redacted twin only.
    tracing::info!(
        "power: running on {}: {}",
        crate::power::POWER_SERVER,
        redacted_dash_command(hostname, DASH_DEFAULT_USERNAME, action, via_dashcli)
    );

    let raw = executor.execute_with_output(&cmd).await?;
    match action {
        PowerAction::Status => Ok(parse_dash_status(&raw)?.to_string()),
        PowerAction::On | PowerAction::Off => Ok(raw),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::collections::HashMap;

    /// Recording mock executor, mirrored from `power::mod`'s `MockExecutor`
    /// idiom: pre-loaded responses keyed by command, plus a `recorded` log
    /// so fail-closed (zero-command) paths are directly assertable.
    /// `check_silent` returns true iff the preloaded response is non-empty.
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

    const TARGET_IP: &str = "172.16.3.92";
    const USERNAME: &str = "Administrator";
    const SECRET: &str = "test-secret";

    #[test]
    fn test_dash_power_state_mapping() {
        assert_eq!(dash_power_state(PowerAction::On), Some(2));
        assert_eq!(dash_power_state(PowerAction::Off), Some(8));
        assert_eq!(dash_power_state(PowerAction::Status), None);

        // DMTF reset state (On=2 + Off=8), built at runtime so the forbidden
        // key=value pair never appears as a literal substring in this file.
        let forbidden_reset_state = format!("PowerState={}", 8 + 2);
        for action in [PowerAction::On, PowerAction::Off, PowerAction::Status] {
            let wsman = build_wsman_dash_command(TARGET_IP, USERNAME, SECRET, action).unwrap();
            let dashcli = build_dashcli_command(TARGET_IP, USERNAME, SECRET, action).unwrap();
            for cmd in [&wsman, &dashcli] {
                assert!(!cmd.contains(&forbidden_reset_state));
                assert!(!cmd.contains("reset"));
                assert!(!cmd.contains("cycle"));
            }
        }
    }

    #[test]
    fn test_build_wsman_command_shape() {
        let off = build_wsman_dash_command(TARGET_IP, USERNAME, SECRET, PowerAction::Off)
            .expect("valid password should build");
        assert!(off.contains("wsman invoke -h 172.16.3.92 -P 16992"));
        assert!(off.contains("RequestPowerStateChange"));
        assert!(off.contains("PowerState=8"));

        let status = build_wsman_dash_command(TARGET_IP, USERNAME, SECRET, PowerAction::Status)
            .expect("valid password should build");
        assert!(status.contains("enumerate"));
        assert!(status.contains("CIM_AssociatedPowerManagementService"));
        assert!(!status.contains("RequestPowerStateChange"));
    }

    #[test]
    fn test_build_dashcli_command_shape() {
        let on = build_dashcli_command(TARGET_IP, USERNAME, SECRET, PowerAction::On)
            .expect("valid password should build");
        assert!(on.contains("dashcli -h 172.16.3.92:16992"));
        assert!(on.contains("power on"));
    }

    #[test]
    fn test_builders_reject_bad_password() {
        for password in ["", "a'b"] {
            let wsman = build_wsman_dash_command(TARGET_IP, USERNAME, password, PowerAction::On);
            assert!(matches!(wsman, Err(AutoInstallError::ConfigError(_))));
            let dashcli =
                build_dashcli_command(TARGET_IP, USERNAME, password, PowerAction::On);
            assert!(matches!(dashcli, Err(AutoInstallError::ConfigError(_))));
        }
    }

    #[test]
    fn test_redacted_omits_password() {
        for via_dashcli in [true, false] {
            let redacted =
                redacted_dash_command(TARGET_IP, USERNAME, PowerAction::Off, via_dashcli);
            assert!(!redacted.contains(SECRET));
            assert!(!redacted.contains("'test-secret'"));
            assert!(redacted.contains(TARGET_IP));
        }
    }

    #[tokio::test]
    async fn test_run_dash_missing_password() {
        let mut mock = MockExecutor::new(&[]);
        let result = run_dash_action(&mut mock, "len-serv-002", None, PowerAction::On).await;
        let err = result.expect_err("missing password must fail");
        assert!(err.to_string().contains("UAA_DASH_PASSWORD"));
        assert_eq!(mock.recorded.len(), 0);
    }

    #[tokio::test]
    async fn test_run_dash_fallback_to_wsman() {
        // dashcli absent: probe response is empty (falsy).
        let mut mock = MockExecutor::new(&[(dash_probe_command().as_str(), "")]);
        let result =
            run_dash_action(&mut mock, "len-serv-002", Some(SECRET), PowerAction::On).await;
        assert!(result.is_ok());

        let expected_cmd =
            build_wsman_dash_command("len-serv-002", DASH_DEFAULT_USERNAME, SECRET, PowerAction::On)
                .unwrap();
        assert_eq!(mock.recorded, vec![dash_probe_command(), expected_cmd]);
    }

    #[tokio::test]
    async fn test_run_dash_prefers_dashcli() {
        // dashcli present: probe response is non-empty.
        let mut mock =
            MockExecutor::new(&[(dash_probe_command().as_str(), "/usr/bin/dashcli")]);
        let result =
            run_dash_action(&mut mock, "len-serv-002", Some(SECRET), PowerAction::On).await;
        assert!(result.is_ok());

        let expected_cmd =
            build_dashcli_command("len-serv-002", DASH_DEFAULT_USERNAME, SECRET, PowerAction::On)
                .unwrap();
        assert_eq!(mock.recorded, vec![dash_probe_command(), expected_cmd]);
    }

    #[test]
    fn test_parse_dash_status() {
        let on_xml = "<p:PowerState>2</p:PowerState>";
        assert_eq!(parse_dash_status(on_xml).unwrap(), "on");

        let off_xml = "<p:PowerState>8</p:PowerState>";
        assert_eq!(parse_dash_status(off_xml).unwrap(), "off");

        let err = parse_dash_status("garbage-nonsense-output").unwrap_err();
        assert!(!err.to_string().contains(SECRET));
    }

    #[tokio::test]
    async fn test_run_dash_status_happy_path() {
        // Anti-over-suppression: a legitimate Status request must flow
        // through every guard (password check, probe, builder, executor,
        // parser) and return Ok("on") — the guard stack must not block the
        // happy path.
        let probe_cmd = dash_probe_command();
        let status_cmd = build_wsman_dash_command(
            "len-serv-002",
            DASH_DEFAULT_USERNAME,
            SECRET,
            PowerAction::Status,
        )
        .unwrap();
        let mut mock = MockExecutor::new(&[
            (probe_cmd.as_str(), ""), // dashcli absent -> wsman
            (status_cmd.as_str(), "<p:PowerState>2</p:PowerState>"),
        ]);

        let result =
            run_dash_action(&mut mock, "len-serv-002", Some(SECRET), PowerAction::Status).await;
        assert_eq!(result.unwrap(), "on");
    }
}
