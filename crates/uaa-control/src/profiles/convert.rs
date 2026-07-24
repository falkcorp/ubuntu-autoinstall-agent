// file: crates/uaa-control/src/profiles/convert.rs
// version: 0.2.0
// guid: 7c1e9a04-3b2d-4e6a-9f11-2a6b8c0d5e73
// last-edited: 2026-07-23

//! Row → typed-profile converters, shared by the operator handlers
//! (`validate_snapshot`) and registry resolution (`resolve_from_registry`,
//! DS-OPS-03).
//!
//! These deserialize a stored group/profile row's `defaults`/`overrides`/
//! `applications` JSON back into the typed `uaa_core::profile` tier the merge
//! and validate passes operate on. Kept `pub(crate)` — NEVER `pub`: they are an
//! internal conversion detail, not part of `uaa-control`'s public surface. An
//! `Err` here means the store holds a row this crate itself never could have
//! written (every write path serializes FROM these same types), so callers
//! surface it as a 500 / fail-closed, not the current request's fault.

use uaa_core::network::ssh_installer::config::ApplicationSpec;
use uaa_core::profile::{HostGroupProfile, HostProfile, InstallationConfigPartial};

use crate::db::{HostGroupRow, HostProfileRow};
use crate::profiles::store::ensure_schema_servable;

/// Deserializes a stored group row's `defaults`/`applications` JSON back into
/// the typed `HostGroupProfile` the merge/validate passes operate on.
///
/// The envelope [`ensure_schema_servable`] gate runs FIRST, before any blob is
/// deserialized: a row written by a newer binary (`schema_version` >
/// [`SCHEMA_VERSION_MAX`][crate::profiles::store::SCHEMA_VERSION_MAX]) is
/// refused with the fixed `schema version {n} exceeds binary max {MAX}` message
/// (group-scoped) rather than mis-parsed against a shape this binary predates.
pub(crate) fn group_row_to_profile(row: &HostGroupRow) -> Result<HostGroupProfile, String> {
    ensure_schema_servable(row.schema_version)
        .map_err(|e| format!("group {:?}: {e}", row.name))?;
    let defaults: InstallationConfigPartial = serde_json::from_value(row.defaults.clone())
        .map_err(|e| format!("group {:?}: stored defaults failed to parse: {e}", row.name))?;
    let applications: Vec<ApplicationSpec> = serde_json::from_value(row.applications.clone())
        .map_err(|e| {
            format!(
                "group {:?}: stored applications failed to parse: {e}",
                row.name
            )
        })?;
    Ok(HostGroupProfile {
        name: row.name.clone(),
        hostname_pattern: row.hostname_pattern.clone(),
        is_standalone: row.is_standalone,
        defaults,
        applications,
    })
}

