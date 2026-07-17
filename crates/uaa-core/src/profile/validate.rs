// file: crates/uaa-core/src/profile/validate.rs
// version: 1.1.0
// guid: 4ab394df-7428-4813-b3ee-0eab0df57448
// last-edited: 2026-07-17

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
        let group = groups_by_name.get(profile.group_name.as_str()).ok_or_else(|| {
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
        let width = token.strip_prefix("index:0").and_then(|w| w.parse::<usize>().ok());
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
        0 => violations.push(
            "no group is marked is_standalone=true; exactly one is required".to_string(),
        ),
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
    use super::*;
    use super::super::InstallationConfigPartial;

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
}
