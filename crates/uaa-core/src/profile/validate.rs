// file: crates/uaa-core/src/profile/validate.rs
// version: 1.2.0
// guid: 4ab394df-7428-4813-b3ee-0eab0df57448
// last-edited: 2026-07-23

//! Validation logic for `HostGroupProfile` / `HostProfile` (DS-PRF-03).
//!
//! Pure functions over slices — no store, no file, no async — mirroring
//! [`crate::autoinstall::host_spec`]'s pure/impure split. Every rejection is
//! [`crate::error::AutoInstallError::ConfigError`]; no new error type is
//! introduced here.
//!
//! **The load-bearing rule is [`check_global_hostname_uniqueness`], not
//! prefix uniqueness.** `hostname_pattern` is free-form per group, so a group
//! `len` with pattern `{name}-serv-{index:03}` and a group `len-serv` with
//! the default `{name}-{index:03}` both render `len-serv-001` despite having
//! distinct `name` prefixes — see `test_distinct_prefixes_can_still_collide`.
//! [`check_prefix_uniqueness`] stays only as a cheap early check with a
//! clearer message; it is not the guarantee.
//!
//! [`check_no_rename`] is a standalone entry point, not wired into
//! [`validate`]: `validate` operates over one snapshot of groups/profiles and
//! has no existing-vs-proposed split to feed it. It exists for a future
//! caller (DS-OPS-01's update-group flow) that already knows, via the store
//! tier's real `Uuid` (`HostGroupRow::id` in `uaa-control`), which existing
//! group a proposed edit corresponds to. `HostGroupProfile` itself carries no
//! `id` — at this pure tier, content-except-`name` equality is the only
//! signal available for "this is the same group, renamed" as opposed to "an
//! unrelated new group that happens to share defaults".

use super::{HostGroupProfile, HostProfile};
use crate::error::{AutoInstallError, Result};
use crate::network::ssh_installer::components::firmware_quirks::FirmwareQuirk;
use crate::network::ssh_installer::config::{
    ApplicationSpec, Arch, HostRole, InstallationConfig, StorageMode,
};
use std::collections::{HashMap, HashSet};

/// Every rule. Collects ALL violations and returns them together — a weak
/// operator fixing one error per round-trip is a bad loop.
///
/// Legal by definition: zero groups (fresh install), a group with no
/// members, and a `hostname_override` in a non-standalone group (it simply
/// wins over the pattern; spec D2).
pub fn validate(groups: &[HostGroupProfile], profiles: &[HostProfile]) -> Result<()> {
    let mut violations = Vec::new();

    for group in groups {
        if let Err(e) = check_hostname_pattern(&group.hostname_pattern) {
            violations.push(e.to_string());
        }
    }
    if let Err(e) = check_prefix_uniqueness(groups) {
        violations.push(e.to_string());
    }
    if let Err(e) = check_standalone_rules(groups, profiles) {
        violations.push(e.to_string());
    }
    if let Err(e) = check_global_hostname_uniqueness(groups, profiles) {
        violations.push(e.to_string());
    }

    if violations.is_empty() {
        Ok(())
    } else {
        Err(AutoInstallError::ConfigError(violations.join("; ")))
    }
}

