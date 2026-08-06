// file: crates/uaa-core/src/network/ssh_installer/credentials.rs
// version: 1.0.0
// guid: 4c8f1a92-7e63-4d20-9b5a-1f0c6e83a7d4
// last-edited: 2026-07-27

//! Random operator-password generation + local reporting for the SSH/native
//! installer.
//!
//! ## Why
//!
//! A password written into the install YAML is a secret at rest that can leak,
//! and — worse — one literal password gets reused on every host installed from
//! that config, so a single leak is a fleet compromise. Instead the operator
//! writes the sentinel [`RANDOM_SENTINEL`] (`"!random"`) where a literal
//! password would go. The install *driver* (the `uaa` process, which for the
//! SSH path runs on the machine you launched the install from) replaces the
//! sentinel with a freshly generated, per-host random password, applies it in
//! the target via the installer's existing base64->`chpasswd` path, and records
//! the generated value in a `0600` file on the driver host plus stdout.
//!
//! Net effect: the cleartext secret never lives in the config, each host gets a
//! distinct password (one leak ≠ fleet compromise), and the only at-rest copy
//! is a root-only `0600` file on the box that ran the install — strictly better
//! than a reusable secret in a checked-in/placed YAML.
//!
//! ## Shape
//!
//! Generation and sentinel-resolution are **pure** (no I/O) so the
//! security-critical logic is unit-tested; only [`write_credentials_file`]
//! touches the filesystem, and it is a thin best-effort sink the driver calls
//! once after a successful install.

use crate::network::ssh_installer::config::InstallationConfig;

/// Config sentinel meaning "generate a random password here at install time".
///
/// Chosen to be unmistakable and never a plausible real password: a bare `!`
/// prefix is not something a human would type as an actual passphrase, and it
/// is distinct from the empty string (which already means "lock the account,
/// SSH-key-only login").
pub const RANDOM_SENTINEL: &str = "!random";

/// Length of generated passwords. 20 characters over the 56-symbol unambiguous
/// charset is ≈116 bits of entropy — strong, yet still hand-typeable at a
/// serial console for break-glass login.
const PASSWORD_LEN: usize = 20;

/// Unambiguous charset: `A-Z a-z 0-9` minus the classic look-alikes
/// (`0`/`O`, `1`/`l`/`I`) so the operator never fat-fingers a console password.
/// Character order is irrelevant to entropy.
const CHARSET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz23456789";

/// One generated credential, surfaced to the operator after install.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedCredential {
    /// Account name, e.g. `"root"` or `"jdfalk"`.
    pub account: String,
    /// The generated cleartext password (reported to the operator only).
    pub password: String,
}

/// Generate one random password from [`CHARSET`] using `ring`'s
/// `SystemRandom` (the same OS-backed CSPRNG this crate uses for LUKS key
/// material), the crate's one vetted randomness source.
///
/// Uses **rejection sampling** to map bytes onto the charset without modulo
/// bias: any byte ≥ the largest multiple of `CHARSET.len()` that fits in a
/// `u8` is discarded, so every accepted byte's residue is equiprobable.
pub fn generate_password() -> String {
    use ring::rand::{SecureRandom, SystemRandom};

    let rng = SystemRandom::new();
    let n = CHARSET.len() as u16; // 56
    let limit = (256u16 / n) * n; // largest multiple of n ≤ 256 → 224
    let mut out = String::with_capacity(PASSWORD_LEN);
    let mut buf = [0u8; 1];
    while out.len() < PASSWORD_LEN {
        rng.fill(&mut buf)
            .expect("SystemRandom::fill must not fail");
        let b = buf[0] as u16;
        if b < limit {
            out.push(CHARSET[(b % n) as usize] as char);
        }
        // else: reject and redraw — keeps the distribution uniform.
    }
    out
}

/// Replace every [`RANDOM_SENTINEL`] password in `config` (root, then each
/// operator user in order) with a freshly generated password, returning the
/// generated credentials in application order.
///
/// A config with no sentinels is left byte-for-byte untouched and yields an
/// empty vec, so this is a genuine no-op for literal-password configs — the
/// feature is strictly opt-in per field.
pub fn resolve_random_passwords(config: &mut InstallationConfig) -> Vec<GeneratedCredential> {
    let mut generated = Vec::new();

    if config.root_password == RANDOM_SENTINEL {
        let pw = generate_password();
        config.root_password = pw.clone();
        generated.push(GeneratedCredential {
            account: "root".to_string(),
            password: pw,
        });
    }

    for user in &mut config.users {
        if user.password == RANDOM_SENTINEL {
            let pw = generate_password();
            user.password = pw.clone();
            generated.push(GeneratedCredential {
                account: user.name.clone(),
                password: pw,
            });
        }
    }

    generated
}

/// Render the credential report used for BOTH the `0600` file and stdout.
///
/// Pure (timestamp is passed in, not read) so the exact output is unit-testable.
pub fn format_credentials_report(
    host_label: &str,
    address: &str,
    generated_at: &str,
    creds: &[GeneratedCredential],
) -> String {
    let mut out = String::new();
    out.push_str(&format!("host: {host_label} ({address})\n"));
    out.push_str(&format!("generated: {generated_at}\n"));
    out.push_str(
        "# random per-host passwords — keep this file 0600; rotate via `passwd` if leaked\n",
    );
    for c in creds {
        out.push_str(&format!("{}: {}\n", c.account, c.password));
    }
    out
}

