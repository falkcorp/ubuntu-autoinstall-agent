// file: crates/uaa-core/src/config_place.rs
// version: 1.1.0
// guid: 0f2da210-310d-48f5-8c58-1b95bd3c6d45
// last-edited: 2026-07-10

//! Config placement — server-local port of `scripts/deploy-usb-configs.sh` (v1.1.0).
//!
//! Places per-host [`InstallationConfig`] files where the autoinstall-agent's
//! MAC-resolved endpoint serves them:
//!
//! ```text
//! <src-dir>/<host>.yaml  ->  <dest-base>/<hexmac>/uaa.yaml   (mode 0644)
//! ```
//!
//! With `--inject-from`, per-host `REPLACE_AT_PLACE_TIME` secret slots are filled
//! from a secrets file into a 0600 staging copy BEFORE placement. Every guard from
//! the shell script is load-bearing and ported here:
//!
//! - secrets file must be mode 0600-or-stricter and NOT inside any git work tree;
//! - secret VALUES never touch argv, logs, error messages, or panic text
//!   (in-memory awk-style injection; refusal reasons never carry a value);
//! - staging copies are 0600 [`NamedTempFile`]s, dropped (cleaned up) on every exit;
//! - `REPLACE_AT_PLACE_TIME` hard gate on the STAGED copy — a secretless config must
//!   never be servable to a booting installer;
//! - server-local only: a remote-looking `--dest` is refused (no HTTP secret-write API).
//!
//! This module is pure `std::fs` + in-memory string work: no external command
//! execution, no network client, no ssh/scp path. The ONE process spawn is
//! `git rev-parse` for the work-tree guard (its argv contains only a directory
//! path, never secret material).

use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::NamedTempFile;

use crate::error::AutoInstallError;
use crate::network::InstallationConfig;
use crate::Result;

/// Secret placeholder literal. A config still containing this after (optional)
/// injection is refused by the hard gate.
pub const PLACEHOLDER: &str = "REPLACE_AT_PLACE_TIME";

/// Known fleet hosts (default placement set when none are named).
pub const KNOWN_HOSTS: [&str; 4] = [
    "len-serv-001",
    "len-serv-002",
    "len-serv-003",
    "unimatrixone",
];

/// Default source directory of `<host>.yaml` files (repo-relative).
pub const DEFAULT_SRC_DIR: &str = "examples/configs/install";

/// Default cloud-init web root on the server.
pub const DEFAULT_DEST_BASE: &str = "/var/www/html/cloud-init";

/// Host → MAC registry (the known fleet MACs). Unknown host → `None`.
pub fn mac_for_host(host: &str) -> Option<&'static str> {
    match host {
        "len-serv-001" => Some("6c:4b:90:bc:39:b3"),
        "len-serv-002" => Some("6c:4b:90:bc:f8:a3"),
        "len-serv-003" => Some("6c:4b:90:bc:f7:f4"),
        "unimatrixone" => Some("ac:1f:6b:40:fc:e2"),
        _ => None,
    }
}

/// MAC with colons stripped (`6c:4b:90:bc:39:b3` → `6c4b90bc39b3`).
pub fn hexmac(mac: &str) -> String {
    mac.replace(':', "")
}

/// Parsed `--inject-from` secrets file.
///
/// Format: top-level unindented `host:` section headers; indented `key: value`
/// lines beneath; the value is everything after `key: ` copied VERBATIM (quotes
/// included). Its [`fmt::Debug`] redacts all values — no secret ever prints.
pub struct SecretsFile {
    sections: HashMap<String, HashMap<String, String>>,
}

impl fmt::Debug for SecretsFile {
    // Manual Debug: print host + key NAMES, but NEVER a value. Values are secrets.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut dbg = f.debug_struct("SecretsFile");
        for (host, keys) in &self.sections {
            let redacted: Vec<String> =
                keys.keys().map(|k| format!("{k}: <redacted>")).collect();
            dbg.field(host, &redacted);
        }
        dbg.finish()
    }
}

