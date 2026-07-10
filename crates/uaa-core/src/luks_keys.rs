// file: crates/uaa-core/src/luks_keys.rs
// version: 1.2.0
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

// ── Tang t=2-of-3 cold-start quorum guard (spec Decision 14 / C8) ─────────────

/// The 3 Tang servers backing every SSS t=2-of-3 binding
/// (PLAN-zfs-luks-multikey.md). Overridable for tests.
pub const DEFAULT_TANG_URLS: [&str; 3] = [
    "http://172.16.2.45",
    "http://172.16.2.46",
    "http://172.16.2.47",
];

/// The `curl` adv-probe command run for one Tang server through the executor.
/// A healthy Tang answers `<url>/adv` with a JWS advertisement JSON; `-sf`
/// makes curl exit non-zero (→ executor `Err`, counted invalid) on any HTTP
/// error, and `--max-time 5` bounds a hung server.
fn tang_adv_command(url: &str) -> String {
    format!("curl -sf --max-time 5 {url}/adv")
}

/// The keyslot-wipe command. `systemd-cryptenroll --wipe-slot` (not
/// `cryptsetup luksKillSlot`) so no passphrase prompt is needed under
/// `sudo -n`.
fn wipe_slot_command(luks_dev: &str, slot: u32) -> String {
    format!("sudo -n systemd-cryptenroll --wipe-slot={slot} {luks_dev}")
}

/// The clevis SSS re-bind command run against ONE fleet host's executor during
/// a Tang server-key rotation sweep.
fn clevis_regen_command(luks_dev: &str, slot: u32) -> String {
    format!("sudo -n clevis luks regen -d {luks_dev} -s {slot} -q")
}

/// Compare a typed-hostname override against the target. Strips ONE trailing
/// `\r\n` or `\n` (a human's `read`/echo newline), then byte-equality — no
/// lowercasing, no trimming, no substring, no `y`/`yes`. `None`, `Some("")`,
/// `Some("yes")`, and a case-mismatched hostname all return `false`.
fn override_matches(override_confirmation: Option<&str>, target_hostname: &str) -> bool {
    match override_confirmation {
        Some(s) => {
            let s = s
                .strip_suffix("\r\n")
                .or_else(|| s.strip_suffix('\n'))
                .unwrap_or(s);
            s == target_hostname
        }
        None => false,
    }
}

/// Probe each Tang adv via `curl -sf --max-time 5 <url>/adv` through the
/// executor and return the count that answered with a non-empty advertisement.
/// A probe failure (executor `Err` OR empty body) counts as invalid — it is
/// NEVER propagated as an `Err`; a degraded fleet must surface as a low count,
/// not a hard error, so the caller's fail-closed logic runs.
pub async fn check_tang_quorum(
    executor: &mut dyn crate::network::CommandExecutor,
    tang_urls: &[&str],
) -> crate::error::Result<usize> {
    let mut valid = 0usize;
    for url in tang_urls {
        let cmd = tang_adv_command(url);
        match executor.execute_with_output(&cmd).await {
            Ok(body) if !body.trim().is_empty() => valid += 1,
            _ => {}
        }
    }
    Ok(valid)
}

/// Fail-closed gate for EVERY destructive op in this module. Passes when
/// `check_tang_quorum >= 2`, OR when `override_confirmation` equals the exact
/// target hostname (see [`override_matches`]). A `valid < 2` fleet with a
/// wrong/empty/`None` override returns `Err(SystemError)` naming the counts
/// and the typed-hostname rule. NEVER logs the override value.
pub async fn require_tang_quorum(
    executor: &mut dyn crate::network::CommandExecutor,
    tang_urls: &[&str],
    target_hostname: &str,
    override_confirmation: Option<&str>,
) -> crate::error::Result<()> {
    let valid = check_tang_quorum(executor, tang_urls).await?;
    if valid >= 2 {
        return Ok(());
    }
    if override_matches(override_confirmation, target_hostname) {
        // Log the fact and the count only — never the override string itself.
        tracing::warn!(
            "luks: Tang quorum {valid}/{} below t=2 but bypassed by typed-hostname override for {target_hostname}",
            tang_urls.len()
        );
        return Ok(());
    }
    Err(AutoInstallError::SystemError(format!(
        "Tang cold-start quorum not met: {valid} of {} advertisements valid (need >= 2). \
         Refusing every destructive keyslot op fail-closed. To override, the operator must \
         type the exact target hostname '{target_hostname}'.",
        tang_urls.len()
    )))
}

