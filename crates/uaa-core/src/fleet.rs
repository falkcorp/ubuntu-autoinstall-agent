// file: crates/uaa-core/src/fleet.rs
// version: 1.1.0
// guid: 0a316b51-1c90-4227-b419-06feed01e2d6
// last-edited: 2026-07-10

//! Fleet inventory/model (spec C1): the netboot server, Tang URLs, LUKS
//! partition, NIC name, power server, power host registry, and the
//! `unimatrixone` reinstall deny-list all move behind [`FleetConfig`], loaded
//! from `/etc/uaa/fleet.yaml` with the CURRENT hardcoded values as defaults.
//!
//! # Edge semantics (the safety property)
//!
//! - `/etc/uaa/fleet.yaml` ABSENT → silent defaults (logged at debug). This is
//!   every dev/test machine today.
//! - File PRESENT but unreadable/unparseable/unknown-field → **hard error,
//!   fail-closed**. A typo'd fleet.yaml silently falling back to defaults
//!   would point installs at the wrong server, so [`FleetConfig`] rejects
//!   unknown fields at parse time (same pattern as `InstallationConfig` in
//!   `network::ssh_installer::config`).
//! - Partial file → absent fields take their defaults
//!   (`#[serde(default = ...)]` per field).
//!
//! Every default is sourced from the pre-existing `pub const` items in
//! `autoinstall::place`, `autoinstall::verify`, and `power` — those consts
//! remain the single source of truth; the literals are never retyped here.
//!
//! # Test isolation
//!
//! Tests must NEVER read the real `/etc/uaa/fleet.yaml`. Unit tests in this
//! module call [`FleetConfig::default`] / [`load_from`] directly against
//! tempdir paths — never [`fleet`], the process-wide cached accessor.
//! [`fleet`] (and therefore any production code that calls it, e.g.
//! `power::lookup_host`) honors the `UAA_FLEET_CONFIG` env override so CI/dev
//! machines without a real fleet.yaml still get deterministic defaults.

use std::path::Path;
use std::sync::OnceLock;

use crate::autoinstall::{place, verify};
use crate::error::AutoInstallError;
use crate::power;

// ── Config model ────────────────────────────────────────────────────────────

/// One entry in the power-control host registry (today's `lookup_host` table).
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
pub struct PowerHostEntry {
    pub hostname: String,
    /// `"ipmi"` | `"amd-dash"` | `"intel-amt"` | `"wol"`. Unknown strings are
    /// accepted here (the file still parses) but resolve to `None` +
    /// a `tracing::warn!` at lookup time — see `power::lookup_host`.
    pub mechanism: String,
    #[serde(default)]
    pub bmc_host: Option<String>,
    #[serde(default)]
    pub username: Option<String>,
}

/// Fleet-wide configuration, loaded from `/etc/uaa/fleet.yaml` with every
/// field defaulting to today's hardcoded value.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FleetConfig {
    #[serde(default = "d_netboot_server")]
    pub netboot_server: String,
    #[serde(default = "d_flip_api_port")]
    pub flip_api_port: u16,
    #[serde(default = "d_cloud_init_base")]
    pub cloud_init_base: String,
    #[serde(default = "d_server_user")]
    pub server_user: String,
    #[serde(default = "d_tang_urls")]
    pub tang_urls: Vec<String>,
    #[serde(default = "d_luks_partition")]
    pub luks_partition: String,
    #[serde(default = "d_lenserv_nic")]
    pub lenserv_nic: String,
    #[serde(default = "d_power_server")]
    pub power_server: String,
    /// Hostnames refused by one-click reinstall (spec C3 / CT-06).
    #[serde(default = "d_reinstall_deny")]
    pub reinstall_deny: Vec<String>,
    /// Power-control host registry — today's `power::lookup_host` table.
    #[serde(default = "d_power_hosts")]
    pub power_hosts: Vec<PowerHostEntry>,
}

impl Default for FleetConfig {
    fn default() -> Self {
        Self {
            netboot_server: d_netboot_server(),
            flip_api_port: d_flip_api_port(),
            cloud_init_base: d_cloud_init_base(),
            server_user: d_server_user(),
            tang_urls: d_tang_urls(),
            luks_partition: d_luks_partition(),
            lenserv_nic: d_lenserv_nic(),
            power_server: d_power_server(),
            reinstall_deny: d_reinstall_deny(),
            power_hosts: d_power_hosts(),
        }
    }
}

