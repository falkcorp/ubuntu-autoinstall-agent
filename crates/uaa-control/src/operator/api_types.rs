// file: crates/uaa-control/src/operator/api_types.rs
// version: 1.1.0
// guid: e0032c3d-53bf-4791-bad1-c20dfdcc0e96
// last-edited: 2026-07-12

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
