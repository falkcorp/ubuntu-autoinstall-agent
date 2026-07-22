// file: crates/uaa-control/src/operator/api_types.rs
// version: 1.5.0
// guid: e0032c3d-53bf-4791-bad1-c20dfdcc0e96
// last-edited: 2026-07-22

//! Operator API response DTOs — field-for-field mirrors of
//! `web/src/api/types.ts` (CT-08's SPA, which pre-declared these shapes
//! against a not-yet-landed CT-07). These are deliberately NOT
//! `crate::db::MachineRow` etc. re-exported: the SPA's `MachineRow` is a
//! reduced+augmented view (adds `consistent`, drops the WAL-only fields),
//! not the full persisted row.

use serde::{Deserialize, Serialize};

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
///
/// `Deserialize` (unlike its sibling view types) because this row is not just
/// an API projection: it is the on-disk shape persisted to
/// `discovered-macs.json` by [`crate::discovered::DiscoveredStore`], so it must
/// round-trip back in.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredMacRow {
    pub mac: String,
    /// Last IP the scanner saw this MAC at (from the neighbor table). Drives
    /// hostname resolution. `#[serde(default)]` so pre-enrichment rows still load.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ip: Option<String>,
    /// Resolved hostname (server-side `getent hosts <ip>`, i.e. `/etc/hosts`,
    /// DNS, and dnsmasq). `Some` marks a known machine the operator can act on;
    /// `None` is an unidentified device (phone/IoT), hidden by default in the SPA.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,
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

// ── Drift review (DS-OPS-02) ──────────────────────────────────────────────
//
// Thin `Serialize`-only views over `crate::profiles::drift::DriftReport` /
// `crate::db::ProfileVersionRow` (DS-REG-05). This module does not compute
// drift or select a restore target — see `handlers.rs`'s drift section doc.

/// One row from `GET /api/drift` — a currently-drifted group or profile,
/// mirrored from `crate::profiles::drift::DriftReport`. Aligns with the
/// SPA's existing `MachineRow.consistent` vocabulary: this row exists
/// precisely when that boolean would read `false` for the object named here.
#[derive(Debug, Clone, Serialize)]
pub struct DriftView {
    pub object_kind: String,
    pub object_id: uuid::Uuid,
    /// The object's stored `content_hash`, hex-encoded.
    pub stored_hash: String,
    /// The hash actually computed over the live body, hex-encoded. Differs
    /// from `stored_hash` by definition — that disagreement IS the drift.
    pub actual_hash: String,
    pub seen_count: u32,
}

/// Response of `POST /api/drift/:object_id/accept` and
/// `POST /api/drift/:object_id/revert` — the freshly appended
/// `ProfileVersionRow` DS-REG-05's `accept_drift`/`revert_drift` returned.
#[derive(Debug, Clone, Serialize)]
pub struct ReviewResultView {
    pub object_kind: String,
    pub object_id: uuid::Uuid,
    /// The version number of the newly appended row (the adopted/restored
    /// body), NOT the drift-evidence row `accept_drift`/`revert_drift`
    /// capture immediately before it.
    pub version: i64,
    /// Set ONLY on a revert response. States plainly that this action
    /// restored the stored INTENT, not the deployed machine: v1 has no
    /// re-render, so the host stays exactly as drifted as it was, and
    /// re-deploying it is a separate operator action (spec D11). `None` on
    /// an accept response, which has no such gap to explain — accepting
    /// adopts the machine's current (drifted) body as the new intent, so
    /// intent and machine already agree.
    pub note: Option<String>,
}