// ── Default-value functions ─────────────────────────────────────────────────
//
// Each of these returns the EXISTING pub const from its owning module — the
// const stays the single source of truth, never retyped here.

fn d_netboot_server() -> String {
    place::DEFAULT_NETBOOT_SERVER.to_string()
}

fn d_flip_api_port() -> u16 {
    place::FLIP_API_PORT
}

fn d_cloud_init_base() -> String {
    place::CLOUD_INIT_BASE.to_string()
}

fn d_server_user() -> String {
    place::DEFAULT_SERVER_USER.to_string()
}

fn d_tang_urls() -> Vec<String> {
    verify::TANG_URLS.iter().map(|s| s.to_string()).collect()
}

fn d_luks_partition() -> String {
    verify::LUKS_PARTITION.to_string()
}

fn d_lenserv_nic() -> String {
    verify::LENSERV_NIC.to_string()
}

fn d_power_server() -> String {
    power::POWER_SERVER.to_string()
}

fn d_reinstall_deny() -> Vec<String> {
    vec!["unimatrixone".to_string()]
}

/// Today's `power::lookup_host` registry, transcribed verbatim: this IS the
/// canonical source now — `power::lookup_host` becomes a thin wrapper reading
/// this list back out through [`fleet`].
fn d_power_hosts() -> Vec<PowerHostEntry> {
    vec![
        PowerHostEntry {
            hostname: "unimatrixone".to_string(),
            mechanism: "ipmi".to_string(),
            bmc_host: Some("172.16.3.150".to_string()),
            username: Some("ADMIN".to_string()),
        },
        PowerHostEntry {
            hostname: "len-serv-001".to_string(),
            mechanism: "amd-dash".to_string(),
            bmc_host: None,
            username: None,
        },
        PowerHostEntry {
            hostname: "len-serv-002".to_string(),
            mechanism: "amd-dash".to_string(),
            bmc_host: None,
            username: None,
        },
        PowerHostEntry {
            hostname: "len-serv-003".to_string(),
            mechanism: "amd-dash".to_string(),
            bmc_host: None,
            username: None,
        },
    ]
}

// ── Loaders ──────────────────────────────────────────────────────────────────

/// Load [`FleetConfig`] from `path`.
///
/// - Path does not exist → `Ok(FleetConfig::default())` (logged at debug).
/// - Path exists but can't be read, parsed, or has an unknown field →
///   `Err(AutoInstallError::ConfigError)` naming the path and the underlying
///   error (fail-closed).
pub fn load_from(path: &Path) -> crate::error::Result<FleetConfig> {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            tracing::debug!(
                "fleet config {} not found; using built-in defaults",
                path.display()
            );
            return Ok(FleetConfig::default());
        }
        Err(e) => {
            return Err(AutoInstallError::ConfigError(format!(
                "failed to read fleet config {}: {e}",
                path.display()
            )));
        }
    };

    serde_yaml::from_str(&content).map_err(|e| {
        AutoInstallError::ConfigError(format!(
            "failed to parse fleet config {}: {e}",
            path.display()
        ))
    })
}

/// Load from `$UAA_FLEET_CONFIG` if set, else `/etc/uaa/fleet.yaml`.
pub fn load_or_default() -> crate::error::Result<FleetConfig> {
    let path = std::env::var_os("UAA_FLEET_CONFIG")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("/etc/uaa/fleet.yaml"));
    load_from(&path)
}

/// Process-wide cached [`FleetConfig`] accessor.
///
/// First call runs [`load_or_default`]. An invalid file at that point PANICS
/// with the `ConfigError` text — silent defaulting on a broken config file is
/// exactly the failure mode this module closes, so this is fail-closed by
/// design, not an oversight.
pub fn fleet() -> &'static FleetConfig {
    static FLEET: OnceLock<FleetConfig> = OnceLock::new();
    FLEET.get_or_init(|| match load_or_default() {
        Ok(cfg) => cfg,
        Err(e) => panic!("{e}"),
    })
}