impl SecretsFile {
    /// Parse the section/key/verbatim-value format (mirrors the awk registry pass).
    pub fn parse(text: &str) -> SecretsFile {
        let mut sections: HashMap<String, HashMap<String, String>> = HashMap::new();
        let mut current: Option<String> = None;
        for line in text.split('\n') {
            if let Some(name) = parse_section_header(line) {
                current = Some(name.to_string());
                sections.entry(name.to_string()).or_default();
                continue;
            }
            if let Some(section) = &current {
                if let Some((key, val)) = parse_indented_kv(line) {
                    sections
                        .entry(section.clone())
                        .or_default()
                        .insert(key, val);
                }
            }
        }
        SecretsFile { sections }
    }

    /// The key→value map for `host`, if present.
    fn section(&self, host: &str) -> Option<&HashMap<String, String>> {
        self.sections.get(host)
    }
}

/// True for an unindented `name:` section header with nothing after the colon
/// but whitespace (awk `^[A-Za-z0-9_-]+:[[:space:]]*$`).
fn parse_section_header(line: &str) -> Option<&str> {
    let name_end = line
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
        .unwrap_or(line.len());
    if name_end == 0 {
        return None; // leading whitespace or empty → not a header
    }
    let name = &line[..name_end];
    let rest = &line[name_end..];
    let after_colon = rest.strip_prefix(':')?;
    if after_colon.chars().all(|c| c == ' ' || c == '\t' || c == '\r') {
        Some(name)
    } else {
        None
    }
}

/// Parse an indented `  key: value` line (awk
/// `^[[:space:]]+[A-Za-z0-9_]+:[[:space:]]*[^[:space:]]`). The value is VERBATIM
/// after `key: ` (quotes and all). Returns `None` unless it has leading
/// whitespace, a valid key, a colon, and a non-empty value.
fn parse_indented_kv(line: &str) -> Option<(String, String)> {
    let after_ws = line.trim_start_matches([' ', '\t']);
    if after_ws.len() == line.len() {
        return None; // no leading whitespace → not an indented key line
    }
    let key_end = after_ws
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(after_ws.len());
    if key_end == 0 {
        return None;
    }
    let key = &after_ws[..key_end];
    let rest = &after_ws[key_end..];
    let after_colon = rest.strip_prefix(':')?;
    let val = after_colon.trim_start_matches([' ', '\t']);
    if val.is_empty() {
        return None; // awk requires a non-space value char
    }
    Some((key.to_string(), val.to_string()))
}

/// Run the three secrets-file guards IN ORDER (exists → git-work-tree → mode).
/// Fails the whole run; never prints or embeds a secret value.
pub fn check_secrets_file(path: &Path) -> Result<()> {
    // 1. Must exist (as a regular file).
    if !path.is_file() {
        return Err(AutoInstallError::ConfigError(format!(
            "--inject-from file not found: {}",
            path.display()
        )));
    }

    // 2. Refuse a secrets file living inside ANY git work tree. The git probe is
    //    the one allowed spawn: argv is only the directory path, never a secret.
    //    If git itself is unavailable (server-side use), the guard PASSES.
    let dir = path.parent().filter(|p| !p.as_os_str().is_empty());
    let probe_dir: PathBuf = match dir {
        Some(d) => d.to_path_buf(),
        None => PathBuf::from("."),
    };
    if let Ok(output) = Command::new("git")
        .arg("-C")
        .arg(&probe_dir)
        .args(["rev-parse", "--is-inside-work-tree"])
        .output()
    {
        if String::from_utf8_lossy(&output.stdout).trim() == "true" {
            return Err(AutoInstallError::ConfigError(format!(
                "--inject-from file is inside a git work tree: {}",
                path.display()
            )));
        }
    }

    // 3. Group/other must have NO permission bits (mode 0600 or stricter).
    let mode = fs::metadata(path)?.permissions().mode();
    if mode & 0o077 != 0 {
        return Err(AutoInstallError::ConfigError(format!(
            "--inject-from file is group/world accessible (need mode 0600 or stricter): {}",
            path.display()
        )));
    }

    Ok(())
}