/// Same as [`group_row_to_profile`] but for a profile row; `group_name` is
/// resolved by the caller (a `HostProfileRow` only carries `group_id`).
pub(crate) fn profile_row_to_profile(
    row: &HostProfileRow,
    group_name: &str,
) -> Result<HostProfile, String> {
    ensure_schema_servable(row.schema_version)
        .map_err(|e| format!("host {:?}: {e}", row.identity))?;
    let overrides: InstallationConfigPartial = serde_json::from_value(row.overrides.clone())
        .map_err(|e| {
            format!(
                "host {:?}: stored overrides failed to parse: {e}",
                row.identity
            )
        })?;
    let applications: Vec<ApplicationSpec> = serde_json::from_value(row.applications.clone())
        .map_err(|e| {
            format!(
                "host {:?}: stored applications failed to parse: {e}",
                row.identity
            )
        })?;
    Ok(HostProfile {
        group_name: group_name.to_string(),
        identity: row.identity.clone(),
        hostname_override: row.hostname_override.clone(),
        overrides,
        applications,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn group_row(defaults: serde_json::Value, schema_version: i64) -> HostGroupRow {
        HostGroupRow {
            id: Uuid::new_v4(),
            name: "prod".to_string(),
            hostname_pattern: "{name}-{index:03}".to_string(),
            is_standalone: false,
            defaults,
            applications: serde_json::json!([]),
            content_hash: vec![],
            version: 1,
            schema_version,
            created_at: None,
            updated_at: None,
        }
    }

    fn profile_row(overrides: serde_json::Value, schema_version: i64) -> HostProfileRow {
        HostProfileRow {
            id: Uuid::new_v4(),
            group_id: Uuid::new_v4(),
            identity: "aa:bb:cc:dd:ee:ff".to_string(),
            hostname_override: None,
            overrides,
            applications: serde_json::json!([]),
            content_hash: vec![],
            version: 1,
            schema_version,
            created_at: None,
            updated_at: None,
        }
    }

    #[test]
    fn test_convert_deserializes_component_bearing_blob() {
        // A blob carrying the NEW component keys wired by PS-WIRE-PARTIAL-11
        // (arch / base_image / firmware_quirks / unlock_policy…) must round-trip
        // into the typed InstallationConfigPartial — this is what "every binary
        // now recognizes the new keys" means for the expand step.
        let row = group_row(
            serde_json::json!({
                "arch": "arm64",
                "base_image": { "release": "jammy" },
                "firmware_quirks": [],
            }),
            0,
        );
        let profile = group_row_to_profile(&row).expect("component keys must deserialize");
        assert_eq!(profile.defaults.arch, Some(uaa_core::network::ssh_installer::config::Arch::Arm64));
        assert!(profile.defaults.base_image.is_some());
        assert_eq!(profile.defaults.firmware_quirks, Some(vec![]));
    }

    #[test]
    fn test_convert_unknown_component_key_is_group_scoped_error() {
        // `deny_unknown_fields` on InstallationConfigPartial means an unknown key
        // is a hard parse error — and convert names the GROUP so an operator can
        // find the offending row. (This brief mutates no stored blob; it only
        // teaches the parser to reject unknown keys with a located error.)
        let row = group_row(serde_json::json!({ "not_a_real_field": true }), 0);
        let err = group_row_to_profile(&row).unwrap_err();
        assert!(err.contains("prod"), "error must name the group: {err}");
        assert!(err.contains("unknown field"), "expected a deny_unknown_fields error: {err}");
    }

    #[test]
    fn test_convert_unknown_component_key_is_host_scoped_error() {
        let row = profile_row(serde_json::json!({ "not_a_real_field": true }), 0);
        let err = profile_row_to_profile(&row, "prod").unwrap_err();
        assert!(
            err.contains("aa:bb:cc:dd:ee:ff"),
            "error must name the host identity: {err}"
        );
        assert!(err.contains("unknown field"), "expected a deny_unknown_fields error: {err}");
    }

    #[test]
    fn test_convert_refuses_future_schema_without_deserializing_blob() {
        // schema_version > MAX must be refused with the fixed version message,
        // and the DELIBERATELY UNPARSEABLE blob must never be touched: if the
        // gate ran AFTER from_value, we'd see a "failed to parse" error instead.
        let row = group_row(serde_json::json!({ "not_a_real_field": true }), 2);
        let err = group_row_to_profile(&row).unwrap_err();
        assert!(
            err.contains("schema version 2 exceeds binary max 1"),
            "expected the version-gate message, got: {err}"
        );
        assert!(
            !err.contains("failed to parse"),
            "the blob must NOT have been deserialized before the version gate: {err}"
        );

        let prow = profile_row(serde_json::json!({ "not_a_real_field": true }), 2);
        let perr = profile_row_to_profile(&prow, "prod").unwrap_err();
        assert!(perr.contains("schema version 2 exceeds binary max 1"), "{perr}");
        assert!(!perr.contains("failed to parse"), "{perr}");
    }
}
