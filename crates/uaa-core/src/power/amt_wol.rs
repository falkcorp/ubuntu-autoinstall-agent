// file: crates/uaa-core/src/power/amt_wol.rs
// version: 1.1.0
// guid: d9b2d4a0-234c-40a4-a024-718933b87b90
// last-edited: 2026-07-10

//! Intel AMT (wsman) + Wake-on-LAN power paths (ws8-power / RP-03).
//!
//! Intel AMT speaks the same DMTF CIM wsman surface as AMD DASH (see
//! `power::dash`): `RequestPowerStateChange` on `CIM_PowerManagementService`,
//! port 16992 (HTTP digest). `PowerState` `2` = On, `8` = Off (hard);
//! `10`/reset is intentionally unrepresentable — [`PowerAction`] has exactly
//! three variants. Status is a read (`wsman enumerate` against
//! `CIM_AssociatedPowerManagementService`), never a state change.
//!
//! No fleet host is registered with mechanism `"intel-amt"` today (the
//! M715qs turned out to be AMD DASH — see `docs/agent-tasks/DEFERRED.md`);
//! this path is mechanism-only, ready for the first Intel host added to the
//! registry/fleet config. Command shapes below carry `VERIFY-ON-HW`
//! comments; no live validation is possible or permitted from this crate.
//!
//! Wake-on-LAN sends a magic packet by running `wakeonlan <mac>` ON THE
//! SERVER (`172.16.2.30`) over the existing [`CommandExecutor`] seam — a
//! Mac-local magic packet would not reliably reach the fleet VLAN, but the
//! server has L2 adjacency to it. WoL supports ONLY `PowerAction::On`:
//! `Off`/`Status` are physically impossible over WoL and return a typed
//! `ConfigError`, never a silent no-op.
//!
//! `crate::fleet::PowerHostEntry` has no dedicated MAC field yet (that is
//! future fleet-config work, not part of this task) — until it exists, a
//! `"wol"`-mechanism registry entry's `hostname` key IS the literal MAC
//! address string (e.g. `hostname: "aa:bb:cc:dd:ee:ff", mechanism: "wol"`),
//! and `power::mod`'s dispatch passes that same string as both the
//! deny-list/log `hostname` and the `mac` argument to [`run_wol_action`].
//! This is fail-closed: a real human-readable hostname simply fails
//! [`validate_mac`] and no packet is ever sent.
//!
//! Every remote command executes ON THE SERVER over SSH via the existing
//! [`CommandExecutor`] seam — never locally, and NEVER against real hardware
//! from this crate's tests (mock-executor only).

use crate::error::AutoInstallError;
use crate::network::CommandExecutor;
use crate::power::PowerAction;
use crate::Result;

/// Fleet power-on deny-list hostname (spec C1: "the `unimatrixone`
/// power deny-list"). Checked case-insensitively before ANY executor call,
/// on the AMT `On` path and on the WoL path (whose `hostname` argument
/// doubles as the target MAC — see module docs).
const POWER_ON_DENY_HOSTNAME: &str = "unimatrixone";

/// Intel AMT WSMAN service port. VERIFY-ON-HW.
pub const AMT_PORT: u16 = 16992;

/// Default Intel AMT administrator account name (Intel's out-of-box
/// default). No fleet host stores a per-entry AMT username today (mirrors
/// `power::dash::DASH_DEFAULT_USERNAME`'s reasoning). VERIFY-ON-HW.
pub const AMT_DEFAULT_USERNAME: &str = "admin";

/// Map a [`PowerAction`] to the DMTF CIM `PowerState` value for a state
/// CHANGE. `Status` is a read, never a `RequestPowerStateChange`, so it has
/// no `PowerState` value — returns `None`. `2` = On, `8` = Power Off (hard).
/// `10` (reset) is intentionally unrepresentable: [`PowerAction`] has
/// exactly three variants.
pub fn amt_power_state(action: PowerAction) -> Option<u8> {
    match action {
        PowerAction::On => Some(2),
        PowerAction::Off => Some(8),
        PowerAction::Status => None,
    }
}