/// Fill `REPLACE_AT_PLACE_TIME` slots in `config_text` from `host`'s secrets
/// section (pure port of the awk injection pass). No IO. Values never logged.
///
/// - a COMMENT line (`^\s*#`) containing the token is DROPPED (the committed
///   examples carry one that would otherwise trip the hard gate on an injected copy);
/// - a `key: REPLACE_AT_PLACE_TIME` line whose key exists in the host's section is
///   rewritten `<original indent><key>: <verbatim value>`;
/// - a placeholder line with NO matching secret passes through unchanged (the hard
///   gate then refuses that host).
pub fn inject_secrets(secrets: &SecretsFile, host: &str, config_text: &str) -> String {
    let section = secrets.section(host);
    let mut out: Vec<String> = Vec::new();
    for line in config_text.split('\n') {
        if !line.contains(PLACEHOLDER) {
            out.push(line.to_string());
            continue;
        }
        // Comment line documenting the placeholder scheme → drop entirely.
        let trimmed = line.trim_start_matches([' ', '\t']);
        if trimmed.starts_with('#') {
            continue;
        }
        // line_key = first whitespace-delimited token, trailing ':' removed.
        let token = line.split_whitespace().next().unwrap_or("");
        let line_key = token.strip_suffix(':').unwrap_or(token);
        if let Some(val) = section.and_then(|s| s.get(line_key)) {
            let indent: String = line
                .chars()
                .take_while(|c| *c == ' ' || *c == '\t')
                .collect();
            out.push(format!("{indent}{line_key}: {val}"));
            continue;
        }
        // Placeholder with no matching secret → unchanged (hard gate refuses later).
        out.push(line.to_string());
    }
    out.join("\n")
}

/// True for a remote-looking destination — scp `host:path` syntax, `ssh://…`, or
/// anything containing `://`. Placement is server-local only.
fn dest_is_remote(dest: &str) -> bool {
    if dest.contains("://") {
        return true;
    }
    // scp `host:path`: chars before the first ':' contain no '/'.
    if let Some(idx) = dest.find(':') {
        if idx > 0 && !dest[..idx].contains('/') {
            return true;
        }
    }
    false
}

/// Options for [`place_configs`].
#[derive(Debug, Clone)]
pub struct PlaceOptions {
    /// Directory of `<host>.yaml` source files.
    pub src_dir: PathBuf,
    /// Cloud-init web root; files land at `<dest_base>/<hexmac>/uaa.yaml`.
    pub dest_base: PathBuf,
    /// Optional per-host secrets file for place-time injection.
    pub inject_from: Option<PathBuf>,
    /// Hosts to place; empty = all [`KNOWN_HOSTS`].
    pub hosts: Vec<String>,
}

/// Outcome of a placement run. Overall `Ok` even with refusals; the CLI maps a
/// non-empty `refused` to exit 1.
#[derive(Debug, Default)]
pub struct PlaceReport {
    /// One `"<host> (<mac>) -> <path>"` line per placed config.
    pub placed: Vec<String>,
    /// `(host, reason)` per refused host. A reason NEVER contains a secret value.
    pub refused: Vec<(String, String)>,
}

impl PlaceReport {
    /// Exit-status view: success iff nothing was refused.
    pub fn is_success(&self) -> bool {
        self.refused.is_empty()
    }
}

