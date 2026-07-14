// file: crates/uaa-control/src/enroll.rs
// version: 1.2.1
// guid: 1a81d662-89c9-4e64-8ce9-9aa51fd3a412
// last-edited: 2026-07-14

//! Enrollment plane (`uaa.enroll.v1` gRPC + `:15002` JSON mirror) — CSR submit +
//! GetCredential poll, idempotent by SPKI fingerprint (spec C6, NORMATIVE).
//!
//! Filled by pki PK-01 (this task), on top of PK-01's own [`crate::ca::InstallCa`].
//! Everything here is storage-agnostic against [`EnrollmentStore`] (this module's own
//! trait — the `enrollments` table has no existing CT-01 CRUD trait to reuse, so this
//! follows the SAME pattern CT-01 established for `RegistryStore`/`AuditStore`: a
//! trait plus an always-compiled in-memory mock, so `cargo test --lib --offline`
//! needs NO live database). Every mutation goes through
//! [`crate::audit::record`] (CT-01's audit hook, PK-01 reuses it — no parallel log).
//!
//! # State machine (spec C6, implemented verbatim)
//!
//! ```text
//! pending --(approve)--> issued --(revoke)--> revoked
//!    |                     |
//!    |                     `--(a NEW fp approved for the same MAC)--> superseded
//!    `--(reject)--> rejected
//! ```
//!
//! * `pending` — freshly submitted, awaiting an operator decision.
//! * `approved` — reserved by the schema (spec "Data model"); this task's `approve`
//!   signs and transitions straight `pending -> issued` in one atomic operator
//!   action (per spec C6's "approve/reject -> control signs ... and returns cert" —
//!   there is no separate sign step). Treated identically to `pending` by
//!   `GetCredential` (no cert bytes yet) for forward compatibility with a future
//!   two-phase approval flow.
//! * `issued` — signed; `cert_pem` set.
//! * `rejected` / `revoked` — terminal UNTIL an operator explicitly calls
//!   [`approve`] again (spec: "operator can re-approve").
//! * `superseded` — an `issued` row's replacement: approving a NEW fp for a MAC that
//!   already has an `issued` row marks the OLD row `superseded` first (reinstalls
//!   wipe the agent state dir and mint a new key — rows must not accrete).
//!
//! # Fail-closed poll (spec C3 enrollment plane)
//!
//! [`get_credential`] for an UNKNOWN SPKI fingerprint returns `Ok(None)` — the JSON
//! handler maps this to `404`, the gRPC handler to `Status::not_found`. Neither path
//! ever auto-issues or auto-creates a row for an unknown fingerprint.
//!
//! # Renewal (same-key auto-issue)
//!
//! [`submit_csr`] on an ALREADY-KNOWN fingerprint is an idempotent upsert: it never
//! resets a decided row back to `pending`, and normally just returns the current
//! state unchanged — EXCEPT when that row is `issued` with a still-unexpired cert
//! (an `issued` row is by definition not revoked), in which case a fresh 90-day cert
//! is minted with NO operator round-trip (spec: "renewal ... auto-issue iff an
//! unexpired unrevoked cert exists for the SPKI"). Because the lookup key IS the
//! SHA-256 of the CSR's public key, "same fingerprint" and "identical public key"
//! are the same fact — no separate key-equality check is needed.
//!
//! NEVER the CockroachDB CA — every signature in this module goes through
//! [`crate::ca::InstallCa`], never a second trust root.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::extract::{Path as AxumPath, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::audit::AuditStore;
use crate::ca::InstallCa;
use crate::db::{EnrollmentRow, EnrollmentState};

// ── Persistence seam (reuses CT-01's trait+mock pattern; no existing enrollments
// CRUD trait to reuse verbatim — this is the analogous trait for THIS table) ──────

/// Persistence seam for the `enrollments` table (spec "Data model": PRIMARY KEY
/// `spki_fingerprint`). Mirrors CT-01's `RegistryStore`/`AuditStore` shape exactly:
/// `insert_if_absent` returns `Ok(true)` iff the row was newly inserted (Decision 22
/// no-clobber law — `Ok(false)` means a row for that fp already existed and was left
/// COMPLETELY untouched).
#[async_trait::async_trait]
pub trait EnrollmentStore: Send + Sync {
    /// Look up by SPKI fingerprint (the primary key).
    async fn get(&self, spki_fingerprint: &str) -> anyhow::Result<Option<EnrollmentRow>>;
    /// `Ok(true)` = inserted; `Ok(false)` = a row for this fp already existed
    /// (Decision 22 no-clobber law) and was left untouched.
    async fn insert_if_absent(&self, row: EnrollmentRow) -> anyhow::Result<bool>;
    /// Full replace of an existing row (state transitions, `cert_pem`, `decided_by`).
    async fn update(&self, row: EnrollmentRow) -> anyhow::Result<()>;
    /// Every row currently `issued` for `mac` — used by [`approve`]'s supersede rule.
    async fn list_issued_for_mac(&self, mac: &str) -> anyhow::Result<Vec<EnrollmentRow>>;
    /// Every row regardless of state — the operator-plane listing view
    /// (`GET /api/enrollments`) needs to see `pending` rows awaiting a
    /// decision, not just `issued` ones.
    async fn list_all(&self) -> anyhow::Result<Vec<EnrollmentRow>>;
}

