// file: crates/uaa-core/src/profile/mod.rs
// version: 1.1.0
// guid: a24bb30b-4056-4a4d-9817-673754a41981
// last-edited: 2026-07-22

//! Host-group / per-host profile scaffolding (DS-PRF-01).
//!
//! A [`HostGroupProfile`] carries the defaults shared by every host in a
//! group; a [`HostProfile`] carries a single host's identity plus any
//! per-host overrides. Both tiers use the SAME partial type,
//! [`InstallationConfigPartial`], mirroring every field of
//! [`InstallationConfig`](crate::network::ssh_installer::config::InstallationConfig)
//! as `Option<T>` (or `Option<Option<T>>` where the source field is itself
//! `Option<T>` — see below).
//!
//! This module defines types only. Merge logic lives in [`merge`] (DS-PRF-02)
//! and validation logic lives in [`validate`] (DS-PRF-03); both are empty
//! stubs here so the two sibling tasks each own one disjoint file and never
//! collide on this one.

pub mod merge;
pub mod validate;

use crate::network::ssh_installer::config::ApplicationSpec;
use serde::{Deserialize, Serialize};

/// Every [`InstallationConfig`](crate::network::ssh_installer::config::InstallationConfig)
/// field, all optional. Used for BOTH a group's defaults
/// (`HostGroupProfile::defaults`) and a host's overrides
/// (`HostProfile::overrides`).
///
/// Fields that are already `Option<T>` on `InstallationConfig` become
/// `Option<Option<T>>` here — see the `tpm2_pin` doc comment below for why a
/// plain `Option<T>` would be wrong for those fields.
///
/// `PartialEq` is implemented manually (not derived) below: `TangServer` in
/// `config.rs` does not derive `PartialEq` and this module stays purely
/// additive with respect to `config.rs`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct InstallationConfigPartial {
    pub hostname: Option<String>,
    pub disk_device: Option<String>,
    pub timezone: Option<String>,
    pub luks_key: Option<String>,
    pub root_password: Option<String>,
    pub network_interface: Option<String>,
    pub network_address: Option<String>,
    pub network_gateway: Option<String>,
    pub network_search: Option<String>,
    pub network_nameservers: Option<Vec<String>>,
    pub network_renderer: Option<String>,
    /// Double Option: `None` = inherit, `Some(None)` = explicitly no release
    /// override, `Some(Some(r))` = this release.
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "deserialize_double_option")]
    pub debootstrap_release: Option<Option<String>>,
    /// Double Option — see `debootstrap_release`.
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "deserialize_double_option")]
    pub debootstrap_mirror: Option<Option<String>>,
    pub initramfs_type: Option<crate::network::ssh_installer::config::InitramfsType>,
    pub tang_servers: Option<Vec<crate::network::ssh_installer::config::TangServer>>,
    pub tang_threshold: Option<u8>,
    pub ssh_authorized_keys: Option<Vec<String>>,
    pub enroll_tpm2: Option<bool>,
    /// Double Option: `None` = inherit from the group, `Some(None)` =
    /// explicitly no PIN, `Some(Some(p))` = this PIN. A plain `Option<String>`
    /// here would make a host meant to have NO pin silently inherit the
    /// group's — this is the trap this type exists to prevent.
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "deserialize_double_option")]
    pub tpm2_pin: Option<Option<String>>,
    pub tpm2_pcr_ids: Option<String>,
    pub expect_fido2: Option<bool>,
    pub install_ca_cert: Option<String>,
    pub applications: Option<Vec<ApplicationSpec>>,
    /// Storage layout (PlainLuks default | NativeKeystore). See config.rs.
    pub storage_mode: Option<crate::network::ssh_installer::config::StorageMode>,
    /// Multi-disk roster for NativeKeystore hosts (by-id + role).
    pub disks: Option<Vec<crate::network::ssh_installer::config::DiskSpec>>,
}

/// Deserializes a "double option" field (`Option<Option<T>>`) so that a
/// PRESENT key with a `null` value distinguishes from an ABSENT key.
///
/// Serde's default `Option<Option<T>>` derive collapses `null` to the outer
/// `None` (same as a missing key), because `Option<T>`'s own `Deserialize`
/// impl treats `null` as `None` — so the outer `Option` never sees the key
/// was present at all. Using this as `deserialize_with` on the field makes
/// serde only invoke it when the key IS present, so it can wrap whatever the
/// inner `Option<T>` deserializes to (including `None` for `null`) in an
/// outer `Some`. Combined with `#[serde(default)]` on the field (used when
/// the key is absent, defaulting to the outer `None`), this yields:
///   - key absent            -> `None`       (inherit)
///   - key present as `null` -> `Some(None)` (explicitly no value)
///   - key present as `v`    -> `Some(Some(v))`
fn deserialize_double_option<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    T: Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    Deserialize::deserialize(deserializer).map(Some)
}