/// Post-merge composition-legality checks over a fully resolved
/// [`InstallationConfig`] — sibling to [`validate`], which runs pre-merge
/// over [`HostGroupProfile`]/[`HostProfile`]. `validate` catches malformed
/// authoring input; `validate_resolved` catches a *combination* of
/// otherwise-legal fields that doesn't make sense together once everything
/// has been merged down to one wire config (PS-VALIDATE-14).
///
/// Rules enforced, collecting every violation rather than stopping at the
/// first (same "one fix-it round-trip" rationale as [`validate`]):
///
/// 1. `storage_mode == NativeKeystore` requires a non-empty `disks` roster
///    AND `arch == Amd64` — the only board this layout has been proven on
///    (X10DSC+) is amd64; `disk_device`-only PlainLuks hosts never hit this.
/// 2. A non-empty `tang_servers` roster requires
///    `1 <= tang_threshold <= tang_servers.len()`: a threshold of zero is a
///    no-op unlock and a threshold above the roster size can never be met.
/// 3. The D2-B clevis `tpm2` peer path: `tpm2_clevis_peer` is not itself a
///    wire field on [`InstallationConfig`], so there is nothing to read off
///    the resolved form beyond what rule 1 already enforces. The surrogate
///    rule is `storage_mode == NativeKeystore` gating the peer path — rule 1
///    already requires that. The authoring-time cross-check ("an authored
///    NativeKeystore is the only config permitted to reach the D2-B peer")
///    belongs at merge time, outside a post-merge validator's scope, and is
///    intentionally not duplicated here.
/// 4. `arch == Arm64` must NOT carry `FirmwareQuirk::GrubRemovableFallback`
///    — that workaround targets the amd64 X10DSC+ board only.
/// 5. `role == TangServer` permits empty `disks` and empty unlock (a Tang
///    server has neither a ZFS pool nor a Clevis binding of its own) but
///    requires an `ApplicationSpec::TangServer` entry in `applications` — a
///    Tang-server host with no Tang workload is a misconfigured host.
///    `role == InstallTarget` requires both a storage disk plan
///    (`disk_device` set OR `disks` non-empty) and a non-empty unlock
///    (`tang_servers` non-empty OR `enroll_tpm2` true) — an install target
///    with neither is a host nobody can unlock after first boot.
///
/// **Not checked here (PS-INSTALLER-29):** a resolved config carrying
/// non-default disk sizes or `reset_enabled` would be unsupported today, but
/// neither is a wire field on [`InstallationConfig`] yet, so this reduces to
/// a no-op — the guard belongs in `merge`/`lower` once sizes become wire
/// fields, not here.
pub fn validate_resolved(cfg: &InstallationConfig) -> Result<()> {
    let mut violations = Vec::new();

    // Rule 1: NativeKeystore requires a disk roster and an amd64 target.
    if cfg.storage_mode == StorageMode::NativeKeystore {
        if cfg.disks.is_empty() {
            violations.push(
                "storage_mode is native-keystore but disks is empty; \
                 native-keystore requires a non-empty disk roster"
                    .to_string(),
            );
        }
        if cfg.arch != Arch::Amd64 {
            violations.push(format!(
                "storage_mode is native-keystore but arch is {:?}; \
                 native-keystore is amd64-only",
                cfg.arch
            ));
        }
    }

    // Rule 2: an SSS threshold that can actually be met.
    if !cfg.tang_servers.is_empty() {
        let n = cfg.tang_servers.len();
        if cfg.tang_threshold < 1 || (cfg.tang_threshold as usize) > n {
            violations.push(format!(
                "tang_threshold {} is out of range for {n} tang_servers; \
                 must be between 1 and {n}",
                cfg.tang_threshold
            ));
        }
    }

    // Rule 3: no separate check — see doc comment; rule 1 already covers
    // the surrogate (storage_mode == NativeKeystore) for the D2-B peer path.

    // Rule 4: the removable-fallback GRUB quirk is amd64-only.
    if cfg.arch == Arch::Arm64
        && cfg
            .firmware_quirks
            .contains(&FirmwareQuirk::GrubRemovableFallback)
    {
        violations.push(
            "arch is arm64 but firmware_quirks contains grub-removable-fallback; \
             that workaround is amd64-only"
                .to_string(),
        );
    }

    // Rule 5: role-specific requirements.
    match cfg.role {
        HostRole::TangServer => {
            let has_tang_app = cfg
                .applications
                .iter()
                .any(|a| matches!(a, ApplicationSpec::TangServer(_)));
            if !has_tang_app {
                violations.push(
                    "role is tang-server but applications has no tang-server entry".to_string(),
                );
            }
        }
        HostRole::InstallTarget => {
            let has_disk_plan = !cfg.disk_device.is_empty() || !cfg.disks.is_empty();
            if !has_disk_plan {
                violations.push(
                    "role is install-target but neither disk_device nor disks is set; \
                     a storage disk plan is required"
                        .to_string(),
                );
            }
            let has_unlock = !cfg.tang_servers.is_empty() || cfg.enroll_tpm2;
            if !has_unlock {
                violations.push(
                    "role is install-target but unlock is empty (no tang_servers and \
                     enroll_tpm2 is false); at least one unlock factor is required"
                        .to_string(),
                );
            }
        }
    }

    if violations.is_empty() {
        Ok(())
    } else {
        Err(AutoInstallError::ConfigError(violations.join("; ")))
    }
}