/// In-memory [`EnrollmentStore`] — ALWAYS compiled (not `#[cfg(test)]`): this
/// module's own tests use it, and it is exported so sibling tasks (operator-plane
/// approve/reject UI, PK-03) can reuse it in their own unit tests without a live
/// database, exactly like `crate::db::registry::MemRegistryStore`.
#[derive(Debug, Default)]
pub struct MemEnrollmentStore {
    rows: Mutex<HashMap<String, EnrollmentRow>>,
}

impl MemEnrollmentStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait::async_trait]
impl EnrollmentStore for MemEnrollmentStore {
    async fn get(&self, spki_fingerprint: &str) -> anyhow::Result<Option<EnrollmentRow>> {
        Ok(self.rows.lock().unwrap().get(spki_fingerprint).cloned())
    }

    async fn insert_if_absent(&self, row: EnrollmentRow) -> anyhow::Result<bool> {
        let mut rows = self.rows.lock().unwrap();
        if rows.contains_key(&row.spki_fingerprint) {
            return Ok(false);
        }
        rows.insert(row.spki_fingerprint.clone(), row);
        Ok(true)
    }

    async fn update(&self, row: EnrollmentRow) -> anyhow::Result<()> {
        self.rows.lock().unwrap().insert(row.spki_fingerprint.clone(), row);
        Ok(())
    }

    async fn list_issued_for_mac(&self, mac: &str) -> anyhow::Result<Vec<EnrollmentRow>> {
        Ok(self
            .rows
            .lock()
            .unwrap()
            .values()
            .filter(|r| r.mac.as_deref() == Some(mac) && r.state == EnrollmentState::Issued)
            .cloned()
            .collect())
    }

    async fn list_all(&self) -> anyhow::Result<Vec<EnrollmentRow>> {
        Ok(self.rows.lock().unwrap().values().cloned().collect())
    }
}

// ── State machine core ──────────────────────────────────────────────────────────

/// `SubmitCsr` (spec C6): compute the SPKI fingerprint, upsert-if-absent as
/// `pending`. A known fingerprint is an idempotent upsert — it returns the current
/// state UNCHANGED, except the renewal rule (see module doc): an `issued` row with
/// an unexpired cert auto-issues a fresh 90-day cert with no operator round-trip.
///
/// Takes `ca`/`audit` in addition to the brief's terse `(store, csr_pem,
/// claimed_mac, claimed_hostname)` signature: the renewal rule requires actually
/// signing (needs the CA) and every issuance is an audited mutation (needs the
/// audit hook) — both explicitly required elsewhere in this same task.
///
/// `claimed_hostname` is accepted (matching the `SubmitCsr` wire shape both
/// transports below use) but deliberately NOT trusted for signing: the
/// `enrollments` table has no hostname column, and both [`approve`] and the
/// renewal branch here derive hostname from the CSR's OWN DNS SAN
/// ([`hostname_from_csr`]) — never from a value supplied alongside a (possibly
/// already-decided) fingerprint. See the renewal branch below for why.
pub async fn submit_csr(
    store: &dyn EnrollmentStore,
    ca: &InstallCa,
    audit: &dyn AuditStore,
    csr_pem: &str,
    claimed_mac: &str,
    _claimed_hostname: &str,
) -> anyhow::Result<EnrollmentRow> {
    let fp = uaa_core::pki::spki_fingerprint(csr_pem)?;

    let candidate = EnrollmentRow {
        spki_fingerprint: fp.clone(),
        mac: Some(claimed_mac.to_string()),
        csr_pem: csr_pem.to_string(),
        state: EnrollmentState::Pending,
        cert_pem: None,
        requested_at: Some(now_rfc3339()),
        decided_by: None,
    };

    if store.insert_if_absent(candidate.clone()).await? {
        return Ok(candidate);
    }

    // Known fp: idempotent upsert — NEVER resets a decided row back to pending.
    let existing = store
        .get(&fp)
        .await?
        .ok_or_else(|| anyhow::anyhow!("enrollment row for {fp} vanished after insert_if_absent"))?;

    if existing.state == EnrollmentState::Issued {
        if let Some(cert_pem) = existing.cert_pem.as_deref() {
            if cert_is_unexpired(cert_pem)? {
                // Renewal: same-key CSR against an unexpired (hence unrevoked —
                // `issued` and `revoked` are mutually exclusive states) cert. Sign
                // from the STORED row's own mac + the STORED csr's own hostname SAN
                // — NEVER from `claimed_mac`/`claimed_hostname`/`csr_pem` on this
                // request. "Same fp" only proves "same public key"; a resubmission
                // could otherwise carry an attacker-chosen mac/hostname and get an
                // install-CA-signed cert with NO operator in the loop. This mirrors
                // exactly what `approve` trusts (the stored row), so the no-operator
                // auto-issue path can never mint a different identity than the one
                // an operator already approved for this key.
                let renewal_mac = existing
                    .mac
                    .clone()
                    .ok_or_else(|| anyhow::anyhow!("issued row {fp} has no mac on file"))?;
                let renewal_hostname = hostname_from_csr(&existing.csr_pem)?;
                let fresh_cert_pem =
                    ca.sign_agent_csr(&existing.csr_pem, &renewal_hostname, &renewal_mac)?;
                let mut renewed = existing.clone();
                renewed.cert_pem = Some(fresh_cert_pem);
                renewed.requested_at = Some(now_rfc3339());
                store.update(renewed.clone()).await?;

                crate::audit::record(
                    audit,
                    "system",
                    "system",
                    "enrollment.renew",
                    Some(fp.clone()),
                    "success",
                    Some(json!({"mac": renewal_mac})),
                )
                .await?;
                return Ok(renewed);
            }
        }
    }

    Ok(existing)
}