impl PartialEq for InstallationConfigPartial {
    fn eq(&self, other: &Self) -> bool {
        let tang_servers_eq = match (&self.tang_servers, &other.tang_servers) {
            (None, None) => true,
            (Some(a), Some(b)) => {
                a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.url == y.url)
            }
            _ => false,
        };
        self.hostname == other.hostname
            && self.disk_device == other.disk_device
            && self.timezone == other.timezone
            && self.luks_key == other.luks_key
            && self.root_password == other.root_password
            && self.network_interface == other.network_interface
            && self.network_address == other.network_address
            && self.network_gateway == other.network_gateway
            && self.network_search == other.network_search
            && self.network_nameservers == other.network_nameservers
            && self.network_renderer == other.network_renderer
            && self.debootstrap_release == other.debootstrap_release
            && self.debootstrap_mirror == other.debootstrap_mirror
            && self.initramfs_type == other.initramfs_type
            && tang_servers_eq
            && self.tang_threshold == other.tang_threshold
            && self.ssh_authorized_keys == other.ssh_authorized_keys
            && self.enroll_tpm2 == other.enroll_tpm2
            && self.tpm2_pin == other.tpm2_pin
            && self.tpm2_pcr_ids == other.tpm2_pcr_ids
            && self.expect_fido2 == other.expect_fido2
            && self.install_ca_cert == other.install_ca_cert
            && self.applications == other.applications
            && self.storage_mode == other.storage_mode
            && self.disks == other.disks
    }
}

/// Defaults shared by every host in a group (e.g. all `unimatrixone-*`
/// hosts), plus applications applied to every member.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostGroupProfile {
    /// The hostname prefix; immutable.
    pub name: String,
    /// Hostname template, default `"{name}-{index:03}"`.
    pub hostname_pattern: String,
    pub is_standalone: bool,
    pub defaults: InstallationConfigPartial,
    pub applications: Vec<ApplicationSpec>,
}

/// A single host: its group membership, identity, and any per-host
/// overrides of the group's defaults.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostProfile {
    pub group_name: String,
    /// The MAC address identifying this host.
    pub identity: String,
    pub hostname_override: Option<String>,
    pub overrides: InstallationConfigPartial,
    pub applications: Vec<ApplicationSpec>,
}

/// Every field of
/// [`CockroachSpec`](crate::network::ssh_installer::config::CockroachSpec),
/// all optional — so a host can override only `locality` without restating
/// `seed_ip` (which has no default). Without this, per-application override
/// degrades to whole-application replace, contradicting the locked model
/// (spec D1).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct CockroachSpecPartial {
    pub version: Option<String>,
    pub port: Option<u16>,
    pub sql_port: Option<u16>,
    pub http_addr: Option<String>,
    pub seed_ip: Option<String>,
    pub cache: Option<String>,
    pub max_sql_memory: Option<String>,
    pub locality: Option<String>,
}

/// Where a resolved field's value came from. Filled by DS-PRF-02.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Group,
    Host,
    Default,
}

/// Maps a resolved `InstallationConfig` field name to the [`Source`] it was
/// resolved from. Filled by DS-PRF-02.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Provenance(pub std::collections::BTreeMap<String, Source>);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::ssh_installer::config::CockroachSpec;

    #[test]
    fn test_partial_all_none_is_legal() {
        let partial = InstallationConfigPartial::default();
        assert_eq!(partial, InstallationConfigPartial {
            hostname: None,
            disk_device: None,
            timezone: None,
            luks_key: None,
            root_password: None,
            network_interface: None,
            network_address: None,
            network_gateway: None,
            network_search: None,
            network_nameservers: None,
            network_renderer: None,
            debootstrap_release: None,
            debootstrap_mirror: None,
            initramfs_type: None,
            tang_servers: None,
            tang_threshold: None,
            ssh_authorized_keys: None,
            enroll_tpm2: None,
            tpm2_pin: None,
            tpm2_pcr_ids: None,
            expect_fido2: None,
            install_ca_cert: None,
            applications: None,
            storage_mode: None,
            disks: None,
        });
    }

    #[test]
    fn test_tpm2_pin_distinguishes_inherit_from_explicit_none() {
        let inherit: InstallationConfigPartial = serde_json::from_str("{}").unwrap();
        assert_eq!(inherit.tpm2_pin, None, "empty object must mean 'inherit'");

        let explicit_none: InstallationConfigPartial =
            serde_json::from_str(r#"{"tpm2_pin": null}"#).unwrap();
        assert_eq!(
            explicit_none.tpm2_pin,
            Some(None),
            "explicit null must mean 'explicitly no PIN'"
        );

        assert_ne!(
            inherit.tpm2_pin, explicit_none.tpm2_pin,
            "inherit (None) and explicitly-none (Some(None)) must never compare equal"
        );
    }

    #[test]
    fn test_partial_rejects_unknown_field() {
        let result: Result<InstallationConfigPartial, _> =
            serde_json::from_str(r#"{"hostnmae": "typo-host"}"#);
        let err = result.expect_err("typo'd key must fail to parse");
        assert!(
            err.to_string().contains("hostnmae"),
            "error must name the offending key, got: {err}"
        );
    }

    #[test]
    fn test_partial_roundtrips_applications() {
        let partial = InstallationConfigPartial {
            applications: Some(vec![ApplicationSpec::Cockroach(CockroachSpec {
                version: "v25.3.0".to_string(),
                port: 36357,
                sql_port: 36257,
                http_addr: ":38080".to_string(),
                seed_ip: "172.16.2.30".to_string(),
                cache: ".25".to_string(),
                max_sql_memory: ".25".to_string(),
                locality: "region=us,cluster-unit=lenovo".to_string(),
            })]),
            ..Default::default()
        };

        let json = serde_json::to_string(&partial).unwrap();
        let roundtripped: InstallationConfigPartial = serde_json::from_str(&json).unwrap();
        assert_eq!(partial, roundtripped);
    }
}
