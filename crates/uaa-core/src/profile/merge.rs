// file: crates/uaa-core/src/profile/merge.rs
// version: 1.2.1
// guid: 57838356-b351-42f5-aa90-c87c98761e81
// last-edited: 2026-07-23

//! Merge logic for `InstallationConfigPartial` -> `InstallationConfig` (DS-PRF-02).
//!
//! [`merge`] resolves a [`HostGroupProfile`](super::HostGroupProfile)'s
//! defaults and a [`HostProfile`](super::HostProfile)'s overrides into a
//! concrete [`InstallationConfig`], recording per-field
//! [`Provenance`](super::Provenance) so "why does this host have this
//! value?" is answerable without recomputing two blobs by hand.
//!
//! **Precedence, per scalar field:** `host.overrides.<f>` if `Some` wins
//! ([`Source::Host`](super::Source::Host)); else `group.defaults.<f>` if
//! `Some` ([`Source::Group`](super::Source::Group)); else the field's own
//! serde default ([`Source::Default`](super::Source::Default)); else — for
//! exactly the 10 fields with no serde default — an error naming every
//! missing field at once.
//!
//! **⚠ Fail-closed applies to exactly 10 fields, not "any unset field".**
//! `InstallationConfig` carries nine `#[serde(default)]` fields plus three
//! `Option<T>` fields (`tpm2_pin`, `debootstrap_release`,
//! `debootstrap_mirror`) that are implicitly optional. A merge that errors
//! on any unset field rejects `examples/configs/install/len-serv-001.yaml`,
//! which omits `network_renderer` and relies on its default — a config that
//! parses and installs fine today. Only `hostname`, `disk_device`,
//! `timezone`, `luks_key`, `root_password`, `network_interface`,
//! `network_address`, `network_gateway`, `network_search`, and
//! `network_nameservers` fail closed.
//!
//! **Three fields are double `Option`s** (`tpm2_pin`, `debootstrap_release`,
//! `debootstrap_mirror`): `None` means "inherit", `Some(None)` means
//! "explicitly no value — do NOT fall through to the group", and
//! `Some(Some(v))` means "this value". Collapsing this to a plain `Option`
//! would make a host meant to have no TPM PIN silently inherit the group's.
//!
//! **Applications are unioned by kind**, host winning whole-entry for a kind
//! present in both tiers — `HostGroupProfile::applications` and
//! `HostProfile::applications` are both `Vec<ApplicationSpec>` (full specs),
//! so that is the only thing the shipped types can express at the top
//! level. The genuine field-by-field merge primitive — the host's
//! `CockroachSpecPartial` over the group's `CockroachSpec`, so a host can
//! override just `locality` without restating `seed_ip` (which has no
//! default) — is [`merge_cockroach`], kept `pub` for direct use once a
//! caller has a real partial (e.g. from an API request body) rather than a
//! fully-resolved `HostProfile::applications` entry.

use crate::error::{AutoInstallError, Result};
use crate::network::ssh_installer::config::{ApplicationSpec, CockroachSpec, InstallationConfig};
use std::collections::BTreeMap;

use super::{CockroachSpecPartial, HostGroupProfile, HostProfile, Provenance, Source};

/// Resolves a required field (no serde default): `host` wins over `group`;
/// if neither supplies a value, records `name` in `missing` and returns
/// `None`. Callers check `missing` once, after resolving every required
/// field, so a single error names all of them — not just the first.
fn resolve_required<T: Clone>(
    host: &Option<T>,
    group: &Option<T>,
    name: &str,
    provenance: &mut Provenance,
    missing: &mut Vec<String>,
) -> Option<T> {
    if let Some(v) = host {
        provenance.0.insert(name.to_string(), Source::Host);
        return Some(v.clone());
    }
    if let Some(v) = group {
        provenance.0.insert(name.to_string(), Source::Group);
        return Some(v.clone());
    }
    missing.push(name.to_string());
    None
}