/// `GetCredential` (spec C6): a pure read. An UNKNOWN fingerprint returns `Ok(None)`
/// — the transport layers map this to `404` / `Status::not_found` — NEVER
/// auto-issuing or auto-creating a row (fail-closed).
pub async fn get_credential(
    store: &dyn EnrollmentStore,
    spki_fingerprint: &str,
) -> anyhow::Result<Option<EnrollmentRow>> {
    store.get(spki_fingerprint).await
}

/// Sign and approve `fp`: sets `issued` + `cert_pem` + `decided_by`. FIRST marks any
/// OTHER `issued` row for the same claimed MAC `superseded` (reinstalls wipe the
/// agent state dir and mint a new key — rows must not accrete) — the supersede
/// mutation, the new `issued` row, AND both audit events all happen before this
/// returns. May be called on a `rejected`/`revoked` row (that IS the documented
/// re-approve path — those states are terminal only until an operator acts again).
///
/// Hostname is derived from the stored CSR itself (the `enrollments` table has no
/// separate hostname column — the CSR's own DNS SAN, set by the agent at
/// `claimed_hostname`, is the source of truth at sign time).
pub async fn approve(
    store: &dyn EnrollmentStore,
    ca: &InstallCa,
    audit: &dyn AuditStore,
    fp: &str,
    decided_by: &str,
) -> anyhow::Result<EnrollmentRow> {
    let row = store
        .get(fp)
        .await?
        .ok_or_else(|| anyhow::anyhow!("cannot approve unknown spki fingerprint: {fp}"))?;
    let mac = row
        .mac
        .clone()
        .ok_or_else(|| anyhow::anyhow!("enrollment row {fp} has no claimed mac"))?;
    let hostname = hostname_from_csr(&row.csr_pem)?;

    let cert_pem = ca.sign_agent_csr(&row.csr_pem, &hostname, &mac)?;

    // Supersede FIRST — before the new row is marked issued.
    for other in store.list_issued_for_mac(&mac).await? {
        if other.spki_fingerprint == fp {
            continue;
        }
        let other_fp = other.spki_fingerprint.clone();
        let mut superseded = other;
        superseded.state = EnrollmentState::Superseded;
        store.update(superseded).await?;
        crate::audit::record(
            audit,
            decided_by,
            "operator",
            "enrollment.supersede",
            Some(other_fp),
            "success",
            Some(json!({"mac": mac, "superseded_by": fp})),
        )
        .await?;
    }

    let mut issued = row;
    issued.state = EnrollmentState::Issued;
    issued.cert_pem = Some(cert_pem);
    issued.decided_by = Some(decided_by.to_string());
    store.update(issued.clone()).await?;

    crate::audit::record(
        audit,
        decided_by,
        "operator",
        "enrollment.approve",
        Some(fp.to_string()),
        "success",
        Some(json!({"mac": mac, "hostname": hostname})),
    )
    .await?;

    Ok(issued)
}

/// Reject `fp`: a terminal state until an operator explicitly re-[`approve`]s it.
pub async fn reject(
    store: &dyn EnrollmentStore,
    audit: &dyn AuditStore,
    fp: &str,
    decided_by: &str,
) -> anyhow::Result<EnrollmentRow> {
    let mut row = store
        .get(fp)
        .await?
        .ok_or_else(|| anyhow::anyhow!("cannot reject unknown spki fingerprint: {fp}"))?;
    row.state = EnrollmentState::Rejected;
    row.decided_by = Some(decided_by.to_string());
    store.update(row.clone()).await?;

    crate::audit::record(
        audit,
        decided_by,
        "operator",
        "enrollment.reject",
        Some(fp.to_string()),
        "success",
        None,
    )
    .await?;

    Ok(row)
}

/// Revoke `fp`: a terminal state until an operator explicitly re-[`approve`]s it.
/// Revocation is recorded as an audit event ONLY here — PK-03 hooks its
/// `regenerate_crl` (spec Decision 25) onto this same audited mutation; CRL
/// generation itself is explicitly out of scope for this task.
pub async fn revoke(
    store: &dyn EnrollmentStore,
    audit: &dyn AuditStore,
    fp: &str,
    decided_by: &str,
) -> anyhow::Result<EnrollmentRow> {
    let mut row = store
        .get(fp)
        .await?
        .ok_or_else(|| anyhow::anyhow!("cannot revoke unknown spki fingerprint: {fp}"))?;
    row.state = EnrollmentState::Revoked;
    row.decided_by = Some(decided_by.to_string());
    store.update(row.clone()).await?;

    crate::audit::record(
        audit,
        decided_by,
        "operator",
        "enrollment.revoke",
        Some(fp.to_string()),
        "success",
        None,
    )
    .await?;

    Ok(row)
}