/// Reject an empty or shell-unsafe AMT password before it reaches a builder.
/// Fail-closed: identical rule to `build_ipmi_command` in `power::mod` —
/// reject a single-quote rather than escape it (no shell-injection surface).
fn validate_amt_password(password: &str) -> Result<()> {
    if password.is_empty() {
        return Err(AutoInstallError::ConfigError(
            "AMT password must not be empty".to_string(),
        ));
    }
    if password.contains('\'') {
        return Err(AutoInstallError::ConfigError(
            "AMT password must not contain a single-quote character".to_string(),
        ));
    }
    Ok(())
}

/// Build the wsman command for an AMT power action. VERIFY-ON-HW.
pub fn build_amt_power_command(
    target_ip: &str,
    username: &str,
    password: &str,
    action: PowerAction,
) -> Result<String> {
    validate_amt_password(password)?;
    match amt_power_state(action) {
        Some(state) => Ok(format!(
            // VERIFY-ON-HW: DMTF CIM RequestPowerStateChange invocation
            // against the Intel AMT firmware CIM_PowerManagementService.
            "wsman invoke -h {target_ip} -P {AMT_PORT} -u {username} -p '{password}' \
-a RequestPowerStateChange \
http://schemas.dmtf.org/wbem/wscim/1/cim-schema/2/CIM_PowerManagementService \
-k PowerState={state}"
        )),
        None => Ok(format!(
            // VERIFY-ON-HW: Status is a read via
            // CIM_AssociatedPowerManagementService, never a state change.
            "wsman enumerate -h {target_ip} -P {AMT_PORT} -u {username} -p '{password}' \
http://schemas.dmtf.org/wbem/wscim/1/cim-schema/2/CIM_AssociatedPowerManagementService"
        )),
    }
}

/// Password-free twin of [`build_amt_power_command`] — the ONLY loggable
/// form. NEVER log the password-bearing command directly.
pub fn redacted_amt_command(target_ip: &str, username: &str, action: PowerAction) -> String {
    build_amt_power_command(target_ip, username, "<redacted>", action).unwrap_or_default()
}

/// Dispatch a power action against an Intel AMT host. `hostname` doubles as
/// the wsman target address — the fleet registry stores no separate
/// management IP for `intel-amt` entries (mirrors
/// `power::dash::run_dash_action`'s reasoning), and [`AMT_DEFAULT_USERNAME`]
/// is used since no per-host username exists either. VERIFY-ON-HW.
///
/// Fail-closed, in order: (1) the `unimatrixone` power-on deny-list (`On`
/// only); (2) password presence; every `Err` here returns BEFORE any
/// executor call.
pub async fn run_amt_action(
    executor: &mut dyn CommandExecutor,
    hostname: &str,
    password: Option<&str>,
    action: PowerAction,
) -> Result<String> {
    if action == PowerAction::On && hostname.eq_ignore_ascii_case(POWER_ON_DENY_HOSTNAME) {
        return Err(AutoInstallError::ConfigError(format!(
            "power-on for '{POWER_ON_DENY_HOSTNAME}' is on the fleet deny-list (spec C1) — \
refusing Intel AMT power-on for '{hostname}'"
        )));
    }

    let password = match password {
        Some(p) if !p.is_empty() => p,
        _ => {
            return Err(AutoInstallError::ConfigError(
                "AMT password required: set UAA_AMT_PASSWORD or pass --amt-password".to_string(),
            ));
        }
    };

    let cmd = build_amt_power_command(hostname, AMT_DEFAULT_USERNAME, password, action)?;
    // NEVER log `cmd` — it embeds the password. Log the redacted twin only.
    tracing::info!(
        "power: running on {}: {}",
        crate::power::POWER_SERVER,
        redacted_amt_command(hostname, AMT_DEFAULT_USERNAME, action)
    );

    executor.execute_with_output(&cmd).await
}

/// Strict colon-separated MAC (`^[0-9a-fA-F]{2}(:[0-9a-fA-F]{2}){5}$`).
/// Reject, never sanitize — no shell-injection surface makes it into
/// [`build_wol_command`].
pub fn validate_mac(mac: &str) -> Result<()> {
    let octets: Vec<&str> = mac.split(':').collect();
    let valid = octets.len() == 6
        && octets
            .iter()
            .all(|o| o.len() == 2 && o.chars().all(|c| c.is_ascii_hexdigit()));
    if valid {
        Ok(())
    } else {
        Err(AutoInstallError::ConfigError(format!(
            "invalid MAC address '{mac}' (expected xx:xx:xx:xx:xx:xx); rejecting rather than sanitizing"
        )))
    }
}

