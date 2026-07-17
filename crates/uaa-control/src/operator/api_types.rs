// file: crates/uaa-control/src/operator/api_types.rs
// version: 1.2.0
// guid: e0032c3d-53bf-4791-bad1-c20dfdcc0e96
// last-edited: 2026-07-17

//! Operator API response DTOs — field-for-field mirrors of
//! `web/src/api/types.ts` (CT-08's SPA, which pre-declared these shapes
//! against a not-yet-landed CT-07). These are deliberately NOT
//! `crate::db::MachineRow` etc. re-exported: the SPA's `MachineRow` is a
//! reduced+augmented view (adds `consistent`, drops the WAL-only fields),
//! not the full persisted row.

use serde::Serialize;

/// One row from `GET /api/machines` and `GET /api/machines/{mac}`.
#[derive(Debug, Clone, Serialize)]
pub struct MachineRow {
    pub mac: String,
    pub hostname: String,
    pub status: String,
    pub boot_target: String,
    pub tpm_ek: Option<String>,
    /// True when every provisioning layer for this machine agrees.
    ///
    /// PLACEHOLDER for this slice: always `true`. Real cross-layer
    /// consistency checking (registry vs. placed config vs. iPXE boot
    /// target vs. install history) is unimplemented — flagged here rather
    /// than silently faked as a TODO comment nobody greps for.
    pub consistent: bool,
    pub last_seen: String,
}

/// One row from `GET /api/enrollments` (pending enrollment CSRs).
#[derive(Debug, Clone, Serialize)]
pub struct EnrollmentRow {
    pub spki_fingerprint: String,
    pub claimed_mac: String,
    pub claimed_hostname: String,
    pub state: String,
    pub first_seen: String,
}

/// One row from `GET /api/discovered` (unknown PXE MACs / discovery inbox).
#[derive(Debug, Clone, Serialize)]
pub struct DiscoveredMacRow {
    pub mac: String,
    pub first_seen: String,
    pub last_seen: String,
    pub dismissed: bool,
}

/// One row from `GET /api/audit` (chained audit log).
#[derive(Debug, Clone, Serialize)]
pub struct AuditEventRow {
    pub seq: i64,
    pub actor: String,
    pub action: String,
    pub outcome: String,
    pub timestamp: String,
    pub detail: Option<String>,
}

/// Result of `GET /api/audit/verify` — audit chain integrity check.
#[derive(Debug, Clone, Serialize)]
pub struct AuditVerifyResult {
    pub ok: bool,
    pub checked: i64,
    pub message: Option<String>,
}

/// Error body shape the SPA's `apiFetch` parses on non-2xx responses
/// (`web/src/api/types.ts`'s `ApiErrorBody`).
#[derive(Debug, Clone, Serialize)]
pub struct ApiErrorBody {
    pub message: String,
}

// ── Profiles (DS-OPS-01) ──────────────────────────────────────────────────
//
// Hand-written `Serialize`-only views over `crate::db::{HostGroupRow,
// HostProfileRow, HostnameAllocationRow}` (DS-REG-01/02), deliberately NOT a
// re-export of those row types — same convention as `MachineRow` above.
// `defaults`/`overrides`/`applications` stay `serde_json::Value` here (the
// same representation the store persists) rather than the typed
// `uaa_core::profile::InstallationConfigPartial` — a view is read-only wire
// shape, not a second copy of that validation-tier type.

/// One row from `GET /api/groups` / `GET /api/groups/:name` (DS-OPS-01).
#[derive(Debug, Clone, Serialize)]
pub struct HostGroupView {
    pub id: uuid::Uuid,
    pub name: String,
    pub hostname_pattern: String,
    pub is_standalone: bool,
    pub defaults: serde_json::Value,
    pub applications: serde_json::Value,
    pub version: i64,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
}

/// One row from `GET /api/groups/:name/profiles` (DS-OPS-01).
#[derive(Debug, Clone, Serialize)]
pub struct HostProfileView {
    pub id: uuid::Uuid,
    pub group_id: uuid::Uuid,
    pub identity: String,
    pub hostname_override: Option<String>,
    pub overrides: serde_json::Value,
    pub applications: serde_json::Value,
    pub version: i64,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
}

/// One row from `GET /api/groups/:name/allocations` (DS-OPS-01). Includes
/// released/rebound-away rows — same append-only history the store returns
/// (see `ProfileStore::list_allocations`'s doc) — so the SPA can render the
/// NIC-replacement history, not just the currently-bound identity.
#[derive(Debug, Clone, Serialize)]
pub struct AllocationView {
    pub identity: String,
    pub index: i64,
    pub hostname: String,
    pub allocated_at: Option<String>,
    pub released_at: Option<String>,
    pub rebound_to: Option<String>,
}