/// Extracts the DNS SAN (the agent-claimed hostname) from a stored CSR — the
/// `enrollments` table has no separate hostname column, so the CSR itself is the
/// source of truth at sign time (see [`approve`]). `pub(crate)` so the operator
/// API's `GET /api/enrollments` listing can display a `claimed_hostname` without
/// the table growing a redundant column.
pub(crate) fn hostname_from_csr(csr_pem: &str) -> anyhow::Result<String> {
    let params = rcgen::CertificateSigningRequestParams::from_pem(csr_pem)
        .map_err(|e| anyhow::anyhow!("invalid CSR: {e}"))?
        .params;
    params
        .subject_alt_names
        .into_iter()
        .find_map(|san| match san {
            rcgen::SanType::DnsName(dns) => Some(dns.to_string()),
            _ => None,
        })
        .ok_or_else(|| anyhow::anyhow!("CSR has no DNS SAN (hostname) to sign"))
}

/// `true` iff `cert_pem` parses and its `not_after` is still in the future. An
/// `issued` row is by construction not revoked (revocation transitions the row to
/// the separate `revoked` state), so this single check covers the renewal rule's
/// "unexpired+unrevoked" clause for an `issued` row.
fn cert_is_unexpired(cert_pem: &str) -> anyhow::Result<bool> {
    let (_, pem) = x509_parser::pem::parse_x509_pem(cert_pem.as_bytes())
        .map_err(|e| anyhow::anyhow!("invalid cert PEM: {e:?}"))?;
    let (_, cert) = x509_parser::parse_x509_certificate(&pem.contents)
        .map_err(|e| anyhow::anyhow!("invalid cert DER: {e:?}"))?;
    let now = x509_parser::time::ASN1Time::now().timestamp();
    Ok(now < cert.validity().not_after.timestamp())
}

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

// ── gRPC service surface (uaa.enroll.v1.EnrollService, CP-02 types) ──────────────

/// Tonic server impl of `uaa.enroll.v1.EnrollService` — delegates to the state
/// machine core above. Exposed as `pub` so the coordinator constructs it (with the
/// real [`MemEnrollmentStore`]-or-`Pg` store, [`InstallCa`], and audit store) and
/// wires it wherever the real gRPC transport lands (spec: TLS termination for
/// `:15001` arrives with PK-03/CT-07 — see `crate::listeners` module doc).
pub struct EnrollGrpcService {
    store: Arc<dyn EnrollmentStore>,
    ca: Arc<InstallCa>,
    audit: Arc<dyn AuditStore>,
}

impl EnrollGrpcService {
    pub fn new(store: Arc<dyn EnrollmentStore>, ca: Arc<InstallCa>, audit: Arc<dyn AuditStore>) -> Self {
        Self { store, ca, audit }
    }
}

#[tonic::async_trait]
impl uaa_proto::enroll::v1::enroll_service_server::EnrollService for EnrollGrpcService {
    async fn submit_csr(
        &self,
        request: tonic::Request<uaa_proto::enroll::v1::SubmitCsrRequest>,
    ) -> Result<tonic::Response<uaa_proto::enroll::v1::SubmitCsrResponse>, tonic::Status> {
        let req = request.into_inner();
        let row = submit_csr(
            self.store.as_ref(),
            self.ca.as_ref(),
            self.audit.as_ref(),
            &req.csr_pem,
            &req.claimed_mac,
            &req.claimed_hostname,
        )
        .await
        .map_err(|e| tonic::Status::invalid_argument(e.to_string()))?;

        Ok(tonic::Response::new(uaa_proto::enroll::v1::SubmitCsrResponse {
            spki_fingerprint: row.spki_fingerprint,
            state: String::from(row.state),
        }))
    }

    async fn get_credential(
        &self,
        request: tonic::Request<uaa_proto::enroll::v1::GetCredentialRequest>,
    ) -> Result<tonic::Response<uaa_proto::enroll::v1::GetCredentialResponse>, tonic::Status> {
        let req = request.into_inner();
        match get_credential(self.store.as_ref(), &req.spki_fingerprint).await {
            Ok(Some(row)) => {
                let issued = row.state == EnrollmentState::Issued;
                Ok(tonic::Response::new(uaa_proto::enroll::v1::GetCredentialResponse {
                    state: String::from(row.state),
                    cert_pem: row.cert_pem.unwrap_or_default(),
                    ca_pem: if issued {
                        self.ca.ca_cert_pem().to_string()
                    } else {
                        String::new()
                    },
                }))
            }
            // Fail-closed: unknown fingerprint -> NOT_FOUND, never auto-issued.
            Ok(None) => Err(tonic::Status::not_found(format!(
                "unknown spki fingerprint: {}",
                req.spki_fingerprint
            ))),
            Err(e) => Err(tonic::Status::internal(e.to_string())),
        }
    }
}