// ── State-file read/update helpers (LK-01 atomic tmp+rename contract) ─────────

/// Read the JSON array at `state_path` (missing or empty file => `vec![]`).
fn read_records(state_path: &std::path::Path) -> crate::error::Result<Vec<LuksCredentialRecord>> {
    if !state_path.exists() {
        return Ok(Vec::new());
    }
    let contents = std::fs::read_to_string(state_path)?;
    if contents.trim().is_empty() {
        return Ok(Vec::new());
    }
    Ok(serde_json::from_str(&contents)?)
}

/// Write `records` back atomically: serialize to `<state_path>.tmp`, then
/// `std::fs::rename` over the target. Same contract as LK-01's `append_state`
/// (never truncate-then-write in place).
fn write_records_atomic(
    state_path: &std::path::Path,
    records: &[LuksCredentialRecord],
) -> crate::error::Result<()> {
    let json = serde_json::to_string_pretty(records)?;
    let mut tmp_name = state_path.as_os_str().to_owned();
    tmp_name.push(".tmp");
    let tmp_path = std::path::PathBuf::from(tmp_name);
    std::fs::write(&tmp_path, json)?;
    std::fs::rename(&tmp_path, state_path)?;
    Ok(())
}

/// Look up the active (un-revoked) keyslot bound to `yubikey_serial`.
fn lookup_active_slot(
    state_path: &std::path::Path,
    yubikey_serial: &str,
) -> crate::error::Result<u32> {
    let records = read_records(state_path)?;
    let record = records
        .iter()
        .find(|r| r.yubikey_serial == yubikey_serial && r.revoked_at.is_none())
        .ok_or_else(|| {
            AutoInstallError::ConfigError(format!(
                "no active LUKS credential recorded for YubiKey serial '{yubikey_serial}'"
            ))
        })?;
    record.luks_keyslot.ok_or_else(|| {
        AutoInstallError::ConfigError(format!(
            "recorded credential for serial '{yubikey_serial}' has no keyslot; cannot revoke"
        ))
    })
}

/// Set `revoked_at` (RFC3339) on the FIRST active record for `yubikey_serial`
/// and persist via the atomic tmp+rename write.
fn mark_revoked(state_path: &std::path::Path, yubikey_serial: &str) -> crate::error::Result<()> {
    let mut records = read_records(state_path)?;
    let target = records
        .iter_mut()
        .find(|r| r.yubikey_serial == yubikey_serial && r.revoked_at.is_none())
        .ok_or_else(|| {
            AutoInstallError::ConfigError(format!(
                "no active LUKS credential recorded for YubiKey serial '{yubikey_serial}' to revoke"
            ))
        })?;
    target.revoked_at = Some(chrono::Utc::now().to_rfc3339());
    write_records_atomic(state_path, &records)
}

// ── revoke ────────────────────────────────────────────────────────────────────

/// Internal wipe leg shared by [`revoke_fido2`] and [`rotate_fido2`]. Assumes
/// the Tang quorum has ALREADY been proven this call. Applies the last-method
/// guard (guard #2), then wipes, verifies the token is gone, and records
/// `revoked_at`. NEVER re-probes quorum.
///
/// Last-method guard: count the fido2 tokens whose keyslot is NOT the one being
/// wiped. Zero remaining => the wipe would leave the header with no
/// systemd-fido2 unlock method at all — refuse (`ConfigError`) unless the
/// typed-hostname override is supplied. Revoking 1 of 3 (2 remain) never trips.
async fn wipe_slot_guarded(
    executor: &mut dyn crate::network::CommandExecutor,
    luks_dev: &str,
    yubikey_serial: &str,
    slot: u32,
    target_hostname: &str,
    override_confirmation: Option<&str>,
    state_path: &std::path::Path,
) -> crate::error::Result<()> {
    // Guard #2: last-method. Read-only luksDump — not a destructive command.
    let before = executor.execute_with_output(&dump_command(luks_dev)).await?;
    let would_remain = parse_fido2_tokens(&before)
        .into_iter()
        .filter(|t| t.keyslot != Some(slot))
        .count();
    if would_remain == 0 && !override_matches(override_confirmation, target_hostname) {
        return Err(AutoInstallError::ConfigError(format!(
            "refusing to wipe keyslot {slot}: it is the LAST systemd-fido2 token in {luks_dev} \
             and revoking it would leave the disk with no FIDO2 unlock method. To override, the \
             operator must type the exact target hostname '{target_hostname}'."
        )));
    }

    // Destructive: wipe the keyslot.
    executor.execute(&wipe_slot_command(luks_dev, slot)).await?;

    // Verify the token is GONE before we record the revocation.
    let after = executor.execute_with_output(&dump_command(luks_dev)).await?;
    if parse_fido2_tokens(&after).iter().any(|t| t.keyslot == Some(slot)) {
        return Err(AutoInstallError::SystemError(format!(
            "wipe-slot ran but a systemd-fido2 token still binds keyslot {slot} on {luks_dev}"
        )));
    }

    mark_revoked(state_path, yubikey_serial)?;
    Ok(())
}

