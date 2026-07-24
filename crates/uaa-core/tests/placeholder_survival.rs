// file: crates/uaa-core/tests/placeholder_survival.rs
// version: 1.0.0
// guid: fd8d505f-015d-4c23-92aa-f1147f5122bf
// last-edited: 2026-07-24

//! Placeholder-survival test harness (PS-PLACEHOLDER-22).
//!
//! Asserts that the literal `REPLACE_AT_PLACE_TIME` token (see
//! [`uaa_core::config_place::PLACEHOLDER`]) in each secret-bearing FLAT
//! [`InstallationConfig`](uaa_core::network::ssh_installer::config::InstallationConfig)
//! field survives `parse(YAML) -> merge()` unchanged.
//!
//! The current `InstallationConfig` is flat, so this harness exercises the
//! literal field names (no component nesting): `tpm2_pin`, `luks_key`,
//! `root_password`, `install_ca_cert`. It mirrors the assertion style of
//! `test_merge_passes_placeholders_through` (`merge.rs`), NOT
//! `test_tpm2_pin_explicit_none_does_not_inherit` (which tests inheritance,
//! a different property).
//!
//! Because `merge`'s fixture helpers (`base_group`/`base_host`/
//! `full_group_defaults`) are `#[cfg(test)]`-private to `merge.rs`, this
//! harness deserializes its own minimal `HostGroupProfile`/`HostProfile`
//! pair from inline YAML rather than re-exporting the private fixtures.
//!
//! [`assert_placeholder_survives`] is the reusable entry point: future
//! per-field migration briefs (e.g. UNLOCK-27, DISK-28) can call it for
//! their own secret field once that field moves into a nested component.

use uaa_core::config_place::PLACEHOLDER;
use uaa_core::network::ssh_installer::config::InstallationConfig;
use uaa_core::profile::merge::merge;
use uaa_core::profile::{HostGroupProfile, HostProfile};

/// Minimal group YAML carrying every field `merge()` requires with no
/// default (`disk_device`, `timezone`, `network_*`) plus all four
/// secret-bearing fields set to the literal `REPLACE_AT_PLACE_TIME`
/// placeholder, exactly as an unfilled group config authors them.
const GROUP_YAML: &str = r#"
name: len-serv
hostname_pattern: "{name}-{index:03}"
is_standalone: false
applications: []
defaults:
  disk_device: /dev/nvme0n1
  timezone: America/New_York
  luks_key: REPLACE_AT_PLACE_TIME
  root_password: REPLACE_AT_PLACE_TIME
  network_interface: enp1s0f0
  network_address: 172.16.3.96/23
  network_gateway: 172.16.2.1
  network_search: jf.local
  network_nameservers:
    - 172.16.2.1
  tpm2_pin: REPLACE_AT_PLACE_TIME
  install_ca_cert: REPLACE_AT_PLACE_TIME
"#;

/// Minimal host YAML: no overrides, so every secret field inherits the
/// group's placeholder unchanged through `merge()`.
const HOST_YAML: &str = r#"
group_name: len-serv
identity: "aa:bb:cc:dd:ee:ff"
hostname_override: len-serv-003
applications: []
overrides: {}
"#;

/// Parses [`GROUP_YAML`]/[`HOST_YAML`] into a [`HostGroupProfile`]/
/// [`HostProfile`] pair. Kept separate from the assertions so each test
/// below shows exactly what it merges.
fn parse_fixture() -> (HostGroupProfile, HostProfile) {
    let group: HostGroupProfile =
        serde_yaml::from_str(GROUP_YAML).expect("group YAML should parse");
    let host: HostProfile = serde_yaml::from_str(HOST_YAML).expect("host YAML should parse");
    (group, host)
}

/// Reusable assertion: merges `group`/`host` and checks that `extract`
/// (a projection from the resolved [`InstallationConfig`] to the
/// field-under-test) still carries the literal `REPLACE_AT_PLACE_TIME`
/// placeholder unchanged.
///
/// `field_name` is used only for the failure message. `extract` should
/// return `Some(&str)` for the resolved field's value (e.g.
/// `|c| Some(c.luks_key.as_str())` for a required `String` field, or
/// `|c| c.tpm2_pin.as_deref()` for an `Option<String>` field).
///
/// # Example
///
/// ```ignore
/// let (group, host) = parse_fixture();
/// assert_placeholder_survives("luks_key", &group, &host, |c| Some(c.luks_key.as_str()));
/// ```
fn assert_placeholder_survives(
    field_name: &str,
    group: &HostGroupProfile,
    host: &HostProfile,
    extract: impl Fn(&InstallationConfig) -> Option<&str>,
) {
    let (config, _provenance) = merge(group, host).expect("merge should succeed");
    let value = extract(&config);
    assert_eq!(
        value,
        Some(PLACEHOLDER),
        "field `{field_name}` did not survive parse(YAML) -> merge() unchanged (got {value:?})"
    );
}

#[test]
fn placeholder_survives_luks_key() {
    let (group, host) = parse_fixture();
    assert_placeholder_survives("luks_key", &group, &host, |c| Some(c.luks_key.as_str()));
}

#[test]
fn placeholder_survives_root_password() {
    let (group, host) = parse_fixture();
    assert_placeholder_survives("root_password", &group, &host, |c| {
        Some(c.root_password.as_str())
    });
}

#[test]
fn placeholder_survives_tpm2_pin() {
    let (group, host) = parse_fixture();
    assert_placeholder_survives("tpm2_pin", &group, &host, |c| c.tpm2_pin.as_deref());
}

#[test]
fn placeholder_survives_install_ca_cert() {
    let (group, host) = parse_fixture();
    assert_placeholder_survives("install_ca_cert", &group, &host, |c| {
        Some(c.install_ca_cert.as_str())
    });
}