/// THE load-bearing rule (spec D2). Materializes every group's hostnames and
/// every `hostname_override`, and rejects any duplicate — across groups, not
/// within one. Also rejects any materialized hostname that is not a
/// DNS-legal label.
pub fn check_global_hostname_uniqueness(
    groups: &[HostGroupProfile],
    profiles: &[HostProfile],
) -> Result<()> {
    let materialized = materialize_hostnames(groups, profiles)?;

    let mut violations = Vec::new();
    let mut claimed_by: HashMap<&str, &str> = HashMap::new();
    for m in &materialized {
        if !is_dns_legal_label(&m.hostname) {
            violations.push(format!(
                "{} materializes hostname {:?}, which is not a DNS-legal label",
                m.origin, m.hostname
            ));
            continue;
        }
        match claimed_by.get(m.hostname.as_str()) {
            Some(prev_origin) => violations.push(format!(
                "hostname {:?} is claimed by both {} and {}",
                m.hostname, prev_origin, m.origin
            )),
            None => {
                claimed_by.insert(&m.hostname, &m.origin);
            }
        }
    }

    if violations.is_empty() {
        Ok(())
    } else {
        Err(AutoInstallError::ConfigError(violations.join("; ")))
    }
}

/// A single materialized hostname plus a human-readable origin, used to build
/// error messages that name the offending host.
struct MaterializedHostname {
    hostname: String,
    origin: String,
}

/// Materializes every profile's hostname: `hostname_override` if set,
/// otherwise the owning group's `hostname_pattern` rendered at the 1-based
/// position of this profile among its group's siblings (in slice order).
fn materialize_hostnames(
    groups: &[HostGroupProfile],
    profiles: &[HostProfile],
) -> Result<Vec<MaterializedHostname>> {
    let groups_by_name: HashMap<&str, &HostGroupProfile> =
        groups.iter().map(|g| (g.name.as_str(), g)).collect();

    let mut next_index: HashMap<&str, u32> = HashMap::new();
    let mut out = Vec::with_capacity(profiles.len());
    for profile in profiles {
        let group = groups_by_name
            .get(profile.group_name.as_str())
            .ok_or_else(|| {
                AutoInstallError::ConfigError(format!(
                    "host {} references unknown group {:?}",
                    profile.identity, profile.group_name
                ))
            })?;

        let slot = next_index.entry(profile.group_name.as_str()).or_insert(0);
        *slot += 1;
        let index = *slot;

        let hostname = match &profile.hostname_override {
            Some(h) => h.clone(),
            None => render_hostname(&group.hostname_pattern, &group.name, index),
        };
        out.push(MaterializedHostname {
            hostname,
            origin: format!("host {} (group {:?})", profile.identity, profile.group_name),
        });
    }
    Ok(out)
}