// ── Unit tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_defaults_match_legacy_constants() {
        let cfg = FleetConfig::default();
        assert_eq!(cfg.netboot_server, "172.16.2.30");
        assert_eq!(cfg.flip_api_port, 25000);
        assert_eq!(cfg.cloud_init_base, "/var/www/html/cloud-init");
        assert_eq!(cfg.server_user, "jdfalk");
        assert_eq!(
            cfg.tang_urls,
            vec![
                "http://172.16.2.45".to_string(),
                "http://172.16.2.46".to_string(),
                "http://172.16.2.47".to_string(),
            ]
        );
        assert_eq!(cfg.luks_partition, "/dev/nvme0n1p4");
        assert_eq!(cfg.lenserv_nic, "enp1s0f0");
        assert_eq!(cfg.power_server, "172.16.2.30");
        assert_eq!(cfg.reinstall_deny, vec!["unimatrixone".to_string()]);

        let unimatrixone = cfg
            .power_hosts
            .iter()
            .find(|e| e.hostname == "unimatrixone")
            .expect("unimatrixone must be in the default registry");
        assert_eq!(unimatrixone.mechanism, "ipmi");
        assert_eq!(unimatrixone.bmc_host.as_deref(), Some("172.16.3.150"));
        assert_eq!(unimatrixone.username.as_deref(), Some("ADMIN"));

        for host in ["len-serv-001", "len-serv-002", "len-serv-003"] {
            let entry = cfg
                .power_hosts
                .iter()
                .find(|e| e.hostname == host)
                .unwrap_or_else(|| panic!("{host} must be in the default registry"));
            assert_eq!(entry.mechanism, "amd-dash");
        }
    }

    #[test]
    fn test_load_from_missing_file_gives_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("does-not-exist.yaml");
        let cfg = load_from(&path).unwrap();
        assert_eq!(cfg, FleetConfig::default());
    }

    #[test]
    fn test_load_from_invalid_yaml_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fleet.yaml");
        std::fs::write(&path, "netboot_server: [not, a, string]\n").unwrap();

        let err = load_from(&path).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains(&path.display().to_string()));
    }

    #[test]
    fn test_load_from_unknown_field_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fleet.yaml");
        std::fs::write(&path, "netboot_servr: x\n").unwrap();

        let err = load_from(&path).unwrap_err();
        assert!(err.to_string().contains(&path.display().to_string()));
    }

    #[test]
    fn test_load_valid_override() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fleet.yaml");
        std::fs::write(&path, "netboot_server: \"10.0.0.9\"\n").unwrap();

        let cfg = load_from(&path).unwrap();
        assert_eq!(cfg.netboot_server, "10.0.0.9");

        // Every other field is unchanged from the default — the fail-closed
        // parser must not reject or perturb a legitimate partial file.
        let default = FleetConfig::default();
        assert_eq!(cfg.flip_api_port, default.flip_api_port);
        assert_eq!(cfg.cloud_init_base, default.cloud_init_base);
        assert_eq!(cfg.server_user, default.server_user);
        assert_eq!(cfg.tang_urls, default.tang_urls);
        assert_eq!(cfg.luks_partition, default.luks_partition);
        assert_eq!(cfg.lenserv_nic, default.lenserv_nic);
        assert_eq!(cfg.power_server, default.power_server);
        assert_eq!(cfg.reinstall_deny, default.reinstall_deny);
        assert_eq!(cfg.power_hosts, default.power_hosts);
    }

    #[test]
    fn test_lookup_host_from_fleet_registry() {
        // Exercises power::lookup_host end-to-end over the default registry
        // via the process-wide `fleet()` accessor (guarded by UAA_FLEET_CONFIG
        // in CI/dev — see module docs).
        assert_eq!(
            power::lookup_host("unimatrixone"),
            Some(power::PowerMechanism::Ipmi {
                bmc_host: "172.16.3.150",
                username: "ADMIN",
            })
        );
        assert_eq!(
            power::lookup_host("len-serv-002"),
            Some(power::PowerMechanism::AmdDash)
        );
        assert_eq!(power::lookup_host("nonexistent"), None);
    }
}