/// Wipe the keyslot bound to `yubikey_serial` (looked up in the state file).
///
/// Guards, in order, each BEFORE any wipe command: (1)
/// [`require_tang_quorum`] (fail-closed t=2-of-3), then (2) the last-method
/// guard (see [`wipe_slot_guarded`]). Then `sudo -n systemd-cryptenroll
/// --wipe-slot=<n> <dev>`, verify via luksDump the token is gone, and set
/// `revoked_at` on the state record via the atomic tmp+rename write.
#[allow(clippy::too_many_arguments)] // signature fixed by luks-keys/TASK-02 brief
pub async fn revoke_fido2(
    executor: &mut dyn crate::network::CommandExecutor,
    luks_dev: &str,
    yubikey_serial: &str,
    target_hostname: &str,
    tang_urls: &[&str],
    override_confirmation: Option<&str>,
    state_path: &std::path::Path,
) -> crate::error::Result<()> {
    validate_luks_dev(luks_dev)?;
    if yubikey_serial.is_empty() {
        return Err(AutoInstallError::ConfigError(
            "YubiKey serial must not be empty".to_string(),
        ));
    }

    // Guard #1: fail-closed quorum, BEFORE any destructive command.
    require_tang_quorum(executor, tang_urls, target_hostname, override_confirmation).await?;

    let slot = lookup_active_slot(state_path, yubikey_serial)?;
    wipe_slot_guarded(
        executor,
        luks_dev,
        yubikey_serial,
        slot,
        target_hostname,
        override_confirmation,
        state_path,
    )
    .await
}

// ── rotate (enroll-NEW-then-revoke-OLD — never reverse, spec C8) ──────────────

/// ENROLL-NEW-THEN-REVOKE-OLD — never reverse (spec C8). Runs
/// [`require_tang_quorum`] ONCE at entry (a rotation must not start against a
/// degraded fleet), then calls LK-01's [`enroll_fido2`] for `new_serial`; ONLY
/// on its `Ok` (the new token is proven present in the LUKS header) does the
/// revoke leg run against `old_serial`. An enroll-leg failure returns its
/// `Err` with the OLD credential completely untouched and no wipe issued.
#[allow(clippy::too_many_arguments)] // signature fixed by luks-keys/TASK-02 brief
pub async fn rotate_fido2(
    executor: &mut dyn crate::network::CommandExecutor,
    luks_dev: &str,
    old_serial: &str,
    new_serial: &str,
    role: CredentialRole,
    passphrase: &str,
    target_hostname: &str,
    tang_urls: &[&str],
    override_confirmation: Option<&str>,
    state_path: &std::path::Path,
) -> crate::error::Result<LuksCredentialRecord> {
    validate_luks_dev(luks_dev)?;
    if old_serial.is_empty() || new_serial.is_empty() {
        return Err(AutoInstallError::ConfigError(
            "old and new YubiKey serials must not be empty".to_string(),
        ));
    }

    // Quorum proven ONCE at entry — covers both the enroll and revoke legs of
    // this single rotation call.
    require_tang_quorum(executor, tang_urls, target_hostname, override_confirmation).await?;

    // Resolve the old keyslot BEFORE enrolling so a bad/unknown old serial
    // fails with the old credential still fully intact (nothing enrolled yet).
    let old_slot = lookup_active_slot(state_path, old_serial)?;

    // ENROLL NEW FIRST. enroll_fido2 self-verifies the new token appears in the
    // header; on any failure it returns Err and appends nothing — old key
    // untouched.
    let new_record = enroll_fido2(executor, luks_dev, role, new_serial, passphrase, state_path).await?;

    // Only now — new key PROVEN present — retire the old one. Quorum already
    // proven this call, so the wipe leg re-checks only the last-method guard
    // (which cannot trip here: the freshly enrolled token remains).
    wipe_slot_guarded(
        executor,
        luks_dev,
        old_serial,
        old_slot,
        target_hostname,
        override_confirmation,
        state_path,
    )
    .await?;

    Ok(new_record)
}