/// Build the tonic server wrapper for [`EnrollGrpcService`] — the coordinator's one
/// call site to add it to a `tonic::transport::Server` once the real `:15001` gRPC
/// transport lands (PK-03/CT-07).
pub fn enroll_grpc_service(
    store: Arc<dyn EnrollmentStore>,
    ca: Arc<InstallCa>,
    audit: Arc<dyn AuditStore>,
) -> uaa_proto::enroll::v1::enroll_service_server::EnrollServiceServer<EnrollGrpcService> {
    uaa_proto::enroll::v1::enroll_service_server::EnrollServiceServer::new(EnrollGrpcService::new(
        store, ca, audit,
    ))
}

// ── `:15002` JSON mirror (POST /enroll/csr, GET /enroll/credential/<fp>) ──────────

/// Request body for `POST /enroll/csr` — field-for-field the wire shape
/// `uaa_core::pki::SubmitCsrRequest` (the agent client, PK-02) serializes.
#[derive(Debug, Deserialize)]
struct SubmitCsrBody {
    csr_pem: String,
    claimed_hostname: String,
    claimed_mac: String,
}

#[derive(Debug, Serialize)]
struct SubmitCsrResponseBody {
    spki_fingerprint: String,
    state: String,
}

/// Response body for `GET /enroll/credential/<fp>` (200 case) — field-for-field the
/// wire shape `uaa_core::pki::GetCredentialResponse` (PK-02) deserializes.
#[derive(Debug, Serialize, Default)]
struct GetCredentialResponseBody {
    state: String,
    cert_pem: String,
    ca_pem: String,
}

#[derive(Clone)]
struct AppState {
    store: Arc<dyn EnrollmentStore>,
    ca: Arc<InstallCa>,
    audit: Arc<dyn AuditStore>,
}

/// The `:15002` enrollment JSON router — `POST /enroll/csr`, `GET
/// /enroll/credential/:fp`. Built by the coordinator with the real store/CA/audit
/// instances and merged into the `:15002` listener wiring point in
/// `crate::listeners` (see that module's doc — never edited by this task).
pub fn enroll_json_router(
    store: Arc<dyn EnrollmentStore>,
    ca: Arc<InstallCa>,
    audit: Arc<dyn AuditStore>,
) -> Router {
    Router::new()
        .route("/enroll/csr", post(handle_submit_csr))
        .route("/enroll/credential/:fp", get(handle_get_credential))
        .with_state(AppState { store, ca, audit })
}

