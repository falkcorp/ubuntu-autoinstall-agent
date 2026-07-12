// file: crates/uaa-control/src/db/mod.rs
// version: 1.1.0
// guid: e43975b3-71de-4377-8ea5-ccd77fe75bc6
// last-edited: 2026-07-12

//! Registry data-layer root for uaa-control.
//!
//! Declares the persistence submodules and — critically — the serde row types
//! that mirror every table in `migrations/0001_init.sql`. These types are
//! pre-declared HERE (owned by CT-01) so that no two follower tasks (CT-02..07,
//! PK-01/03, IP-01..04) ever add the same type in disjoint files. Followers add
//! *methods* and query logic in their own modules; they do not redefine these rows.
//!
//! `status` / `state` / `boot_target` are typed enums. Because the parity data is
//! dirty (legacy Python registries contain values that predate this schema), every
//! such enum carries a spelled-out `Unknown(String)` variant that preserves the raw
//! value instead of failing deserialization. See the note on [`BootTarget`] for why
//! this is realized with `#[serde(from/into = "String")]` rather than the literal
//! `rename_all = "kebab-case"` + `#[serde(other)]` (which cannot preserve the string).

pub mod migrations;
pub mod registry;
pub mod store;

use serde::{Deserialize, Serialize};

/// Next-boot intent (spec Decision 13). Serialized as kebab-case; unknown legacy
/// values are preserved verbatim in [`BootTarget::Unknown`].
///
/// Realized with `#[serde(from/into = "String")]` (not `rename_all` +
/// `#[serde(other)]`): serde's `other` attribute only accepts a *unit* variant and
/// discards the offending string, which would silently drop dirty parity data. The
/// manual `String` conversions keep kebab-case serialization AND round-trip unknowns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub enum BootTarget {
    LocalDisk,
    CustomAutoinstall,
    PxeDisabled,
    PxeGrub,
    Unknown(String),
}

impl From<String> for BootTarget {
    fn from(s: String) -> Self {
        match s.as_str() {
            "local-disk" => Self::LocalDisk,
            "custom-autoinstall" => Self::CustomAutoinstall,
            "pxe-disabled" => Self::PxeDisabled,
            "pxe-grub" => Self::PxeGrub,
            _ => Self::Unknown(s),
        }
    }
}

impl From<BootTarget> for String {
    fn from(b: BootTarget) -> Self {
        match b {
            BootTarget::LocalDisk => "local-disk".to_string(),
            BootTarget::CustomAutoinstall => "custom-autoinstall".to_string(),
            BootTarget::PxeDisabled => "pxe-disabled".to_string(),
            BootTarget::PxeGrub => "pxe-grub".to_string(),
            BootTarget::Unknown(s) => s,
        }
    }
}

/// Machine approval lifecycle (`seen|pending|approved|revoked`), Unknown-preserving.
///
/// `Seen` (constellation addition, not present in the Python ground-truth) marks
/// a MAC the machine plane observed on the wire (`/autoinstall/*`) that nobody
/// ever explicitly registered via `/api/register` — distinct from `Pending`,
/// which means a human ran the registration flow and is now awaiting approval.
/// `/api/approve/<mac>` treats both identically (sets `Approved` unconditionally
/// on any existing row), so a `Seen` machine is approvable straight from the
/// dashboard with no registration step required.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub enum MachineStatus {
    Seen,
    Pending,
    Approved,
    Revoked,
    Unknown(String),
}

impl From<String> for MachineStatus {
    fn from(s: String) -> Self {
        match s.as_str() {
            "seen" => Self::Seen,
            "pending" => Self::Pending,
            "approved" => Self::Approved,
            "revoked" => Self::Revoked,
            _ => Self::Unknown(s),
        }
    }
}

impl From<MachineStatus> for String {
    fn from(s: MachineStatus) -> Self {
        match s {
            MachineStatus::Seen => "seen".to_string(),
            MachineStatus::Pending => "pending".to_string(),
            MachineStatus::Approved => "approved".to_string(),
            MachineStatus::Revoked => "revoked".to_string(),
            MachineStatus::Unknown(v) => v,
        }
    }
}

/// Enrollment state machine (`pending|approved|issued|rejected|revoked|superseded`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub enum EnrollmentState {
    Pending,
    Approved,
    Issued,
    Rejected,
    Revoked,
    Superseded,
    Unknown(String),
}

impl From<String> for EnrollmentState {
    fn from(s: String) -> Self {
        match s.as_str() {
            "pending" => Self::Pending,
            "approved" => Self::Approved,
            "issued" => Self::Issued,
            "rejected" => Self::Rejected,
            "revoked" => Self::Revoked,
            "superseded" => Self::Superseded,
            _ => Self::Unknown(s),
        }
    }
}

impl From<EnrollmentState> for String {
    fn from(s: EnrollmentState) -> Self {
        match s {
            EnrollmentState::Pending => "pending".to_string(),
            EnrollmentState::Approved => "approved".to_string(),
            EnrollmentState::Issued => "issued".to_string(),
            EnrollmentState::Rejected => "rejected".to_string(),
            EnrollmentState::Revoked => "revoked".to_string(),
            EnrollmentState::Superseded => "superseded".to_string(),
            EnrollmentState::Unknown(v) => v,
        }
    }
}