// ── rotate-tang: fleet sweep-before-retire state machine (Decision 14) ────────

/// Enforces sweep-before-retire in STATE, not prose. Constructed with the full
/// fleet host list; [`TangRotation::retire_old_key`] is unreachable (returns
/// `Err`) until every host is marked rebound. Server-side Tang key retirement
/// is an operator action outside this binary — this type gates and DESCRIBES
/// it, it never executes it.
#[derive(Debug, Clone)]
pub struct TangRotation {
    /// host -> rebound? Ordered so `pending_hosts` is deterministic.
    hosts: std::collections::BTreeMap<String, bool>,
}

impl TangRotation {
    /// An empty fleet is a config bug (a sweep over nobody is not a free pass
    /// to retire) => `ConfigError`.
    pub fn new(fleet_hosts: &[String]) -> crate::error::Result<Self> {
        if fleet_hosts.is_empty() {
            return Err(AutoInstallError::ConfigError(
                "TangRotation requires a non-empty fleet host list; a sweep over zero hosts must \
                 not be able to authorize old-key retirement"
                    .to_string(),
            ));
        }
        let hosts = fleet_hosts
            .iter()
            .map(|h| (h.clone(), false))
            .collect::<std::collections::BTreeMap<String, bool>>();
        Ok(Self { hosts })
    }

    /// Re-bind ONE host's SSS pin via `sudo -n clevis luks regen -d <dev> -s
    /// <slot> -q`, using an `executor` already connected to that host. Gated by
    /// [`require_tang_quorum`] against the host (the NEW Tang key must already
    /// be advertised — quorum with the new key present proves re-bind can
    /// succeed). On `Ok` the host is marked rebound. Unknown host =>
    /// `ConfigError`.
    pub async fn rebind_host(
        &mut self,
        executor: &mut dyn crate::network::CommandExecutor,
        host: &str,
        luks_dev: &str,
        slot: u32,
    ) -> crate::error::Result<()> {
        validate_luks_dev(luks_dev)?;
        if !self.hosts.contains_key(host) {
            return Err(AutoInstallError::ConfigError(format!(
                "host '{host}' is not part of this Tang rotation fleet"
            )));
        }
        // The new Tang key must already be advertised for this host's binding —
        // no typed-hostname override here: a re-bind against a degraded fleet
        // would produce an SSS pin that cannot meet quorum on next boot.
        require_tang_quorum(executor, &DEFAULT_TANG_URLS, host, None).await?;

        executor
            .execute(&clevis_regen_command(luks_dev, slot))
            .await?;
        self.hosts.insert(host.to_string(), true);
        Ok(())
    }

    /// Hosts that have NOT yet been re-bound, sorted.
    pub fn pending_hosts(&self) -> Vec<String> {
        self.hosts
            .iter()
            .filter(|(_, rebound)| !**rebound)
            .map(|(host, _)| host.clone())
            .collect()
    }

    /// Return the retire PLAN string ONLY when [`pending_hosts`](Self::pending_hosts)
    /// is empty; otherwise `Err(SystemError)` listing the un-rebound hosts.
    /// This never executes retirement (no executor argument) — sweep-before-
    /// retire is unreachable-early by construction.
    pub fn retire_old_key(&self) -> crate::error::Result<String> {
        let pending = self.pending_hosts();
        if !pending.is_empty() {
            return Err(AutoInstallError::SystemError(format!(
                "cannot retire the old Tang server key: {} of {} fleet host(s) not yet re-bound: {}",
                pending.len(),
                self.hosts.len(),
                pending.join(", ")
            )));
        }
        Ok(format!(
            "All {} fleet host(s) re-bound to the new Tang key. Safe to retire the old server-side \
             Tang key (operator action, outside this binary): rotate keys in /var/db/tang on the \
             retiring Tang server and run `tang-show-keys`/`systemctl restart tangd.socket`.",
            self.hosts.len()
        ))
    }
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