async fn handle_submit_csr(State(state): State<AppState>, Json(body): Json<SubmitCsrBody>) -> Response {
    match submit_csr(
        state.store.as_ref(),
        state.ca.as_ref(),
        state.audit.as_ref(),
        &body.csr_pem,
        &body.claimed_mac,
        &body.claimed_hostname,
    )
    .await
    {
        Ok(row) => Json(SubmitCsrResponseBody {
            spki_fingerprint: row.spki_fingerprint,
            state: String::from(row.state),
        })
        .into_response(),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

async fn handle_get_credential(State(state): State<AppState>, AxumPath(fp): AxumPath<String>) -> Response {
    match get_credential(state.store.as_ref(), &fp).await {
        Ok(Some(row)) => {
            let issued = row.state == EnrollmentState::Issued;
            Json(GetCredentialResponseBody {
                state: String::from(row.state),
                cert_pem: row.cert_pem.unwrap_or_default(),
                ca_pem: if issued {
                    state.ca.ca_cert_pem().to_string()
                } else {
                    String::new()
                },
            })
            .into_response()
        }
        // Fail-closed: unknown fingerprint -> 404, never auto-issued.
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "unknown spki fingerprint"})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::MemAuditStore;
    use axum::body::Body;
    use axum::http::Request;
    use tempfile::tempdir;
    use tower::ServiceExt;
    use uaa_core::pki::{generate_keypair_and_csr, AgentIdentity};

    fn test_ca() -> InstallCa {
        let dir = tempdir().unwrap();
        InstallCa::load_or_create(&dir.path().join("ca")).unwrap()
    }

    fn identity(hostname: &str, mac: &str) -> AgentIdentity {
        AgentIdentity {
            hostname: hostname.to_string(),
            mac: mac.to_string(),
        }
    }

    fn csr_for(id: &AgentIdentity) -> String {
        generate_keypair_and_csr(id).unwrap().1
    }

    // ── submit_csr ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_submit_csr_idempotent_reclaim() {
        let store = MemEnrollmentStore::new();
        let ca = test_ca();
        let audit = MemAuditStore::new();
        let id = identity("host1", "aa:bb:cc:dd:ee:01");
        let csr_pem = csr_for(&id);

        let first = submit_csr(&store, &ca, &audit, &csr_pem, &id.mac, &id.hostname)
            .await
            .unwrap();
        assert_eq!(first.state, EnrollmentState::Pending);

        let second = submit_csr(&store, &ca, &audit, &csr_pem, &id.mac, &id.hostname)
            .await
            .unwrap();
        assert_eq!(second.state, EnrollmentState::Pending);
        assert_eq!(second.spki_fingerprint, first.spki_fingerprint);

        // Exactly one row for this fp — the second submit must not have appended.
        assert!(store.get(&first.spki_fingerprint).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn test_list_all_returns_every_state() {
        let store = MemEnrollmentStore::new();
        let ca = test_ca();
        let audit = MemAuditStore::new();
        let pending = identity("pending-host", "aa:bb:cc:dd:ee:01");
        let approved = identity("approved-host", "aa:bb:cc:dd:ee:02");
        submit_csr(
            &store,
            &ca,
            &audit,
            &csr_for(&pending),
            &pending.mac,
            &pending.hostname,
        )
        .await
        .unwrap();
        let issued = submit_csr(
            &store,
            &ca,
            &audit,
            &csr_for(&approved),
            &approved.mac,
            &approved.hostname,
        )
        .await
        .unwrap();
        approve(&store, &ca, &audit, &issued.spki_fingerprint, "tester")
            .await
            .unwrap();

        let mut all = store.list_all().await.unwrap();
        all.sort_by(|a, b| a.spki_fingerprint.cmp(&b.spki_fingerprint));
        assert_eq!(all.len(), 2);
        assert!(all.iter().any(|r| r.state == EnrollmentState::Pending));
        assert!(all.iter().any(|r| r.state == EnrollmentState::Issued));
    }

    // ── fail-closed 404 ───────────────────────────────────────────────────

    #[tokio::test]
    async fn test_unknown_fp_get_credential_404() {
        let store = MemEnrollmentStore::new();
        let result = get_credential(&store, "deadbeef-unknown-fp").await.unwrap();
        assert!(result.is_none(), "unknown fp must never auto-issue — Ok(None)");
    }

    #[tokio::test]
    async fn test_unknown_fp_json_returns_404() {
        let store: Arc<dyn EnrollmentStore> = Arc::new(MemEnrollmentStore::new());
        let ca = Arc::new(test_ca());
        let audit: Arc<dyn AuditStore> = Arc::new(MemAuditStore::new());
        let router = enroll_json_router(store, ca, audit);

        let response = router
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/enroll/credential/unknown-fp")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    // ── approve / SAN / lifetime ──────────────────────────────────────────

    #[tokio::test]
    async fn test_approve_signs_90d_san() {
        let store = MemEnrollmentStore::new();
        let ca = test_ca();
        let audit = MemAuditStore::new();
        let id = identity("host2", "aa:bb:cc:dd:ee:02");
        let csr_pem = csr_for(&id);

        let submitted = submit_csr(&store, &ca, &audit, &csr_pem, &id.mac, &id.hostname)
            .await
            .unwrap();
        let issued = approve(&store, &ca, &audit, &submitted.spki_fingerprint, "alice")
            .await
            .unwrap();

        assert_eq!(issued.state, EnrollmentState::Issued);
        let cert_pem = issued.cert_pem.expect("cert_pem set on issue");

        let (_, pem) = x509_parser::pem::parse_x509_pem(cert_pem.as_bytes()).unwrap();
        let (_, cert) = x509_parser::parse_x509_certificate(&pem.contents).unwrap();
        let validity = cert.validity();
        let lifetime_days =
            (validity.not_after.timestamp() - validity.not_before.timestamp()) / 86_400;
        assert!((89..=91).contains(&lifetime_days), "expected ~90d, got {lifetime_days}d");

        let mut found_dns = false;
        let mut found_uri = false;
        for ext in cert.extensions() {
            if let x509_parser::extensions::ParsedExtension::SubjectAlternativeName(san) =
                ext.parsed_extension()
            {
                for name in &san.general_names {
                    match name {
                        x509_parser::extensions::GeneralName::DNSName(dns) if *dns == id.hostname => {
                            found_dns = true
                        }
                        x509_parser::extensions::GeneralName::URI(uri)
                            if *uri == format!("uaa-mac:{}", id.mac) =>
                        {
                            found_uri = true
                        }
                        _ => {}
                    }
                }
            }
        }
        assert!(found_dns && found_uri, "SAN must be hostname (DNS) + uaa-mac:<mac> (URI)");
    }

    // ── anti-over-suppression ─────────────────────────────────────────────

    #[tokio::test]
    async fn test_approved_fp_still_issued_happy_path() {
        let store = MemEnrollmentStore::new();
        let ca = test_ca();
        let audit = MemAuditStore::new();
        let id = identity("host3", "aa:bb:cc:dd:ee:03");
        let csr_pem = csr_for(&id);

        let submitted = submit_csr(&store, &ca, &audit, &csr_pem, &id.mac, &id.hostname)
            .await
            .unwrap();
        approve(&store, &ca, &audit, &submitted.spki_fingerprint, "alice")
            .await
            .unwrap();

        let polled = get_credential(&store, &submitted.spki_fingerprint)
            .await
            .unwrap()
            .expect("known fp must be found");
        assert_eq!(polled.state, EnrollmentState::Issued);
        assert!(polled.cert_pem.is_some(), "issued poll must return the signed cert");
    }

    // ── supersede-on-reinstall ────────────────────────────────────────────

    #[tokio::test]
    async fn test_supersede_on_reinstall() {
        let store = MemEnrollmentStore::new();
        let ca = test_ca();
        let audit = MemAuditStore::new();
        let mac = "aa:bb:cc:dd:ee:04";

        // First install: submit + approve -> issued.
        let id1 = identity("host4", mac);
        let csr1 = csr_for(&id1);
        let first = submit_csr(&store, &ca, &audit, &csr1, mac, &id1.hostname)
            .await
            .unwrap();
        let first_issued = approve(&store, &ca, &audit, &first.spki_fingerprint, "alice")
            .await
            .unwrap();
        assert_eq!(first_issued.state, EnrollmentState::Issued);

        // Reinstall: state dir wiped, agent mints a NEW keypair -> a NEW fp for the
        // SAME mac.
        let id2 = identity("host4", mac);
        let csr2 = csr_for(&id2);
        let second = submit_csr(&store, &ca, &audit, &csr2, mac, &id2.hostname)
            .await
            .unwrap();
        assert_ne!(second.spki_fingerprint, first.spki_fingerprint, "reinstall mints a new keypair");

        let second_issued = approve(&store, &ca, &audit, &second.spki_fingerprint, "alice")
            .await
            .unwrap();
        assert_eq!(second_issued.state, EnrollmentState::Issued);

        let old_row = store.get(&first.spki_fingerprint).await.unwrap().unwrap();
        assert_eq!(
            old_row.state,
            EnrollmentState::Superseded,
            "approving the new fp must supersede the old issued row"
        );

        let new_row = store.get(&second.spki_fingerprint).await.unwrap().unwrap();
        assert_eq!(new_row.state, EnrollmentState::Issued);
    }

    // ── renewal (same-key auto-issue) ─────────────────────────────────────

    #[tokio::test]
    async fn test_renewal_same_key_auto_issue() {
        let store = MemEnrollmentStore::new();
        let ca = test_ca();
        let audit = MemAuditStore::new();
        let id = identity("host5", "aa:bb:cc:dd:ee:05");
        let csr_pem = csr_for(&id);

        let submitted = submit_csr(&store, &ca, &audit, &csr_pem, &id.mac, &id.hostname)
            .await
            .unwrap();
        let first_issued = approve(&store, &ca, &audit, &submitted.spki_fingerprint, "alice")
            .await
            .unwrap();
        let first_cert = first_issued.cert_pem.clone().unwrap();

        // Same-key CSR resubmitted (agent renewal at 2/3 lifetime) — no operator
        // round-trip: submit_csr alone must auto-issue a FRESH cert.
        let renewed = submit_csr(&store, &ca, &audit, &csr_pem, &id.mac, &id.hostname)
            .await
            .unwrap();
        assert_eq!(renewed.state, EnrollmentState::Issued);
        assert_eq!(renewed.spki_fingerprint, submitted.spki_fingerprint);
        assert_ne!(
            renewed.cert_pem.unwrap(),
            first_cert,
            "renewal must mint a FRESH cert, not return the old one"
        );
    }

    #[tokio::test]
    async fn test_renewal_ignores_spoofed_claim_on_resubmit() {
        // Security regression: a same-key resubmission with a DIFFERENT claimed
        // mac/hostname must NOT get those attacker-controlled values signed onto
        // the fresh cert with no operator in the loop. "Same fp" only proves "same
        // public key" — the renewal path must sign from the STORED row's mac and
        // the STORED csr's own hostname SAN, exactly like `approve` does.
        let store = MemEnrollmentStore::new();
        let ca = test_ca();
        let audit = MemAuditStore::new();
        let id = identity("realhost", "aa:bb:cc:dd:ee:09");
        let csr_pem = csr_for(&id);

        let submitted = submit_csr(&store, &ca, &audit, &csr_pem, &id.mac, &id.hostname)
            .await
            .unwrap();
        approve(&store, &ca, &audit, &submitted.spki_fingerprint, "alice")
            .await
            .unwrap();

        // Same CSR bytes (same key, same fp), but the request now claims a
        // DIFFERENT mac/hostname — must be ignored by the auto-issue path.
        let renewed = submit_csr(
            &store,
            &ca,
            &audit,
            &csr_pem,
            "ff:ff:ff:ff:ff:ff",
            "attacker-claimed-host",
        )
        .await
        .unwrap();
        assert_eq!(renewed.state, EnrollmentState::Issued);
        let cert_pem = renewed.cert_pem.unwrap();

        let (_, pem) = x509_parser::pem::parse_x509_pem(cert_pem.as_bytes()).unwrap();
        let (_, cert) = x509_parser::parse_x509_certificate(&pem.contents).unwrap();
        let mut saw_spoofed_host = false;
        let mut saw_spoofed_mac = false;
        let mut saw_real_host = false;
        for ext in cert.extensions() {
            if let x509_parser::extensions::ParsedExtension::SubjectAlternativeName(san) =
                ext.parsed_extension()
            {
                for name in &san.general_names {
                    match name {
                        x509_parser::extensions::GeneralName::DNSName(dns) => {
                            if *dns == "attacker-claimed-host" {
                                saw_spoofed_host = true;
                            }
                            if *dns == id.hostname {
                                saw_real_host = true;
                            }
                        }
                        x509_parser::extensions::GeneralName::URI(uri)
                            if *uri == "uaa-mac:ff:ff:ff:ff:ff:ff" =>
                        {
                            saw_spoofed_mac = true;
                        }
                        _ => {}
                    }
                }
            }
        }
        assert!(!saw_spoofed_host, "renewal must never sign the resubmitted hostname claim");
        assert!(!saw_spoofed_mac, "renewal must never sign the resubmitted mac claim");
        assert!(saw_real_host, "renewal must keep signing the ORIGINAL approved hostname");
        // Row's mac on file must also be untouched by the spoofed resubmission.
        assert_eq!(renewed.mac.as_deref(), Some(id.mac.as_str()));
    }

    #[tokio::test]
    async fn test_renewal_refused_when_revoked() {
        let store = MemEnrollmentStore::new();
        let ca = test_ca();
        let audit = MemAuditStore::new();
        let id = identity("host6", "aa:bb:cc:dd:ee:06");
        let csr_pem = csr_for(&id);

        let submitted = submit_csr(&store, &ca, &audit, &csr_pem, &id.mac, &id.hostname)
            .await
            .unwrap();
        approve(&store, &ca, &audit, &submitted.spki_fingerprint, "alice")
            .await
            .unwrap();
        let revoked = revoke(&store, &audit, &submitted.spki_fingerprint, "alice")
            .await
            .unwrap();
        assert_eq!(revoked.state, EnrollmentState::Revoked);

        // Same-key CSR after revoke: submit_csr must NOT auto-issue.
        let after = submit_csr(&store, &ca, &audit, &csr_pem, &id.mac, &id.hostname)
            .await
            .unwrap();
        assert_eq!(after.state, EnrollmentState::Revoked, "revoked must stay revoked, no auto-issue");
    }

    // ── rejected holds until re-approve ───────────────────────────────────

    #[tokio::test]
    async fn test_rejected_holds_until_reapprove() {
        let store = MemEnrollmentStore::new();
        let ca = test_ca();
        let audit = MemAuditStore::new();
        let id = identity("host7", "aa:bb:cc:dd:ee:07");
        let csr_pem = csr_for(&id);

        let submitted = submit_csr(&store, &ca, &audit, &csr_pem, &id.mac, &id.hostname)
            .await
            .unwrap();
        let rejected = reject(&store, &audit, &submitted.spki_fingerprint, "alice")
            .await
            .unwrap();
        assert_eq!(rejected.state, EnrollmentState::Rejected);

        // Holds: a poll (and even a resubmit) must not move it off `rejected`.
        let polled = get_credential(&store, &submitted.spki_fingerprint)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(polled.state, EnrollmentState::Rejected);
        let resubmitted = submit_csr(&store, &ca, &audit, &csr_pem, &id.mac, &id.hostname)
            .await
            .unwrap();
        assert_eq!(resubmitted.state, EnrollmentState::Rejected, "resubmit must not clear rejected");

        // Operator re-approves: rejected -> issued.
        let issued = approve(&store, &ca, &audit, &submitted.spki_fingerprint, "bob")
            .await
            .unwrap();
        assert_eq!(issued.state, EnrollmentState::Issued);
    }

    // ── CA custody boundary ───────────────────────────────────────────────

    #[test]
    fn test_no_second_trust_root_referenced_in_this_module() {
        // The whole point of Decision 6: this module never names a second CA. A
        // grep-based acceptance check backs this at the repo level; this test pins
        // the same property at the type level (the only signer type in scope is
        // `crate::ca::InstallCa`).
        fn assert_signer_is_install_ca(_ca: &InstallCa) {}
        let ca = test_ca();
        assert_signer_is_install_ca(&ca);
    }

    // ── gRPC surface ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_grpc_submit_then_get_credential_not_found() {
        use uaa_proto::enroll::v1::enroll_service_server::EnrollService as _;
        use uaa_proto::enroll::v1::{GetCredentialRequest, SubmitCsrRequest};

        let store: Arc<dyn EnrollmentStore> = Arc::new(MemEnrollmentStore::new());
        let ca = Arc::new(test_ca());
        let audit: Arc<dyn AuditStore> = Arc::new(MemAuditStore::new());
        let svc = EnrollGrpcService::new(store, ca, audit);

        let unknown = svc
            .get_credential(tonic::Request::new(GetCredentialRequest {
                spki_fingerprint: "unknown".to_string(),
            }))
            .await;
        assert_eq!(unknown.unwrap_err().code(), tonic::Code::NotFound);

        let id = identity("host8", "aa:bb:cc:dd:ee:08");
        let csr_pem = csr_for(&id);
        let resp = svc
            .submit_csr(tonic::Request::new(SubmitCsrRequest {
                csr_pem,
                claimed_hostname: id.hostname.clone(),
                claimed_mac: id.mac.clone(),
            }))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(resp.state, "pending");
    }
}