/// Saga lifecycle state (spec Decision on approve-SAGA / compensation).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub enum SagaState {
    Running,
    Done,
    Compensating,
    Compensated,
    CompensationPending,
    Unknown(String),
}

impl From<String> for SagaState {
    fn from(s: String) -> Self {
        match s.as_str() {
            "running" => Self::Running,
            "done" => Self::Done,
            "compensating" => Self::Compensating,
            "compensated" => Self::Compensated,
            "compensation_pending" => Self::CompensationPending,
            _ => Self::Unknown(s),
        }
    }
}

impl From<SagaState> for String {
    fn from(s: SagaState) -> Self {
        match s {
            SagaState::Running => "running".to_string(),
            SagaState::Done => "done".to_string(),
            SagaState::Compensating => "compensating".to_string(),
            SagaState::Compensated => "compensated".to_string(),
            SagaState::CompensationPending => "compensation_pending".to_string(),
            SagaState::Unknown(v) => v,
        }
    }
}

/// `machines` table row. Note `r#type` mirrors the SQL `type` column (a Rust
/// keyword); serde serializes the field key as `"type"` without a rename attribute.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MachineRow {
    pub mac: String,
    pub hostname: String,
    pub ip: Option<String>,
    pub r#type: String,
    pub status: MachineStatus,
    pub boot_target: BootTarget,
    pub tpm_ek: Option<String>,
    pub registered_at: Option<String>,
    pub approved_at: Option<String>,
    pub last_seen: Option<String>,
    pub last_ip: Option<String>,
    pub installed_at: Option<String>,
    pub last_install_status: Option<String>,
    pub updated_at: Option<String>,
}

/// `install_history` table row. `event_id` is the WAL-replay dedup key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallEvent {
    pub event_id: uuid::Uuid,
    pub mac: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub status: String,
    pub detail: Option<serde_json::Value>,
}

/// `enrollments` table row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnrollmentRow {
    pub spki_fingerprint: String,
    pub mac: Option<String>,
    pub csr_pem: String,
    pub state: EnrollmentState,
    pub cert_pem: Option<String>,
    pub requested_at: Option<String>,
    pub decided_by: Option<String>,
}

/// `yubikeys` table row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct YubikeyRow {
    pub fingerprint: String,
    pub gpg_pubkey: Option<String>,
    pub ssh_pubkey: Option<String>,
    pub comment: Option<String>,
    pub serial: Option<String>,
    pub status: String,
    pub registered_at: Option<String>,
}

/// `luks_credentials` table row (FIDO2 keyslot tracking).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LuksCredentialRow {
    pub id: uuid::Uuid,
    pub mac: String,
    pub yubikey_serial: String,
    pub role: String,
    pub luks_keyslot: Option<i64>,
    pub enrolled_at: Option<String>,
    pub revoked_at: Option<String>,
}

/// `tang_servers` table row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TangServerRow {
    pub hostname: String,
    pub ip: Option<String>,
    pub tang_url: Option<String>,
    pub adv_keys: Option<serde_json::Value>,
    pub last_seen: Option<String>,
}

/// `discovered_macs` table row (uaa-pxe inbox).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveredMacRow {
    pub mac: String,
    pub first_seen: Option<String>,
    pub last_seen: Option<String>,
    pub arch_hint: Option<String>,
    pub vendor_class: Option<String>,
    pub dismissed: bool,
}

/// `audit_events` table row (hash-chained; spec Decision 21).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEventRow {
    pub seq: i64,
    pub at: Option<String>,
    pub actor: String,
    pub role: String,
    pub action: String,
    pub target: Option<String>,
    pub outcome: String,
    pub detail: Option<serde_json::Value>,
    #[serde(with = "serde_bytes_hex")]
    pub prev_hash: Vec<u8>,
    #[serde(with = "serde_bytes_hex")]
    pub hash: Vec<u8>,
}

/// `audit_checkpoints` table row (daily ed25519-signed tip).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditCheckpointRow {
    pub day: String,
    pub tip_seq: i64,
    #[serde(with = "serde_bytes_hex")]
    pub tip_hash: Vec<u8>,
    #[serde(with = "serde_bytes_hex")]
    pub signature: Vec<u8>,
}

/// `saga_log` table row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SagaRow {
    pub saga_id: uuid::Uuid,
    pub kind: String,
    pub state: SagaState,
    pub steps: serde_json::Value,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
}

/// Hex codec for `BYTES` columns so audit hashes survive the JSON snapshot/WAL
/// round-trip as lowercase hex strings (CRDB stores raw bytes at runtime).
mod serde_bytes_hex {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        serializer.serialize_str(&hex)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(deserializer)?;
        if s.len() % 2 != 0 {
            return Err(serde::de::Error::custom("odd-length hex string"));
        }
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(serde::de::Error::custom))
            .collect()
    }
}