/// Resolves a field that carries a serde default: `host` wins over `group`
/// wins over `default` (the field's own `InstallationConfig` default,
/// obtained from [`resolved_defaults`] rather than duplicated here).
fn resolve_defaulted<T: Clone>(
    host: &Option<T>,
    group: &Option<T>,
    default: T,
    name: &str,
    provenance: &mut Provenance,
) -> T {
    if let Some(v) = host {
        provenance.0.insert(name.to_string(), Source::Host);
        return v.clone();
    }
    if let Some(v) = group {
        provenance.0.insert(name.to_string(), Source::Group);
        return v.clone();
    }
    provenance.0.insert(name.to_string(), Source::Default);
    default
}

/// Resolves a double-`Option` field (`tpm2_pin`, `debootstrap_release`,
/// `debootstrap_mirror`). `host` being `Some(_)` — including `Some(None)`,
/// "explicitly no value" — wins outright and does NOT fall through to
/// `group`; only `host == None` ("inherit") consults the group tier.
fn resolve_double_option(
    host: &Option<Option<String>>,
    group: &Option<Option<String>>,
    name: &str,
    provenance: &mut Provenance,
) -> Option<String> {
    if let Some(inner) = host {
        provenance.0.insert(name.to_string(), Source::Host);
        return inner.clone();
    }
    if let Some(inner) = group {
        provenance.0.insert(name.to_string(), Source::Group);
        return inner.clone();
    }
    provenance.0.insert(name.to_string(), Source::Default);
    None
}

/// Builds an `InstallationConfig` from a placeholder document so the 9
/// defaulted fields' literal default values can be read back off it,
/// instead of duplicating them here. Several of the underlying
/// `#[serde(default = "...")]` functions in `config.rs` (the tang
/// threshold, the TPM2/FIDO2 opt-in default, the PCR id list) are private
/// to that module by design — this deserializes through `config.rs`'s own
/// `Deserialize` impl, which calls them internally, rather than requiring
/// them to be made `pub` (out of scope: this task owns `merge.rs` only).
/// The 10 fields with no default get throwaway placeholders; `merge` never
/// reads them off this value.
fn resolved_defaults() -> InstallationConfig {
    serde_json::from_str(
        r#"{
            "hostname": "",
            "disk_device": "",
            "timezone": "",
            "luks_key": "",
            "root_password": "",
            "network_interface": "",
            "network_address": "",
            "network_gateway": "",
            "network_search": "",
            "network_nameservers": []
        }"#,
    )
    .expect(
        "InstallationConfig's placeholder document must deserialize: every \
         required field is supplied above and every other field carries a \
         serde default",
    )
}

/// The variant tag used to key the application union — mirrors
/// `ApplicationSpec`'s `#[serde(tag = "kind", rename_all = "kebab-case")]`.
fn app_kind(app: &ApplicationSpec) -> &'static str {
    match app {
        ApplicationSpec::Cockroach(_) => "cockroach",
        ApplicationSpec::TangServer(_) => "tang-server",
    }
}

/// Unions `group` and `host` application lists by kind: a kind present in
/// only one tier is taken as-is; a kind present in both takes the host's
/// whole entry (see the module doc for why this can't be a field-by-field
/// merge at this call site). Ordered deterministically by kind (a
/// `BTreeMap`'s iteration order) so the resolved config is reproducible.
fn union_applications(
    group_apps: &[ApplicationSpec],
    host_apps: &[ApplicationSpec],
) -> (Vec<ApplicationSpec>, Source) {
    let mut by_kind: BTreeMap<&'static str, ApplicationSpec> = BTreeMap::new();
    for app in group_apps {
        by_kind.insert(app_kind(app), app.clone());
    }
    for app in host_apps {
        by_kind.insert(app_kind(app), app.clone());
    }

    let source = if !host_apps.is_empty() {
        Source::Host
    } else if !group_apps.is_empty() {
        Source::Group
    } else {
        Source::Default
    };

    (by_kind.into_values().collect(), source)
}