    // ── LK-02: Tang quorum guard + revoke/rotate/rotate-tang ─────────────────

    const TANG_A: &str = "http://tang-a";
    const TANG_B: &str = "http://tang-b";
    const TANG_C: &str = "http://tang-c";
    const TANG_URLS: [&str; 3] = [TANG_A, TANG_B, TANG_C];
    const HOST: &str = "len-serv-001";
    /// A non-empty stand-in for a real JWS advertisement body.
    const ADV: &str = r#"{"payload":"e30","signatures":[{"protected":"e30"}]}"#;

    /// luksDump with two systemd-fido2 tokens (slots 2 and 5).
    const DUMP_TWO: &str = "\
Tokens:
  1: systemd-fido2
        Keyslot:    2
  3: systemd-fido2
        Keyslot:    5
Keyslots:
  2: luks2
  5: luks2
";
    /// luksDump with one systemd-fido2 token (slot 5) — slot 2 gone.
    const DUMP_ONE_SLOT5: &str = "\
Tokens:
  3: systemd-fido2
        Keyslot:    5
Keyslots:
  5: luks2
";
    /// luksDump with a single systemd-fido2 token (slot 2) — the last method.
    const DUMP_ONLY_SLOT2: &str = "\
Tokens:
  1: systemd-fido2
        Keyslot:    2
Keyslots:
  2: luks2
";
    /// luksDump with zero fido2 tokens.
    const DUMP_NONE: &str = "\
Tokens:
  0: clevis
        Keyslot:    1
Keyslots:
  1: luks2
";

    /// Seed a state file with one active credential (serial, role, slot).
    fn seed_state(state_path: &std::path::Path, serial: &str, role: CredentialRole, slot: u32) {
        let records = vec![LuksCredentialRecord {
            yubikey_serial: serial.to_string(),
            role,
            luks_keyslot: Some(slot),
            enrolled_at: "2026-07-10T00:00:00Z".to_string(),
            revoked_at: None,
        }];
        std::fs::write(state_path, serde_json::to_string_pretty(&records).unwrap()).unwrap();
    }

    fn count_wipes(recorded: &[String]) -> usize {
        recorded.iter().filter(|c| c.contains("--wipe-slot")).count()
    }

    // ── check_tang_quorum / require_tang_quorum ──────────────────────────────

    #[tokio::test]
    async fn test_quorum_counts_valid_advs() {
        // 3 mocked adv responses, one (TANG_C) failing (empty) → count == 2.
        let mut mock = MockExecutor::new()
            .queue(&tang_adv_command(TANG_A), ADV)
            .queue(&tang_adv_command(TANG_B), ADV)
            .queue(&tang_adv_command(TANG_C), "");
        let valid = check_tang_quorum(&mut mock, &TANG_URLS).await.unwrap();
        assert_eq!(valid, 2);
    }

