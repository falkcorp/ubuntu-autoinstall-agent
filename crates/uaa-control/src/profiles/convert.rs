// file: crates/uaa-control/src/profiles/convert.rs
// version: 0.1.0
// guid: 7c1e9a04-3b2d-4e6a-9f11-2a6b8c0d5e73
// last-edited: 2026-07-18

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

/// Deserializes a stored group row's `defaults`/`applications` JSON back into
/// the typed `HostGroupProfile` the merge/validate passes operate on.
pub(crate) fn group_row_to_profile(row: &HostGroupRow) -> Result<HostGroupProfile, String> {
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