/// Merges a host's `CockroachSpecPartial` onto a group's `CockroachSpec`,
/// field by field: every `Some` in `overrides` wins, every `None` falls
/// through to `base`. This is what lets a host override only `locality`
/// without restating `seed_ip` (which has no default on `CockroachSpec`
/// itself) — the trap `CockroachSpecPartial` exists to avoid (spec
/// Decision 1).
pub fn merge_cockroach(base: &CockroachSpec, overrides: &CockroachSpecPartial) -> CockroachSpec {
    CockroachSpec {
        version: overrides
            .version
            .clone()
            .unwrap_or_else(|| base.version.clone()),
        port: overrides.port.unwrap_or(base.port),
        sql_port: overrides.sql_port.unwrap_or(base.sql_port),
        http_addr: overrides
            .http_addr
            .clone()
            .unwrap_or_else(|| base.http_addr.clone()),
        seed_ip: overrides
            .seed_ip
            .clone()
            .unwrap_or_else(|| base.seed_ip.clone()),
        cache: overrides.cache.clone().unwrap_or_else(|| base.cache.clone()),
        max_sql_memory: overrides
            .max_sql_memory
            .clone()
            .unwrap_or_else(|| base.max_sql_memory.clone()),
        locality: overrides
            .locality
            .clone()
            .unwrap_or_else(|| base.locality.clone()),
    }
}