    #[tokio::test]
    async fn test_quorum_fail_closed() {
        // 1-of-3 valid, no override → require_tang_quorum Err naming counts.
        let mut mock = MockExecutor::new().queue(&tang_adv_command(TANG_A), ADV);
        let err = require_tang_quorum(&mut mock, &TANG_URLS, HOST, None)
            .await
            .unwrap_err();
        assert!(matches!(err, AutoInstallError::SystemError(_)));
        let msg = err.to_string();
        assert!(msg.contains('1') && msg.contains('3'));
        assert!(msg.contains("hostname"));

        // Downstream revoke_fido2 on the same degraded fleet records ZERO
        // wipe commands.
        let dir = tempfile::tempdir().unwrap();
        let state_path = dir.path().join("luks-credentials.json");
        seed_state(&state_path, "12345678", CredentialRole::Primary, 2);
        let mut mock = MockExecutor::new().queue(&tang_adv_command(TANG_A), ADV);
        let err = revoke_fido2(
            &mut mock,
            DEV,
            "12345678",
            HOST,
            &TANG_URLS,
            None,
            &state_path,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, AutoInstallError::SystemError(_)));
        assert_eq!(count_wipes(&mock.recorded), 0);
    }

    #[tokio::test]
    async fn test_override_exact_hostname_only() {
        // valid < 2 for every sub-case; only the EXACT hostname bypasses.
        // Fresh mock per sub-assertion (each re-probes the quorum).
        let ok = require_tang_quorum(
            &mut MockExecutor::new().queue(&tang_adv_command(TANG_A), ADV),
            &TANG_URLS,
            HOST,
            Some(HOST),
        )
        .await;
        assert!(ok.is_ok(), "exact hostname must bypass fail-closed guard");

        // Exact hostname with a trailing newline (a human's echo) still bypasses.
        let ok_nl = require_tang_quorum(
            &mut MockExecutor::new().queue(&tang_adv_command(TANG_A), ADV),
            &TANG_URLS,
            HOST,
            Some("len-serv-001\n"),
        )
        .await;
        assert!(ok_nl.is_ok());

        for bad in [Some("yes"), Some(""), Some("LEN-SERV-001"), None] {
            let err = require_tang_quorum(
                &mut MockExecutor::new().queue(&tang_adv_command(TANG_A), ADV),
                &TANG_URLS,
                HOST,
                bad,
            )
            .await;
            assert!(err.is_err(), "override {bad:?} must NOT bypass the guard");
        }
    }

    // ── revoke_fido2 ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_revoke_last_method_guard() {
        let dir = tempfile::tempdir().unwrap();
        let state_path = dir.path().join("luks-credentials.json");
        seed_state(&state_path, "serial-last", CredentialRole::Primary, 2);

        // Header with exactly 1 fido2 token (slot 2), quorum OK (2-of-3),
        // no override → ConfigError, ZERO wipe commands.
        let mut mock = MockExecutor::new()
            .queue(&tang_adv_command(TANG_A), ADV)
            .queue(&tang_adv_command(TANG_B), ADV)
            .queue(&dump_command(DEV), DUMP_ONLY_SLOT2);
        let err = revoke_fido2(
            &mut mock,
            DEV,
            "serial-last",
            HOST,
            &TANG_URLS,
            None,
            &state_path,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, AutoInstallError::ConfigError(_)));
        assert!(err.to_string().contains("LAST"));
        assert_eq!(count_wipes(&mock.recorded), 0);

        // Same situation WITH the exact-hostname override → wipe proceeds.
        let mut mock = MockExecutor::new()
            .queue(&tang_adv_command(TANG_A), ADV)
            .queue(&tang_adv_command(TANG_B), ADV)
            .queue(&dump_command(DEV), DUMP_ONLY_SLOT2)
            .queue(&dump_command(DEV), DUMP_NONE);
        revoke_fido2(
            &mut mock,
            DEV,
            "serial-last",
            HOST,
            &TANG_URLS,
            Some(HOST),
            &state_path,
        )
        .await
        .expect("override must let the last-method wipe proceed");
        assert_eq!(count_wipes(&mock.recorded), 1);
        assert!(mock.recorded.contains(&wipe_slot_command(DEV, 2)));
    }

    #[tokio::test]
    async fn test_revoke_sets_revoked_at() {
        let dir = tempfile::tempdir().unwrap();
        let state_path = dir.path().join("luks-credentials.json");
        // Serial bound to slot 5; a second fido2 token (slot 2) also present so
        // the last-method guard does not trip.
        seed_state(&state_path, "serial-5", CredentialRole::Backup1, 5);

        let mut mock = MockExecutor::new()
            .queue(&tang_adv_command(TANG_A), ADV)
            .queue(&tang_adv_command(TANG_B), ADV)
            .queue(&dump_command(DEV), DUMP_TWO) // last-method count
            .queue(&dump_command(DEV), DUMP_ONLY_SLOT2); // verify: slot 5 gone, slot 2 remains
        revoke_fido2(
            &mut mock,
            DEV,
            "serial-5",
            HOST,
            &TANG_URLS,
            None,
            &state_path,
        )
        .await
        .expect("2-of-3 quorum revoke should succeed");

        // Wipe recorded exactly once, with the right slot.
        assert_eq!(count_wipes(&mock.recorded), 1);
        assert!(mock.recorded.contains(&wipe_slot_command(DEV, 5)));

        // State record's revoked_at is now Some; no .tmp left behind (rename).
        let records = read_records(&state_path).unwrap();
        assert_eq!(records.len(), 1);
        assert!(records[0].revoked_at.is_some());
        let mut tmp = state_path.as_os_str().to_owned();
        tmp.push(".tmp");
        assert!(!std::path::PathBuf::from(tmp).exists());
    }

    // ── rotate_fido2 ─────────────────────────────────────────────────────────

    /// Build the dump-response queue for a full rotation: enroll before/after
    /// then revoke last-method/verify.
    fn queue_rotation_dumps(mock: MockExecutor) -> MockExecutor {
        mock.queue(&dump_command(DEV), DUMP_ONLY_SLOT2) // enroll BEFORE (slot 2 only)
            .queue(&dump_command(DEV), DUMP_TWO) // enroll AFTER (new slot 5 appeared)
            .queue(&dump_command(DEV), DUMP_TWO) // revoke last-method (2 remain)
            .queue(&dump_command(DEV), DUMP_ONE_SLOT5) // revoke verify (slot 2 gone)
    }

    #[tokio::test]
    async fn test_rotate_order_enroll_then_revoke() {
        let dir = tempfile::tempdir().unwrap();
        let state_path = dir.path().join("luks-credentials.json");
        seed_state(&state_path, "old-serial", CredentialRole::Primary, 2);

        let mock = MockExecutor::new()
            .queue(&tang_adv_command(TANG_A), ADV)
            .queue(&tang_adv_command(TANG_B), ADV)
            .queue(&tang_adv_command(TANG_C), ADV);
        let mut mock = queue_rotation_dumps(mock);

        rotate_fido2(
            &mut mock,
            DEV,
            "old-serial",
            "new-serial",
            CredentialRole::Backup1,
            "test-passphrase",
            HOST,
            &TANG_URLS,
            None,
            &state_path,
        )
        .await
        .expect("rotation should succeed");

        // The recorded sequence must show the enroll (cryptenroll) command
        // strictly BEFORE any --wipe-slot.
        let enroll_cmd = build_enroll_command(DEV, "test-passphrase").unwrap();
        let enroll_idx = mock
            .recorded
            .iter()
            .position(|c| *c == enroll_cmd)
            .expect("enroll command recorded");
        let wipe_idx = mock
            .recorded
            .iter()
            .position(|c| c.contains("--wipe-slot"))
            .expect("wipe command recorded");
        assert!(
            enroll_idx < wipe_idx,
            "enroll (idx {enroll_idx}) must precede wipe (idx {wipe_idx})"
        );
        assert_eq!(count_wipes(&mock.recorded), 1);
    }

    #[tokio::test]
    async fn test_rotate_enroll_failure_keeps_old() {
        let dir = tempfile::tempdir().unwrap();
        let state_path = dir.path().join("luks-credentials.json");
        seed_state(&state_path, "old-serial", CredentialRole::Primary, 2);

        // Enroll leg fails: before and after dumps identical → no new token.
        let mut mock = MockExecutor::new()
            .queue(&tang_adv_command(TANG_A), ADV)
            .queue(&tang_adv_command(TANG_B), ADV)
            .queue(&tang_adv_command(TANG_C), ADV)
            .queue(&dump_command(DEV), DUMP_ONLY_SLOT2)
            .queue(&dump_command(DEV), DUMP_ONLY_SLOT2);
        let err = rotate_fido2(
            &mut mock,
            DEV,
            "old-serial",
            "new-serial",
            CredentialRole::Backup1,
            "test-passphrase",
            HOST,
            &TANG_URLS,
            None,
            &state_path,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, AutoInstallError::SystemError(_)));

        // ZERO wipes, old state record unchanged (still active), no new record.
        assert_eq!(count_wipes(&mock.recorded), 0);
        let records = read_records(&state_path).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].yubikey_serial, "old-serial");
        assert!(records[0].revoked_at.is_none());
    }

    #[tokio::test]
    async fn test_rotate_happy_path() {
        // Anti-over-suppression: full 3-of-3 quorum, healthy header, valid
        // passphrase → rotation completes through EVERY guard.
        let dir = tempfile::tempdir().unwrap();
        let state_path = dir.path().join("luks-credentials.json");
        seed_state(&state_path, "old-serial", CredentialRole::Primary, 2);

        let mock = MockExecutor::new()
            .queue(&tang_adv_command(TANG_A), ADV)
            .queue(&tang_adv_command(TANG_B), ADV)
            .queue(&tang_adv_command(TANG_C), ADV);
        let mut mock = queue_rotation_dumps(mock);

        let new_record = rotate_fido2(
            &mut mock,
            DEV,
            "old-serial",
            "new-serial",
            CredentialRole::Backup1,
            "test-passphrase",
            HOST,
            &TANG_URLS,
            None,
            &state_path,
        )
        .await
        .expect("healthy rotation must not be blocked by the guard stack");

        assert_eq!(new_record.yubikey_serial, "new-serial");
        assert_eq!(new_record.luks_keyslot, Some(5));
        assert!(new_record.revoked_at.is_none());

        // New record appended, old record's revoked_at set.
        let records = read_records(&state_path).unwrap();
        assert_eq!(records.len(), 2);
        let old = records.iter().find(|r| r.yubikey_serial == "old-serial").unwrap();
        let new = records.iter().find(|r| r.yubikey_serial == "new-serial").unwrap();
        assert!(old.revoked_at.is_some());
        assert!(new.revoked_at.is_none());
    }

    // ── TangRotation sweep-before-retire ─────────────────────────────────────

    #[tokio::test]
    async fn test_tang_rotation_gates_retire() {
        let fleet = vec![
            "len-serv-001".to_string(),
            "len-serv-002".to_string(),
            "len-serv-003".to_string(),
        ];
        let mut rot = TangRotation::new(&fleet).expect("non-empty fleet");

        // Each rebind_host re-probes quorum against DEFAULT_TANG_URLS, so queue
        // valid advs for .45/.46 (2-of-3) once PER rebind.
        let regen = clevis_regen_command(DEV, 3);
        for host in ["len-serv-001", "len-serv-002"] {
            let mut mock = MockExecutor::new()
                .queue(&tang_adv_command(DEFAULT_TANG_URLS[0]), ADV)
                .queue(&tang_adv_command(DEFAULT_TANG_URLS[1]), ADV);
            rot.rebind_host(&mut mock, host, DEV, 3).await.unwrap();
            assert!(mock.recorded.contains(&regen));
        }

        // After 2 of 3 rebinds, retire is still gated and names the 3rd host.
        let err = rot.retire_old_key().unwrap_err();
        assert!(matches!(err, AutoInstallError::SystemError(_)));
        assert!(err.to_string().contains("len-serv-003"));
        assert_eq!(rot.pending_hosts(), vec!["len-serv-003".to_string()]);

        // Rebind the last host → retire returns the plan string.
        let mut mock = MockExecutor::new()
            .queue(&tang_adv_command(DEFAULT_TANG_URLS[0]), ADV)
            .queue(&tang_adv_command(DEFAULT_TANG_URLS[1]), ADV);
        rot.rebind_host(&mut mock, "len-serv-003", DEV, 3).await.unwrap();
        let plan = rot.retire_old_key().expect("all hosts rebound → Ok(plan)");
        assert!(plan.contains("Safe to retire"));
        assert!(rot.pending_hosts().is_empty());
    }

    #[test]
    fn test_tang_rotation_empty_fleet_refuses() {
        let err = TangRotation::new(&[]).unwrap_err();
        assert!(matches!(err, AutoInstallError::ConfigError(_)));
    }

    #[tokio::test]
    async fn test_tang_rotation_rejects_unknown_host_and_bad_dev() {
        let fleet = vec!["len-serv-001".to_string()];
        let mut rot = TangRotation::new(&fleet).unwrap();

        // Unknown host → ConfigError, no command run.
        let mut mock = MockExecutor::new()
            .queue(&tang_adv_command(DEFAULT_TANG_URLS[0]), ADV)
            .queue(&tang_adv_command(DEFAULT_TANG_URLS[1]), ADV);
        let err = rot
            .rebind_host(&mut mock, "unknown-host", DEV, 3)
            .await
            .unwrap_err();
        assert!(matches!(err, AutoInstallError::ConfigError(_)));

        // Injection-y device path is rejected before anything runs.
        let mut mock = MockExecutor::new();
        let err = rot
            .rebind_host(&mut mock, "len-serv-001", "/dev/sda; rm -rf /", 3)
            .await
            .unwrap_err();
        assert!(matches!(err, AutoInstallError::ConfigError(_)));
        assert_eq!(mock.recorded.len(), 0);
    }
}