/// Renders `pattern` for `name` at 1-based `index`, substituting `{name}`
/// and `{index}` / `{index:0N}` (zero-padded to `N` digits).
fn render_hostname(pattern: &str, name: &str, index: u32) -> String {
    let with_name = pattern.replace("{name}", name);

    let mut out = String::new();
    let mut rest = with_name.as_str();
    while let Some(start) = rest.find("{index") {
        out.push_str(&rest[..start]);
        let after = &rest[start..];
        let Some(end) = after.find('}') else {
            // Unterminated placeholder: emit literally rather than panic.
            out.push_str(after);
            rest = "";
            break;
        };
        let token = &after[1..end]; // "index" or "index:0N"
        let width = token
            .strip_prefix("index:0")
            .and_then(|w| w.parse::<usize>().ok());
        match width {
            Some(width) => out.push_str(&format!("{index:0width$}")),
            None => out.push_str(&index.to_string()),
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

/// The cheap early check: rejects two groups sharing the same `name`
/// (prefix). Necessary but NOT sufficient for hostname uniqueness — see the
/// module doc and [`check_global_hostname_uniqueness`].
pub fn check_prefix_uniqueness(groups: &[HostGroupProfile]) -> Result<()> {
    let mut seen: HashSet<&str> = HashSet::new();
    let mut dupes: Vec<&str> = Vec::new();
    for g in groups {
        if !seen.insert(g.name.as_str()) {
            dupes.push(g.name.as_str());
        }
    }
    if dupes.is_empty() {
        Ok(())
    } else {
        Err(AutoInstallError::ConfigError(format!(
            "duplicate group name(s): {}",
            dupes.join(", ")
        )))
    }
}

/// Spec D3: exactly one group has `is_standalone == true` (skipped when
/// `groups` is empty — a fresh install has no groups yet and that is legal);
/// every `HostProfile` in it must carry an explicit `hostname_override`.
///
/// Deliberately does NOT flag a second standalone host: `vm-test` and
/// `unimatrixone` are 2 of 5 real machines and both legitimately live in the
/// standalone group. A rule that rejects a second standalone member would
/// fail on the fleet's normal state from day one.
pub fn check_standalone_rules(groups: &[HostGroupProfile], profiles: &[HostProfile]) -> Result<()> {
    if groups.is_empty() {
        return Ok(());
    }

    let mut violations = Vec::new();
    let standalone: Vec<&HostGroupProfile> = groups.iter().filter(|g| g.is_standalone).collect();
    match standalone.len() {
        1 => {}
        0 => violations
            .push("no group is marked is_standalone=true; exactly one is required".to_string()),
        n => violations.push(format!(
            "{n} groups are marked is_standalone=true; exactly one is required"
        )),
    }

    for group in &standalone {
        for profile in profiles.iter().filter(|p| p.group_name == group.name) {
            if profile.hostname_override.is_none() {
                violations.push(format!(
                    "host {} in standalone group {:?} must carry an explicit hostname_override",
                    profile.identity, group.name
                ));
            }
        }
    }

    if violations.is_empty() {
        Ok(())
    } else {
        Err(AutoInstallError::ConfigError(violations.join("; ")))
    }
}

/// `hostname_pattern` must contain an `{index` placeholder — without one it
/// renders the same name for every member of the group.
pub fn check_hostname_pattern(pattern: &str) -> Result<()> {
    if pattern.contains("{index") {
        Ok(())
    } else {
        Err(AutoInstallError::ConfigError(format!(
            "hostname_pattern {pattern:?} has no {{index}} placeholder; every member would render the same hostname"
        )))
    }
}

/// A DNS-legal label: 1-63 characters, `[a-z0-9-]` only, and no leading or
/// trailing `-`.
pub fn is_dns_legal_label(s: &str) -> bool {
    if s.is_empty() || s.len() > 63 {
        return false;
    }
    if s.starts_with('-') || s.ends_with('-') {
        return false;
    }
    s.bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Group names are immutable (spec D2). Detects an attempted rename: a
/// `proposed` group whose content matches an `existing` group in every field
/// except `name`. Renaming is done by creating a new group and rebinding
/// hosts (DS-REG-03), never by editing `name` in place.
///
/// Not called from [`validate`] — see the module doc for why.
pub fn check_no_rename(existing: &[HostGroupProfile], proposed: &HostGroupProfile) -> Result<()> {
    for g in existing {
        if g.name == proposed.name {
            continue;
        }
        let same_content_different_name = HostGroupProfile {
            name: proposed.name.clone(),
            ..g.clone()
        } == *proposed;
        if same_content_different_name {
            return Err(AutoInstallError::ConfigError(format!(
                "group {:?} has the same content as existing group {:?} under a different name; \
                 group names are immutable — create a new group and rebind hosts instead",
                proposed.name, g.name
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::InstallationConfigPartial;
    use super::*;

    fn group(name: &str, pattern: &str, is_standalone: bool) -> HostGroupProfile {
        HostGroupProfile {
            name: name.to_string(),
            hostname_pattern: pattern.to_string(),
            is_standalone,
            defaults: InstallationConfigPartial::default(),
            applications: Vec::new(),
        }
    }

    fn profile(group_name: &str, identity: &str, hostname_override: Option<&str>) -> HostProfile {
        HostProfile {
            group_name: group_name.to_string(),
            identity: identity.to_string(),
            hostname_override: hostname_override.map(|s| s.to_string()),
            overrides: InstallationConfigPartial::default(),
            applications: Vec::new(),
        }
    }

    const DEFAULT_PATTERN: &str = "{name}-{index:03}";

    /// The whole point: `len`'s pattern renders `len-serv-001` and
    /// `len-serv`'s default pattern *also* renders `len-serv-001` — distinct
    /// prefixes, colliding hostnames.
    #[test]
    fn test_distinct_prefixes_can_still_collide() {
        let groups = vec![
            group("len", "{name}-serv-{index:03}", false),
            group("len-serv", DEFAULT_PATTERN, false),
        ];
        let profiles = vec![
            profile("len", "aa:bb:cc:dd:ee:01", None),
            profile("len-serv", "aa:bb:cc:dd:ee:02", None),
        ];
        let err = check_global_hostname_uniqueness(&groups, &profiles).unwrap_err();
        assert!(err.to_string().contains("len-serv-001"), "got: {err}");
    }

    #[test]
    fn test_hostname_override_collides_with_generated() {
        let groups = vec![
            group("len-serv", DEFAULT_PATTERN, false), // -> len-serv-001
            group("other", DEFAULT_PATTERN, false),
        ];
        let profiles = vec![
            profile("len-serv", "aa:bb:cc:dd:ee:01", None),
            profile("other", "aa:bb:cc:dd:ee:02", Some("len-serv-001")),
        ];
        let err = check_global_hostname_uniqueness(&groups, &profiles).unwrap_err();
        assert!(err.to_string().contains("len-serv-001"), "got: {err}");
    }

    #[test]
    fn test_duplicate_prefix_rejected() {
        let groups = vec![
            group("len-serv", DEFAULT_PATTERN, false),
            group("len-serv", DEFAULT_PATTERN, false),
        ];
        let err = check_prefix_uniqueness(&groups).unwrap_err();
        assert!(err.to_string().contains("len-serv"), "got: {err}");
    }

    #[test]
    fn test_group_rename_rejected() {
        let existing = vec![group("len-serv", DEFAULT_PATTERN, false)];
        let mut proposed = existing[0].clone();
        proposed.name = "lenserv-renamed".to_string();

        let err = check_no_rename(&existing, &proposed).unwrap_err();
        assert!(err.to_string().contains("len-serv"), "got: {err}");
        assert!(err.to_string().contains("lenserv-renamed"), "got: {err}");
    }

    #[test]
    fn test_exactly_one_standalone() {
        let two_standalone = vec![
            group("a", DEFAULT_PATTERN, true),
            group("b", DEFAULT_PATTERN, true),
        ];
        assert!(check_standalone_rules(&two_standalone, &[]).is_err());

        let zero_standalone = vec![group("a", DEFAULT_PATTERN, false)];
        assert!(check_standalone_rules(&zero_standalone, &[]).is_err());
    }

    #[test]
    fn test_standalone_requires_explicit_hostname() {
        let groups = vec![group("standalone", DEFAULT_PATTERN, true)];
        let profiles = vec![profile("standalone", "aa:bb:cc:dd:ee:01", None)];
        assert!(check_standalone_rules(&groups, &profiles).is_err());
    }

    /// `vm-test` and `unimatrixone` are 2 of 5 real machines and both
    /// legitimately live in the standalone group — this must be `Ok`, with
    /// no rejection of any kind.
    #[test]
    fn test_second_standalone_host_is_legal() {
        let groups = vec![group("standalone", DEFAULT_PATTERN, true)];
        let profiles = vec![
            profile("standalone", "aa:bb:cc:dd:ee:01", Some("vm-test")),
            profile("standalone", "aa:bb:cc:dd:ee:02", Some("unimatrixone")),
        ];
        assert!(check_standalone_rules(&groups, &profiles).is_ok());
    }

    #[test]
    fn test_pattern_without_index_rejected() {
        let err = check_hostname_pattern("{name}-server").unwrap_err();
        assert!(err.to_string().contains("{name}-server"), "got: {err}");
    }

    #[test]
    fn test_dns_illegal_hostname_rejected() {
        let groups = vec![group("g", DEFAULT_PATTERN, false)];
        let profiles = vec![profile("g", "aa:bb:cc:dd:ee:01", Some("Len_Serv!"))];
        let err = check_global_hostname_uniqueness(&groups, &profiles).unwrap_err();
        assert!(err.to_string().contains("Len_Serv!"), "got: {err}");
    }

    /// The real fleet shape: a `len-serv` group with 3 members plus a
    /// `standalone` group with `vm-test` and `unimatrixone`. Must validate
    /// clean — an over-strict rule here rejects the fleet this system exists
    /// to deploy.
    #[test]
    fn test_valid_fleet_passes() {
        let groups = vec![
            group("len-serv", DEFAULT_PATTERN, false),
            group("standalone", DEFAULT_PATTERN, true),
        ];
        let profiles = vec![
            profile("len-serv", "aa:bb:cc:dd:ee:01", None),
            profile("len-serv", "aa:bb:cc:dd:ee:02", None),
            profile("len-serv", "aa:bb:cc:dd:ee:03", None),
            profile("standalone", "aa:bb:cc:dd:ee:04", Some("vm-test")),
            profile("standalone", "aa:bb:cc:dd:ee:05", Some("unimatrixone")),
        ];
        assert!(validate(&groups, &profiles).is_ok());
    }

    #[test]
    fn test_zero_groups_is_legal() {
        assert!(validate(&[], &[]).is_ok());
    }

    // -- validate_resolved (PS-VALIDATE-14) --

    use crate::network::ssh_installer::config::{DiskRole, DiskSpec, TangServer, TangServerSpec};

    /// A minimal legal `InstallationConfig`: `PlainLuks`/amd64/`InstallTarget`
    /// with a disk_device and TPM2 unlock — mirrors the pattern used by
    /// `applications.rs::sample_config` elsewhere in this crate.
    fn base_config() -> InstallationConfig {
        InstallationConfig {
            hostname: "test-host".into(),
            disk_device: "/dev/nvme0n1".into(),
            timezone: "UTC".into(),
            luks_key: "key".into(),
            root_password: "root".into(),
            network_interface: "eth0".into(),
            network_address: "192.0.2.10/24".into(),
            network_gateway: "192.0.2.1".into(),
            network_search: "example.test".into(),
            network_nameservers: vec!["1.1.1.1".into()],
            network_renderer: crate::network::ssh_installer::config::default_network_renderer(),
            debootstrap_release: None,
            debootstrap_mirror: None,
            initramfs_type: Default::default(),
            tang_servers: vec![],
            tang_threshold: 2,
            ssh_authorized_keys: vec![],
            enroll_tpm2: true,
            tpm2_pin: None,
            tpm2_pcr_ids: "7".into(),
            expect_fido2: true,
            install_ca_cert: "test-ca-pem".into(),
            applications: vec![],
            storage_mode: StorageMode::PlainLuks,
            disks: Vec::new(),
            arch: Arch::Amd64,
            role: HostRole::InstallTarget,
            firmware_quirks: Vec::new(),
            hooks: Default::default(),
        }
    }

    fn disk(id: &str, role: DiskRole) -> DiskSpec {
        DiskSpec {
            id: id.to_string(),
            role,
        }
    }

    #[test]
    fn test_base_config_passes() {
        assert!(validate_resolved(&base_config()).is_ok());
    }

    // Rule 1: NativeKeystore requires disks + amd64.

    #[test]
    fn test_rule1_native_keystore_with_disks_and_amd64_passes() {
        let mut cfg = base_config();
        cfg.storage_mode = StorageMode::NativeKeystore;
        cfg.disks = vec![disk("disk-a", DiskRole::System)];
        cfg.arch = Arch::Amd64;
        assert!(validate_resolved(&cfg).is_ok());
    }

    #[test]
    fn test_rule1_native_keystore_without_disks_fails() {
        let mut cfg = base_config();
        cfg.storage_mode = StorageMode::NativeKeystore;
        cfg.disks = Vec::new();
        cfg.arch = Arch::Amd64;
        let err = validate_resolved(&cfg).unwrap_err();
        assert!(
            err.to_string()
                .contains("native-keystore requires a non-empty disk roster"),
            "got: {err}"
        );
    }

    #[test]
    fn test_rule1_native_keystore_on_arm64_fails() {
        let mut cfg = base_config();
        cfg.storage_mode = StorageMode::NativeKeystore;
        cfg.disks = vec![disk("disk-a", DiskRole::System)];
        cfg.arch = Arch::Arm64;
        let err = validate_resolved(&cfg).unwrap_err();
        assert!(
            err.to_string().contains("native-keystore is amd64-only"),
            "got: {err}"
        );
    }

    // Rule 2: tang_threshold in range.

    #[test]
    fn test_rule2_threshold_in_range_passes() {
        let mut cfg = base_config();
        cfg.tang_servers = vec![
            TangServer {
                url: "http://tang1".into(),
            },
            TangServer {
                url: "http://tang2".into(),
            },
        ];
        cfg.tang_threshold = 1;
        assert!(validate_resolved(&cfg).is_ok());
    }

    #[test]
    fn test_rule2_threshold_above_server_count_fails() {
        let mut cfg = base_config();
        cfg.tang_servers = vec![TangServer {
            url: "http://tang1".into(),
        }];
        cfg.tang_threshold = 2;
        let err = validate_resolved(&cfg).unwrap_err();
        assert!(
            err.to_string()
                .contains("tang_threshold 2 is out of range for 1 tang_servers"),
            "got: {err}"
        );
    }

    #[test]
    fn test_rule2_zero_threshold_fails() {
        let mut cfg = base_config();
        cfg.tang_servers = vec![TangServer {
            url: "http://tang1".into(),
        }];
        cfg.tang_threshold = 0;
        let err = validate_resolved(&cfg).unwrap_err();
        assert!(
            err.to_string().contains("tang_threshold 0 is out of range"),
            "got: {err}"
        );
    }

    // Rule 4: arm64 must not carry GrubRemovableFallback.

    #[test]
    fn test_rule4_arm64_without_grub_quirk_passes() {
        let mut cfg = base_config();
        cfg.arch = Arch::Arm64;
        cfg.firmware_quirks = Vec::new();
        assert!(validate_resolved(&cfg).is_ok());
    }

    #[test]
    fn test_rule4_arm64_with_grub_removable_fallback_fails() {
        let mut cfg = base_config();
        cfg.arch = Arch::Arm64;
        cfg.firmware_quirks = vec![FirmwareQuirk::GrubRemovableFallback];
        let err = validate_resolved(&cfg).unwrap_err();
        assert!(
            err.to_string().contains("that workaround is amd64-only"),
            "got: {err}"
        );
    }

    #[test]
    fn test_rule4_amd64_with_grub_removable_fallback_passes() {
        let mut cfg = base_config();
        cfg.arch = Arch::Amd64;
        cfg.firmware_quirks = vec![FirmwareQuirk::GrubRemovableFallback];
        assert!(validate_resolved(&cfg).is_ok());
    }

    // Rule 5: role-specific requirements.

    #[test]
    fn test_rule5_tang_server_with_tang_app_passes() {
        let mut cfg = base_config();
        cfg.role = HostRole::TangServer;
        cfg.disk_device = String::new();
        cfg.tang_servers = Vec::new();
        cfg.enroll_tpm2 = false;
        cfg.applications = vec![ApplicationSpec::TangServer(TangServerSpec {
            port: 80,
            key_directory: "/etc/tang/keys".into(),
        })];
        assert!(validate_resolved(&cfg).is_ok());
    }

    #[test]
    fn test_rule5_tang_server_without_tang_app_fails() {
        let mut cfg = base_config();
        cfg.role = HostRole::TangServer;
        cfg.applications = Vec::new();
        let err = validate_resolved(&cfg).unwrap_err();
        assert!(
            err.to_string()
                .contains("role is tang-server but applications has no tang-server entry"),
            "got: {err}"
        );
    }

    #[test]
    fn test_rule5_install_target_with_disk_and_unlock_passes() {
        let mut cfg = base_config();
        cfg.role = HostRole::InstallTarget;
        cfg.disk_device = "/dev/nvme0n1".into();
        cfg.enroll_tpm2 = true;
        assert!(validate_resolved(&cfg).is_ok());
    }

    #[test]
    fn test_rule5_install_target_with_empty_unlock_fails() {
        let mut cfg = base_config();
        cfg.role = HostRole::InstallTarget;
        cfg.disk_device = "/dev/nvme0n1".into();
        cfg.tang_servers = Vec::new();
        cfg.enroll_tpm2 = false;
        let err = validate_resolved(&cfg).unwrap_err();
        assert!(
            err.to_string()
                .contains("role is install-target but unlock is empty"),
            "got: {err}"
        );
    }

    #[test]
    fn test_rule5_install_target_without_disk_plan_fails() {
        let mut cfg = base_config();
        cfg.role = HostRole::InstallTarget;
        cfg.disk_device = String::new();
        cfg.disks = Vec::new();
        cfg.enroll_tpm2 = true;
        let err = validate_resolved(&cfg).unwrap_err();
        assert!(
            err.to_string()
                .contains("role is install-target but neither disk_device nor disks is set"),
            "got: {err}"
        );
    }

    #[test]
    fn test_all_violations_collected_together() {
        let mut cfg = base_config();
        cfg.storage_mode = StorageMode::NativeKeystore;
        cfg.disks = Vec::new();
        cfg.arch = Arch::Arm64;
        cfg.firmware_quirks = vec![FirmwareQuirk::GrubRemovableFallback];
        cfg.tang_servers = vec![TangServer {
            url: "http://tang1".into(),
        }];
        cfg.tang_threshold = 5;
        let err = validate_resolved(&cfg).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("non-empty disk roster"), "got: {msg}");
        assert!(msg.contains("amd64-only"), "got: {msg}");
        assert!(
            msg.contains("tang_threshold 5 is out of range"),
            "got: {msg}"
        );
    }
}