/// Resolves a `HostGroupProfile`'s defaults and a `HostProfile`'s overrides
/// into a concrete `InstallationConfig`, plus the per-field `Provenance` of
/// how it got there. See the module doc for precedence, the fail-closed
/// scope, the double-`Option` trap, and the applications union.
pub fn merge(group: &HostGroupProfile, host: &HostProfile) -> Result<(InstallationConfig, Provenance)> {
    let mut provenance = Provenance::default();
    let mut missing: Vec<String> = Vec::new();

    // `hostname` is the one required field NOT sourced from
    // `InstallationConfigPartial`: it comes from the resolved allocation
    // (`host.hostname_override`), or — for a group whose `hostname_pattern`
    // has no `{index}` placeholder (e.g. a standalone group's fixed name) —
    // the pattern itself, used literally. `merge` never renders a
    // `{name}`/`{index}` template; that is the allocator's job.
    let hostname = if let Some(h) = &host.hostname_override {
        provenance.0.insert("hostname".to_string(), Source::Host);
        Some(h.clone())
    } else if !group.hostname_pattern.contains("{index") {
        provenance.0.insert("hostname".to_string(), Source::Group);
        Some(group.hostname_pattern.clone())
    } else {
        missing.push("hostname".to_string());
        None
    };

    let disk_device = resolve_required(
        &host.overrides.disk_device,
        &group.defaults.disk_device,
        "disk_device",
        &mut provenance,
        &mut missing,
    );
    let timezone = resolve_required(
        &host.overrides.timezone,
        &group.defaults.timezone,
        "timezone",
        &mut provenance,
        &mut missing,
    );
    let luks_key = resolve_required(
        &host.overrides.luks_key,
        &group.defaults.luks_key,
        "luks_key",
        &mut provenance,
        &mut missing,
    );
    let root_password = resolve_required(
        &host.overrides.root_password,
        &group.defaults.root_password,
        "root_password",
        &mut provenance,
        &mut missing,
    );
    let network_interface = resolve_required(
        &host.overrides.network_interface,
        &group.defaults.network_interface,
        "network_interface",
        &mut provenance,
        &mut missing,
    );
    let network_address = resolve_required(
        &host.overrides.network_address,
        &group.defaults.network_address,
        "network_address",
        &mut provenance,
        &mut missing,
    );
    let network_gateway = resolve_required(
        &host.overrides.network_gateway,
        &group.defaults.network_gateway,
        "network_gateway",
        &mut provenance,
        &mut missing,
    );
    let network_search = resolve_required(
        &host.overrides.network_search,
        &group.defaults.network_search,
        "network_search",
        &mut provenance,
        &mut missing,
    );
    let network_nameservers = resolve_required(
        &host.overrides.network_nameservers,
        &group.defaults.network_nameservers,
        "network_nameservers",
        &mut provenance,
        &mut missing,
    );

    if !missing.is_empty() {
        return Err(AutoInstallError::ConfigError(format!(
            "profile merge: missing required field(s) with no default and no \
             override from either tier: {}",
            missing.join(", ")
        )));
    }

    let defaults = resolved_defaults();

    let network_renderer = resolve_defaulted(
        &host.overrides.network_renderer,
        &group.defaults.network_renderer,
        defaults.network_renderer.clone(),
        "network_renderer",
        &mut provenance,
    );
    let initramfs_type = resolve_defaulted(
        &host.overrides.initramfs_type,
        &group.defaults.initramfs_type,
        defaults.initramfs_type.clone(),
        "initramfs_type",
        &mut provenance,
    );
    let tang_servers = resolve_defaulted(
        &host.overrides.tang_servers,
        &group.defaults.tang_servers,
        defaults.tang_servers.clone(),
        "tang_servers",
        &mut provenance,
    );
    let tang_threshold = resolve_defaulted(
        &host.overrides.tang_threshold,
        &group.defaults.tang_threshold,
        defaults.tang_threshold,
        "tang_threshold",
        &mut provenance,
    );
    let ssh_authorized_keys = resolve_defaulted(
        &host.overrides.ssh_authorized_keys,
        &group.defaults.ssh_authorized_keys,
        defaults.ssh_authorized_keys.clone(),
        "ssh_authorized_keys",
        &mut provenance,
    );
    let enroll_tpm2 = resolve_defaulted(
        &host.overrides.enroll_tpm2,
        &group.defaults.enroll_tpm2,
        defaults.enroll_tpm2,
        "enroll_tpm2",
        &mut provenance,
    );
    let tpm2_pcr_ids = resolve_defaulted(
        &host.overrides.tpm2_pcr_ids,
        &group.defaults.tpm2_pcr_ids,
        defaults.tpm2_pcr_ids.clone(),
        "tpm2_pcr_ids",
        &mut provenance,
    );
    let expect_fido2 = resolve_defaulted(
        &host.overrides.expect_fido2,
        &group.defaults.expect_fido2,
        defaults.expect_fido2,
        "expect_fido2",
        &mut provenance,
    );
    let install_ca_cert = resolve_defaulted(
        &host.overrides.install_ca_cert,
        &group.defaults.install_ca_cert,
        defaults.install_ca_cert.clone(),
        "install_ca_cert",
        &mut provenance,
    );

    let debootstrap_release = resolve_double_option(
        &host.overrides.debootstrap_release,
        &group.defaults.debootstrap_release,
        "debootstrap_release",
        &mut provenance,
    );
    let debootstrap_mirror = resolve_double_option(
        &host.overrides.debootstrap_mirror,
        &group.defaults.debootstrap_mirror,
        "debootstrap_mirror",
        &mut provenance,
    );
    let tpm2_pin = resolve_double_option(
        &host.overrides.tpm2_pin,
        &group.defaults.tpm2_pin,
        "tpm2_pin",
        &mut provenance,
    );

    let (applications, apps_source) = union_applications(&group.applications, &host.applications);
    provenance.0.insert("applications".to_string(), apps_source);

    // Storage layout resolves host-override → group-default → PlainLuks. A
    // NativeKeystore host (U1 / server profile) carries both its `storage_mode`
    // and its `disks` roster in the override; a Lenovo host omits both and gets
    // the byte-identical PlainLuks path.
    let storage_mode = host
        .overrides
        .storage_mode
        .clone()
        .or_else(|| group.defaults.storage_mode.clone())
        .unwrap_or_default();
    let disks = host
        .overrides
        .disks
        .clone()
        .or_else(|| group.defaults.disks.clone())
        .unwrap_or_default();

    // Every `resolve_required` call above either populated `missing` (and we
    // already returned on that) or returned `Some`; these `expect`s just
    // spell that invariant out for the compiler.
    let config = InstallationConfig {
        hostname: hostname.expect("checked non-missing above"),
        disk_device: disk_device.expect("checked non-missing above"),
        timezone: timezone.expect("checked non-missing above"),
        luks_key: luks_key.expect("checked non-missing above"),
        root_password: root_password.expect("checked non-missing above"),
        network_interface: network_interface.expect("checked non-missing above"),
        network_address: network_address.expect("checked non-missing above"),
        network_gateway: network_gateway.expect("checked non-missing above"),
        network_search: network_search.expect("checked non-missing above"),
        network_nameservers: network_nameservers.expect("checked non-missing above"),
        network_renderer,
        debootstrap_release,
        debootstrap_mirror,
        initramfs_type,
        tang_servers,
        tang_threshold,
        ssh_authorized_keys,
        enroll_tpm2,
        tpm2_pin,
        tpm2_pcr_ids,
        expect_fido2,
        install_ca_cert,
        applications,
        storage_mode,
        disks,
        // Not yet resolvable from group/host overrides — `InstallationConfigPartial`
        // carries no arch/role/firmware_quirks/hooks fields (PS-WIRE-AXES-10 wires
        // them onto `InstallationConfig` only; profile-tier resolution is a later,
        // per-axis task). Every resolved config gets the byte-identical default.
        arch: Default::default(),
        role: Default::default(),
        firmware_quirks: Vec::new(),
        hooks: Default::default(),
    };

    Ok((config, provenance))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::InstallationConfigPartial;

    fn sample_cockroach_spec() -> CockroachSpec {
        CockroachSpec {
            version: "v25.3.0".to_string(),
            port: 36357,
            sql_port: 36257,
            http_addr: ":38080".to_string(),
            seed_ip: "172.16.2.30".to_string(),
            cache: ".25".to_string(),
            max_sql_memory: ".25".to_string(),
            locality: "region=us,cluster-unit=lenovo".to_string(),
        }
    }

    fn full_group_defaults() -> InstallationConfigPartial {
        InstallationConfigPartial {
            disk_device: Some("/dev/nvme0n1".to_string()),
            timezone: Some("America/New_York".to_string()),
            luks_key: Some("REPLACE_AT_PLACE_TIME".to_string()),
            root_password: Some("REPLACE_AT_PLACE_TIME".to_string()),
            network_interface: Some("enp1s0f0".to_string()),
            network_address: Some("172.16.3.96/23".to_string()),
            network_gateway: Some("172.16.2.1".to_string()),
            network_search: Some("jf.local".to_string()),
            network_nameservers: Some(vec!["172.16.2.1".to_string()]),
            ..Default::default()
        }
    }

    fn base_group() -> HostGroupProfile {
        HostGroupProfile {
            name: "len-serv".to_string(),
            hostname_pattern: "{name}-{index:03}".to_string(),
            is_standalone: false,
            defaults: full_group_defaults(),
            applications: Vec::new(),
        }
    }

    fn base_host() -> HostProfile {
        HostProfile {
            group_name: "len-serv".to_string(),
            identity: "aa:bb:cc:dd:ee:ff".to_string(),
            hostname_override: Some("len-serv-003".to_string()),
            overrides: InstallationConfigPartial::default(),
            applications: Vec::new(),
        }
    }

    #[test]
    fn test_merge_host_overrides_group() {
        let group = base_group();
        let mut host = base_host();
        host.overrides.disk_device = Some("/dev/sda".to_string());

        let (config, provenance) = merge(&group, &host).expect("merge should succeed");

        assert_eq!(config.disk_device, "/dev/sda");
        assert_eq!(provenance.0.get("disk_device"), Some(&Source::Host));
    }

    #[test]
    fn test_merge_unset_inherits_group() {
        let group = base_group();
        let host = base_host();

        let (config, provenance) = merge(&group, &host).expect("merge should succeed");

        assert_eq!(config.disk_device, "/dev/nvme0n1");
        assert_eq!(provenance.0.get("disk_device"), Some(&Source::Group));
    }

    #[test]
    fn test_serde_defaulted_field_is_not_unset() {
        // Neither tier sets network_renderer — exactly len-serv-001.yaml's
        // shape. This must resolve, not error.
        let group = base_group();
        let host = base_host();

        let (config, provenance) = merge(&group, &host).expect("merge should succeed");

        assert_eq!(config.network_renderer, "networkd");
        assert_eq!(provenance.0.get("network_renderer"), Some(&Source::Default));
    }

    #[test]
    fn test_merge_fails_closed_on_defaultless_unset_field() {
        let mut group = base_group();
        group.defaults.luks_key = None;
        let host = base_host();

        let err = merge(&group, &host).expect_err("luks_key has no default and no override");

        assert!(
            err.to_string().contains("luks_key"),
            "error must name luks_key, got: {err}"
        );
    }

    #[test]
    fn test_merge_error_names_all_missing_fields() {
        let mut group = base_group();
        group.defaults.luks_key = None;
        group.defaults.root_password = None;
        group.defaults.timezone = None;
        let host = base_host();

        let err = merge(&group, &host).expect_err("three required fields are unset");
        let msg = err.to_string();

        assert!(msg.contains("luks_key"), "msg: {msg}");
        assert!(msg.contains("root_password"), "msg: {msg}");
        assert!(msg.contains("timezone"), "msg: {msg}");
    }

    #[test]
    fn test_tpm2_pin_explicit_none_does_not_inherit() {
        let mut group = base_group();
        group.defaults.tpm2_pin = Some(Some("1234".to_string()));
        let mut host = base_host();
        host.overrides.tpm2_pin = Some(None);

        let (config, provenance) = merge(&group, &host).expect("merge should succeed");

        assert_eq!(config.tpm2_pin, None);
        assert_eq!(provenance.0.get("tpm2_pin"), Some(&Source::Host));
    }

    #[test]
    fn test_tpm2_pin_unset_inherits_group() {
        let mut group = base_group();
        group.defaults.tpm2_pin = Some(Some("1234".to_string()));
        let host = base_host();

        let (config, provenance) = merge(&group, &host).expect("merge should succeed");

        assert_eq!(config.tpm2_pin, Some("1234".to_string()));
        assert_eq!(provenance.0.get("tpm2_pin"), Some(&Source::Group));
    }

    #[test]
    fn test_merge_application_lists_union() {
        let cockroach = ApplicationSpec::Cockroach(sample_cockroach_spec());

        let mut group_only = base_group();
        group_only.applications = vec![cockroach.clone()];
        let host_none = base_host();
        let (config, _provenance) =
            merge(&group_only, &host_none).expect("merge should succeed");
        assert_eq!(config.applications, vec![cockroach.clone()]);

        let group_none = base_group();
        let mut host_only = base_host();
        host_only.applications = vec![cockroach.clone()];
        let (config, _provenance) =
            merge(&group_none, &host_only).expect("merge should succeed");
        assert_eq!(config.applications, vec![cockroach]);
    }

    #[test]
    fn test_merge_application_partial_overrides_field() {
        let group_spec = CockroachSpec {
            locality: "A".to_string(),
            seed_ip: "172.16.2.30".to_string(),
            ..sample_cockroach_spec()
        };
        let host_partial = CockroachSpecPartial {
            locality: Some("B".to_string()),
            ..Default::default()
        };

        let resolved = merge_cockroach(&group_spec, &host_partial);

        assert_eq!(resolved.locality, "B");
        assert_eq!(resolved.seed_ip, "172.16.2.30");
    }

    #[test]
    fn test_merge_passes_placeholders_through() {
        let group = base_group();
        let host = base_host();

        let (config, _provenance) = merge(&group, &host).expect("merge should succeed");

        assert_eq!(config.luks_key, "REPLACE_AT_PLACE_TIME");
    }
}