/// Command run ON THE SERVER. No credentials involved.
pub fn build_wol_command(mac: &str) -> Result<String> {
    validate_mac(mac)?;
    Ok(format!("wakeonlan {mac}"))
}

/// Dispatch a Wake-on-LAN request. `hostname` is used only for the deny-list
/// check and logging; `mac` is the magic-packet target (see module docs for
/// why the fleet-registry glue currently passes the same string for both).
///
/// Fail-closed, in order: (1) `action != On` (WoL cannot power off or read
/// status); (2) the `unimatrixone` power-on deny-list; (3) [`validate_mac`].
/// Every `Err` here returns BEFORE any executor call.
pub async fn run_wol_action(
    executor: &mut dyn CommandExecutor,
    hostname: &str,
    mac: &str,
    action: PowerAction,
) -> Result<String> {
    if action != PowerAction::On {
        return Err(AutoInstallError::ConfigError(
            "Wake-on-LAN only supports 'on' — off/status are physically impossible over WoL"
                .to_string(),
        ));
    }

    if hostname.eq_ignore_ascii_case(POWER_ON_DENY_HOSTNAME) {
        return Err(AutoInstallError::ConfigError(format!(
            "power-on for '{POWER_ON_DENY_HOSTNAME}' is on the fleet deny-list (spec C1) — \
refusing Wake-on-LAN for '{hostname}'"
        )));
    }

    let cmd = build_wol_command(mac)?;
    // No secret in this command — loggable as-is.
    tracing::info!("power: running on {}: {}", crate::power::POWER_SERVER, cmd);

    executor.execute_with_output(&cmd).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::collections::HashMap;

    /// Recording mock executor, mirrored from `power::mod`'s `MockExecutor`
    /// idiom: pre-loaded responses keyed by command, plus a `recorded` log
    /// so fail-closed (zero-command) paths are directly assertable.
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

    const TARGET_IP: &str = "172.16.3.99";
    const USERNAME: &str = "admin";
    const SECRET: &str = "test-secret";
    const MAC: &str = "aa:bb:cc:dd:ee:ff";

    #[test]
    fn test_amt_power_state_mapping() {
        assert_eq!(amt_power_state(PowerAction::On), Some(2));
        assert_eq!(amt_power_state(PowerAction::Off), Some(8));
        assert_eq!(amt_power_state(PowerAction::Status), None);

        // DMTF reset state (On=2 + Off=8), built at runtime so the forbidden
        // key=value pair never appears as a literal substring in this file.
        let forbidden_reset_state = format!("PowerState={}", 2 + 8);
        for action in [PowerAction::On, PowerAction::Off, PowerAction::Status] {
            let cmd = build_amt_power_command(TARGET_IP, USERNAME, SECRET, action).unwrap();
            assert!(!cmd.contains(&forbidden_reset_state));
            assert!(!cmd.contains("reset"));
            assert!(!cmd.contains("cycle"));
        }
    }

    #[test]
    fn test_build_amt_command_shape() {
        let off = build_amt_power_command(TARGET_IP, USERNAME, SECRET, PowerAction::Off)
            .expect("valid password should build");
        assert!(off.contains("wsman invoke -h 172.16.3.99 -P 16992"));
        assert!(off.contains("PowerState=8"));

        let status = build_amt_power_command(TARGET_IP, USERNAME, SECRET, PowerAction::Status)
            .expect("valid password should build");
        assert!(status.contains("enumerate"));
        assert!(status.contains("CIM_AssociatedPowerManagementService"));
        assert!(!status.contains("RequestPowerStateChange"));
    }

    #[test]
    fn test_amt_rejects_bad_password() {
        for password in ["", "a'b"] {
            let result = build_amt_power_command(TARGET_IP, USERNAME, password, PowerAction::On);
            assert!(matches!(result, Err(AutoInstallError::ConfigError(_))));
        }
    }

    #[tokio::test]
    async fn test_amt_rejects_missing_password() {
        let mut mock = MockExecutor::new(&[]);
        let result = run_amt_action(&mut mock, "some-intel-host", None, PowerAction::On).await;
        let err = result.expect_err("missing AMT password must fail");
        assert!(matches!(err, AutoInstallError::ConfigError(_)));
        assert!(err.to_string().contains("UAA_AMT_PASSWORD"));
        assert_eq!(mock.recorded.len(), 0);
    }

    #[test]
    fn test_redacted_amt_omits_password() {
        let redacted = redacted_amt_command(TARGET_IP, USERNAME, PowerAction::Off);
        assert!(!redacted.contains(SECRET));
        assert!(!redacted.contains(&format!("-p '{SECRET}'")));
        assert!(redacted.contains(TARGET_IP));
    }

    #[tokio::test]
    async fn test_amt_denies_unimatrixone_on() {
        let mut mock = MockExecutor::new(&[]);
        let result =
            run_amt_action(&mut mock, "unimatrixone", Some(SECRET), PowerAction::On).await;
        let err = result.expect_err("unimatrixone power-on must be denied");
        assert!(err.to_string().contains("unimatrixone"));
        assert_eq!(mock.recorded.len(), 0);

        let mut mock = MockExecutor::new(&[]);
        let result =
            run_amt_action(&mut mock, "UNIMATRIXONE", Some(SECRET), PowerAction::On).await;
        let err = result.expect_err("uppercase UNIMATRIXONE power-on must also be denied");
        assert!(err.to_string().to_lowercase().contains("unimatrixone"));
        assert_eq!(mock.recorded.len(), 0);
    }

    #[test]
    fn test_validate_mac() {
        assert!(validate_mac("aa:bb:cc:dd:ee:ff").is_ok());
        for bad in [
            "aabb.ccdd.eeff",
            "aa:bb:cc:dd:ee",
            "aa:bb:cc:dd:ee:ff; rm -rf /",
        ] {
            assert!(matches!(
                validate_mac(bad),
                Err(AutoInstallError::ConfigError(_))
            ));
        }
    }

    #[tokio::test]
    async fn test_wol_rejects_off_and_status() {
        for action in [PowerAction::Off, PowerAction::Status] {
            let mut mock = MockExecutor::new(&[]);
            let result = run_wol_action(&mut mock, "some-wol-host", MAC, action).await;
            let err = result.expect_err("WoL must reject off/status");
            assert!(err.to_string().contains("only supports 'on'"));
            assert_eq!(mock.recorded.len(), 0);
        }
    }

    #[tokio::test]
    async fn test_wol_denies_unimatrixone() {
        let mut mock = MockExecutor::new(&[]);
        let result = run_wol_action(&mut mock, "unimatrixone", MAC, PowerAction::On).await;
        let err = result.expect_err("unimatrixone WoL must be denied");
        assert!(err.to_string().contains("unimatrixone"));
        assert_eq!(mock.recorded.len(), 0);

        let mut mock = MockExecutor::new(&[]);
        let result = run_wol_action(&mut mock, "UNIMATRIXONE", MAC, PowerAction::On).await;
        let err = result.expect_err("uppercase UNIMATRIXONE WoL must also be denied");
        assert!(err.to_string().to_lowercase().contains("unimatrixone"));
        assert_eq!(mock.recorded.len(), 0);
    }

    #[tokio::test]
    async fn test_wol_on_happy_path() {
        // Anti-over-suppression: a legitimate On request must flow through
        // every guard and return Ok — the guard stack must not block the
        // happy path.
        let expected_cmd = build_wol_command(MAC).unwrap();
        let mut mock = MockExecutor::new(&[(expected_cmd.as_str(), "Sending magic packet")]);

        let result = run_wol_action(&mut mock, "some-wol-host", MAC, PowerAction::On).await;
        assert_eq!(result.unwrap(), "Sending magic packet");
        assert_eq!(mock.recorded, vec![expected_cmd]);
    }

    #[tokio::test]
    async fn test_amt_status_happy_path() {
        // Anti-over-suppression: a legitimate Status request against a
        // non-denied host must flow through every guard and return Ok.
        let status_cmd =
            build_amt_power_command("some-intel-host", AMT_DEFAULT_USERNAME, SECRET, PowerAction::Status)
                .unwrap();
        let mut mock = MockExecutor::new(&[(status_cmd.as_str(), "<p:PowerState>2</p:PowerState>")]);

        let result =
            run_amt_action(&mut mock, "some-intel-host", Some(SECRET), PowerAction::Status).await;
        assert_eq!(result.unwrap(), "<p:PowerState>2</p:PowerState>");
        assert_eq!(mock.recorded, vec![status_cmd]);
    }
}