/// Directory the driver writes per-host credential files into.
pub const CREDENTIALS_DIR: &str = "/var/lib/uaa/credentials";

/// Write `report` to `<CREDENTIALS_DIR>/<sanitized host_label>.txt` with `0700`
/// dir / `0600` file permissions, returning the path written.
///
/// The caller treats this as best-effort: a failure to persist must NOT fail an
/// otherwise-successful install (the passwords are still printed to stdout), so
/// the driver logs the error and moves on. Filename is sanitized so an odd
/// hostname can't escape the directory.
pub fn write_credentials_file(
    host_label: &str,
    report: &str,
) -> std::io::Result<std::path::PathBuf> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    let dir = std::path::Path::new(CREDENTIALS_DIR);
    std::fs::create_dir_all(dir)?;
    // Tighten the dir even if it pre-existed with looser perms.
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;

    let safe: String = host_label
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let safe = if safe.is_empty() {
        "host".to_string()
    } else {
        safe
    };
    let path = dir.join(format!("{safe}.txt"));

    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&path)?;
    f.write_all(report.as_bytes())?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::ssh_installer::config::UserAccount;

    fn base_user(name: &str, password: &str) -> UserAccount {
        UserAccount {
            name: name.to_string(),
            password: password.to_string(),
            groups: vec!["sudo".to_string()],
            shell: "/bin/bash".to_string(),
            ssh_authorized_keys: vec![],
        }
    }

    #[test]
    fn generated_password_is_expected_length_and_charset() {
        let pw = generate_password();
        assert_eq!(pw.len(), PASSWORD_LEN);
        // Every char is from the unambiguous charset — no 0/O/1/l/I.
        for c in pw.chars() {
            assert!(
                CHARSET.contains(&(c as u8)),
                "generated char {c:?} not in the unambiguous charset"
            );
        }
        assert!(!pw.contains('0'));
        assert!(!pw.contains('O'));
        assert!(!pw.contains('1'));
        assert!(!pw.contains('l'));
        assert!(!pw.contains('I'));
    }

    #[test]
    fn generated_passwords_are_distinct() {
        // Not a randomness proof, but catches a constant/fixed-seed regression.
        let a = generate_password();
        let b = generate_password();
        assert_ne!(a, b, "two generations returned the identical password");
    }

    #[test]
    fn resolve_replaces_root_and_user_sentinels() {
        let mut cfg = InstallationConfig::for_len_serv_003();
        cfg.root_password = RANDOM_SENTINEL.to_string();
        cfg.users = vec![
            base_user("jdfalk", RANDOM_SENTINEL),
            base_user("svc", ""), // empty stays empty (locked), not generated
            base_user("ops", "literalpw"),
        ];

        let generated = resolve_random_passwords(&mut cfg);

        // root + jdfalk generated, in that order; svc/ops untouched.
        assert_eq!(generated.len(), 2);
        assert_eq!(generated[0].account, "root");
        assert_eq!(generated[1].account, "jdfalk");

        // Sentinels are gone from the config; real passwords are in place.
        assert_ne!(cfg.root_password, RANDOM_SENTINEL);
        assert_eq!(cfg.root_password.len(), PASSWORD_LEN);
        assert_eq!(cfg.users[0].password, generated[1].password);
        assert_eq!(cfg.users[0].password.len(), PASSWORD_LEN);

        // Non-sentinel fields are left exactly as-is.
        assert_eq!(cfg.users[1].password, "");
        assert_eq!(cfg.users[2].password, "literalpw");
    }

    #[test]
    fn resolve_is_noop_without_sentinels() {
        let mut cfg = InstallationConfig::for_len_serv_003();
        let before = cfg.root_password.clone();
        cfg.users = vec![base_user("ops", "literalpw")];

        let generated = resolve_random_passwords(&mut cfg);

        assert!(generated.is_empty());
        assert_eq!(cfg.root_password, before);
        assert_eq!(cfg.users[0].password, "literalpw");
    }

    #[test]
    fn report_lists_every_credential() {
        let creds = vec![
            GeneratedCredential {
                account: "root".to_string(),
                password: "Kf9mQ2pLxR7nT4wZbcde".to_string(),
            },
            GeneratedCredential {
                account: "jdfalk".to_string(),
                password: "Bd3vN8sHy6cJ2qMwfghi".to_string(),
            },
        ];
        let report = format_credentials_report("u1", "172.16.2.35", "2026-07-27T12:00:00Z", &creds);
        assert!(report.contains("host: u1 (172.16.2.35)"));
        assert!(report.contains("generated: 2026-07-27T12:00:00Z"));
        assert!(report.contains("root: Kf9mQ2pLxR7nT4wZbcde"));
        assert!(report.contains("jdfalk: Bd3vN8sHy6cJ2qMwfghi"));
    }
}