/// Server-local placement driver. Ports the shell script's whole-run secrets
/// guards + per-host placement loop.
pub fn place_configs(opts: &PlaceOptions) -> Result<PlaceReport> {
    // Remote-dest refusal FIRST — before any host or secrets file is touched.
    let dest_str = opts.dest_base.to_string_lossy();
    if dest_is_remote(&dest_str) {
        return Err(AutoInstallError::ConfigError(
            "place-time injection is server-local only; there is NO HTTP secret-write API, by design"
                .to_string(),
        ));
    }

    // Whole-run secrets-file guards (abort with exit 1 BEFORE any host).
    let secrets: Option<SecretsFile> = match &opts.inject_from {
        Some(path) => {
            check_secrets_file(path)?;
            let text = fs::read_to_string(path)?;
            Some(SecretsFile::parse(&text))
        }
        None => None,
    };

    let hosts: Vec<String> = if opts.hosts.is_empty() {
        KNOWN_HOSTS.iter().map(|s| s.to_string()).collect()
    } else {
        opts.hosts.clone()
    };

    let mut report = PlaceReport::default();
    for host in &hosts {
        // Unknown host → REFUSED (per-host, not a global abort).
        let mac = match mac_for_host(host) {
            Some(m) => m,
            None => {
                report.refused.push((
                    host.clone(),
                    "unknown host (add its MAC to mac_for_host)".to_string(),
                ));
                continue;
            }
        };

        let src = opts.src_dir.join(format!("{host}.yaml"));
        if !src.is_file() {
            report
                .refused
                .push((host.clone(), format!("source not found: {}", src.display())));
            continue;
        }
        let config_text = fs::read_to_string(&src)?;

        // Stage into a 0600 NamedTempFile when injecting. `_staged` keeps the temp
        // file alive (and 0600) through placement, then drops (cleans up) on exit.
        let (final_text, _staged) = match &secrets {
            Some(s) => {
                let injected = inject_secrets(s, host, &config_text);
                let mut tmp = NamedTempFile::new()?; // Unix: created 0600
                tmp.write_all(injected.as_bytes())?;
                tmp.flush()?;
                (injected, Some(tmp))
            }
            None => (config_text, None),
        };

        // HARD GATE on the staged copy: never place a config whose secrets were
        // not injected. The reason carries the token + src path, never a value.
        if final_text.contains(PLACEHOLDER) {
            report.refused.push((
                host.clone(),
                format!(
                    "{} still contains {PLACEHOLDER} — inject real secrets into a staging copy",
                    src.display()
                ),
            ));
            continue;
        }

        // Structural gate: the fully-injected staged copy must parse as
        // InstallationConfig (deny_unknown_fields). The serde error is DELIBERATELY
        // NOT embedded in the reason — the staged text carries real secrets and a
        // serde message can echo offending content.
        if serde_yaml::from_str::<InstallationConfig>(&final_text).is_err() {
            report.refused.push((
                host.clone(),
                "staged config is not a valid InstallationConfig (deny_unknown_fields)".to_string(),
            ));
            continue;
        }

        // Place: mkdir -p <dest>/<hexmac>, write uaa.yaml, force mode 0644.
        let dest_dir = opts.dest_base.join(hexmac(mac));
        fs::create_dir_all(&dest_dir)?;
        let dest_file = dest_dir.join("uaa.yaml");
        fs::write(&dest_file, final_text.as_bytes())?;
        fs::set_permissions(&dest_file, fs::Permissions::from_mode(0o644))?;

        report
            .placed
            .push(format!("{host} ({mac}) -> {}", dest_file.display()));
    }

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    /// A full, valid `InstallationConfig` YAML with the given secret lines still
    /// as placeholders and a placeholder-bearing comment (which injection drops).
    fn placeholder_config(host: &str) -> String {
        format!(
            "hostname: {host}\n\
             disk_device: /dev/nvme0n1\n\
             timezone: America/New_York\n\
             # inject: replace every REPLACE_AT_PLACE_TIME token before serving\n\
             luks_key: REPLACE_AT_PLACE_TIME\n\
             root_password: REPLACE_AT_PLACE_TIME\n\
             network_interface: enp1s0f0\n\
             network_address: 172.16.3.96/23\n\
             network_gateway: 172.16.2.1\n\
             network_search: jf.local\n\
             network_nameservers:\n  - 172.16.2.1\n\
             enroll_tpm2: true\n\
             tpm2_pin: REPLACE_AT_PLACE_TIME\n"
        )
    }

    /// A full, valid `InstallationConfig` YAML with real (fake) values, no placeholders.
    fn valid_config(host: &str) -> String {
        format!(
            "hostname: {host}\n\
             disk_device: /dev/nvme0n1\n\
             timezone: America/New_York\n\
             luks_key: already-set\n\
             root_password: already-set\n\
             network_interface: enp1s0f0\n\
             network_address: 172.16.3.94/23\n\
             network_gateway: 172.16.2.1\n\
             network_search: jf.local\n\
             network_nameservers:\n  - 172.16.2.1\n"
        )
    }

    fn write_secrets(dir: &Path, body: &str) -> PathBuf {
        let p = dir.join("uaa-secrets.yaml");
        fs::write(&p, body).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o600)).unwrap();
        p
    }

    #[test]
    fn test_mac_registry_and_hexmac() {
        assert_eq!(mac_for_host("len-serv-001"), Some("6c:4b:90:bc:39:b3"));
        assert_eq!(mac_for_host("len-serv-002"), Some("6c:4b:90:bc:f8:a3"));
        assert_eq!(mac_for_host("len-serv-003"), Some("6c:4b:90:bc:f7:f4"));
        assert_eq!(mac_for_host("unimatrixone"), Some("ac:1f:6b:40:fc:e2"));
        assert_eq!(mac_for_host("nope"), None);
        assert_eq!(hexmac("6c:4b:90:bc:39:b3"), "6c4b90bc39b3");
    }

    #[test]
    fn test_secrets_perms_guard() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.yaml");
        fs::write(&p, "len-serv-001:\n  luks_key: x\n").unwrap();

        fs::set_permissions(&p, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(check_secrets_file(&p).is_err(), "0644 must be refused");

        fs::set_permissions(&p, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(check_secrets_file(&p).is_ok(), "0600 must pass");

        fs::set_permissions(&p, fs::Permissions::from_mode(0o400)).unwrap();
        assert!(check_secrets_file(&p).is_ok(), "0400 must pass");
    }

    #[test]
    fn test_secrets_git_tree_refusal() {
        let dir = tempfile::tempdir().unwrap();
        // Make the dir a git work tree.
        let ok = Command::new("git")
            .arg("-C")
            .arg(dir.path())
            .arg("init")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        assert!(ok, "git must be available for this test");

        let p = write_secrets(dir.path(), "len-serv-001:\n  luks_key: x\n");
        let err = check_secrets_file(&p).unwrap_err();
        assert!(
            err.to_string().contains("git work tree"),
            "expected git work tree refusal, got: {err}"
        );
    }

    #[test]
    fn test_inject_verbatim_and_comment_drop() {
        let secrets = SecretsFile::parse("myhost:\n  luks_key: \"the value\"\n");
        // Explicit newlines (no backslash-continuation, which would eat indent).
        let config = "  luks_key: REPLACE_AT_PLACE_TIME\n# note REPLACE_AT_PLACE_TIME here\n  other_key: REPLACE_AT_PLACE_TIME\n";
        let out = inject_secrets(&secrets, "myhost", config);

        // verbatim value with quotes, 2-space indent preserved
        assert!(out.contains("  luks_key: \"the value\""), "got: {out}");
        // comment line dropped
        assert!(!out.contains("# note"), "comment not dropped: {out}");
        // unmatched placeholder survives unchanged
        assert!(out.contains("  other_key: REPLACE_AT_PLACE_TIME"), "got: {out}");
    }

    #[test]
    fn test_place_refuses_leftover_placeholder() {
        let src = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();
        fs::write(
            src.path().join("len-serv-001.yaml"),
            placeholder_config("len-serv-001"),
        )
        .unwrap();

        let opts = PlaceOptions {
            src_dir: src.path().to_path_buf(),
            dest_base: dest.path().to_path_buf(),
            inject_from: None,
            hosts: vec!["len-serv-001".to_string()],
        };
        let report = place_configs(&opts).unwrap();

        assert!(report.placed.is_empty());
        assert_eq!(report.refused.len(), 1);
        assert_eq!(report.refused[0].0, "len-serv-001");
        // No file written under dest.
        assert!(!dest.path().join("6c4b90bc39b3").join("uaa.yaml").exists());
    }

    #[test]
    fn test_place_refuses_unknown_host_and_missing_src() {
        let src = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();
        // len-serv-001 has a valid, ready-to-serve config → should still place.
        fs::write(
            src.path().join("len-serv-001.yaml"),
            valid_config("len-serv-001"),
        )
        .unwrap();
        // len-serv-002 source intentionally missing.

        let opts = PlaceOptions {
            src_dir: src.path().to_path_buf(),
            dest_base: dest.path().to_path_buf(),
            inject_from: None,
            hosts: vec![
                "nosuchhost".to_string(),
                "len-serv-002".to_string(),
                "len-serv-001".to_string(),
            ],
        };
        let report = place_configs(&opts).unwrap();

        assert_eq!(report.refused.len(), 2, "unknown + missing src");
        let refused_hosts: Vec<&str> = report.refused.iter().map(|(h, _)| h.as_str()).collect();
        assert!(refused_hosts.contains(&"nosuchhost"));
        assert!(refused_hosts.contains(&"len-serv-002"));
        // Other host still places.
        assert_eq!(report.placed.len(), 1);
        assert!(dest.path().join("6c4b90bc39b3").join("uaa.yaml").exists());
    }

    #[test]
    fn test_place_refuses_remote_dest() {
        let src = tempfile::tempdir().unwrap();
        for remote in ["172.16.2.30:/var/www", "ssh://x/y"] {
            let opts = PlaceOptions {
                src_dir: src.path().to_path_buf(),
                dest_base: PathBuf::from(remote),
                inject_from: None,
                hosts: vec!["len-serv-001".to_string()],
            };
            let err = place_configs(&opts).unwrap_err();
            assert!(
                matches!(err, AutoInstallError::ConfigError(_)),
                "expected ConfigError for {remote}, got {err:?}"
            );
            assert!(err.to_string().contains("server-local only"));
        }
    }

    #[test]
    fn test_refusal_reasons_never_leak_values() {
        let src = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();
        let secrets_dir = tempfile::tempdir().unwrap();
        // luks_key injects, root_password/tpm2_pin left un-injected → hard gate.
        let secrets_path = write_secrets(
            secrets_dir.path(),
            "len-serv-001:\n  luks_key: sekrit-123\n",
        );
        fs::write(
            src.path().join("len-serv-001.yaml"),
            placeholder_config("len-serv-001"),
        )
        .unwrap();

        let opts = PlaceOptions {
            src_dir: src.path().to_path_buf(),
            dest_base: dest.path().to_path_buf(),
            inject_from: Some(secrets_path.clone()),
            hosts: vec!["len-serv-001".to_string()],
        };
        let report = place_configs(&opts).unwrap();

        assert_eq!(report.refused.len(), 1, "leftover placeholder → refused");
        for (_, reason) in &report.refused {
            assert!(!reason.contains("sekrit-123"), "reason leaked value: {reason}");
        }
        // Debug never prints the value.
        let secrets = SecretsFile::parse(&fs::read_to_string(&secrets_path).unwrap());
        let dbg = format!("{secrets:?}");
        assert!(!dbg.contains("sekrit-123"), "Debug leaked value: {dbg}");
        assert!(dbg.contains("<redacted>"), "Debug missing redaction: {dbg}");
        assert!(report.placed.is_empty());
    }

    #[test]
    fn test_place_happy_path_end_to_end() {
        let src = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();
        let secrets_dir = tempfile::tempdir().unwrap();
        let secrets_path = write_secrets(
            secrets_dir.path(),
            "len-serv-001:\n  \
             luks_key: \"test-passphrase\"\n  \
             root_password: test-root-pass\n  \
             tpm2_pin: \"12345678\"\n",
        );
        fs::write(
            src.path().join("len-serv-001.yaml"),
            placeholder_config("len-serv-001"),
        )
        .unwrap();

        let opts = PlaceOptions {
            src_dir: src.path().to_path_buf(),
            dest_base: dest.path().to_path_buf(),
            inject_from: Some(secrets_path),
            hosts: vec!["len-serv-001".to_string()],
        };
        let report = place_configs(&opts).unwrap();

        assert!(report.is_success(), "refused: {:?}", report.refused);
        assert_eq!(report.placed.len(), 1);

        let placed = dest.path().join("6c4b90bc39b3").join("uaa.yaml");
        assert!(placed.exists(), "file not placed");
        // Mode 0644.
        let mode = fs::metadata(&placed).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o644, "wrong mode: {:o}", mode);
        let content = fs::read_to_string(&placed).unwrap();
        assert!(content.contains("test-passphrase"), "injected value missing");
        assert!(!content.contains(PLACEHOLDER), "placeholder leftover");
        // Parses as InstallationConfig.
        serde_yaml::from_str::<InstallationConfig>(&content).expect("must parse");
    }
}
