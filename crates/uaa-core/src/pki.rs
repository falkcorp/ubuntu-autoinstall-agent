// file: crates/uaa-core/src/pki.rs
// version: 1.1.1
// guid: fd51b533-b595-41dc-97e2-1e5c54476890
// last-edited: 2026-07-14

//! PKI (CA/cert issuance) — agent-side half of spec C6 / Decision 7.
//!
//! Implements the client state machine (design spec `constellation-design.md`,
//! component C1 sketch + component C6, NORMATIVE):
//!
//! 1. Generate a P-256 keypair + CSR once (`generate_keypair_and_csr`), SAN =
//!    hostname + `uaa-mac:<mac>` URI — persist under `state_dir` and re-derive the
//!    SAME SPKI fingerprint on every restart (idempotent re-claim, never mint a
//!    second keypair over a live claim).
//! 2. Pin the install CA from `--ca` (baked into the ISO/PXE seed by PK-04):
//!    missing/unreadable CA file is FAIL-CLOSED — a typed [`AutoInstallError`] the
//!    caller can retry on. NEVER fall back to system roots or plain HTTP
//!    (Decision 7); the install CA is the ONLY trust root — the CockroachDB CA is
//!    NEVER used (Decision 6).
//! 3. `SubmitCsr` (idempotent — safe on every boot), then poll `GetCredential`
//!    keyed by the SPKI fingerprint with exponential backoff (30s → 5m cap,
//!    [`backoff_delay`]). `pending`/`approved` → keep polling. `issued` → persist
//!    `agent.crt` (0600, tmp+rename) and return. `rejected`/`revoked`/`superseded`
//!    → log loudly and hold at a fixed 1h poll interval (the loop survives a
//!    transition back to `issued` if an operator re-approves). Unknown-fp 404
//!    while we hold a local claim means the server lost state — re-submit the CSR,
//!    never error out.
//! 4. A persisted, unexpired, non-past-2/3-lifetime `agent.crt` short-circuits the
//!    whole flow with zero network calls.
//!
//! The HTTP transport (`:15002` JSON mirror of `uaa.enroll.v1.EnrollService`) sits
//! behind [`EnrollTransport`] so tests run against a scripted mock — no sockets —
//! and the sleep between polls sits behind [`Sleeper`] so tests never really sleep.

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use rcgen::{
    CertificateParams, DistinguishedName, DnType, Ia5String, KeyPair, PKCS_ECDSA_P256_SHA256,
    SanType,
};
use x509_parser::certification_request::X509CertificationRequest;
use x509_parser::pem::parse_x509_pem;
use x509_parser::prelude::FromDer;
use x509_parser::time::ASN1Time;

use crate::{AutoInstallError, Result};

/// PEM-encoded private key.
pub type KeyPem = String;
/// PEM-encoded certificate signing request.
pub type CsrPem = String;
/// PEM-encoded certificate (agent cert or CA cert).
pub type CertPem = String;

/// Exponential backoff base (30s) and cap (5m) per spec C6.
const BACKOFF_BASE_SECS: u64 = 30;
const BACKOFF_CAP_SECS: u64 = 300;
/// Fixed poll interval while held in a terminal state (rejected/revoked/superseded).
const HOLD_INTERVAL: Duration = Duration::from_secs(3600);
/// Renew (re-submit the same-key CSR) once a persisted cert has consumed 2/3 of
/// its lifetime — matches PK-01's server-side same-key auto-issue rule.
const RENEWAL_LIFETIME_NUMERATOR: i64 = 2;
const RENEWAL_LIFETIME_DENOMINATOR: i64 = 3;

const AGENT_KEY_FILE: &str = "agent.key";
const AGENT_CSR_FILE: &str = "agent.csr";
const AGENT_CERT_FILE: &str = "agent.crt";
const CLAIM_FILE: &str = "claim.json";

/// Identity claimed by this agent when requesting a certificate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentIdentity {
    pub hostname: String,
    pub mac: String,
}

/// Local record of what we submitted and when — persisted alongside the key/CSR
/// so a restart can re-derive the SAME SPKI fingerprint without minting a new
/// keypair over a live claim.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaimRecord {
    pub spki_fingerprint: String,
    pub submitted_at: String,
}

/// The issued credential returned once enrollment reaches the `issued` state (or
/// a still-valid persisted cert short-circuits the flow).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credential {
    pub spki_fingerprint: String,
    pub cert_pem: CertPem,
    pub ca_pem: CertPem,
}

/// Local persisted enrollment state: keypair, CSR, SPKI fingerprint, and claim
/// record, loaded (or created once) from `state_dir`.
pub struct EnrollState {
    pub csr_pem: CsrPem,
    pub spki_fp: String,
    pub claim: ClaimRecord,
}

impl EnrollState {
    /// Load persisted `agent.key`/`agent.csr`/`claim.json` from `state_dir`,
    /// creating all three (generating a keypair+CSR once) if absent. Re-loading
    /// an existing dir always yields the identical SPKI fingerprint — this is the
    /// idempotent re-claim contract from spec C6.
    pub fn load_or_init(state_dir: &Path, identity: &AgentIdentity) -> Result<Self> {
        ensure_state_dir(state_dir)?;

        let key_path = state_dir.join(AGENT_KEY_FILE);
        let csr_path = state_dir.join(AGENT_CSR_FILE);
        let claim_path = state_dir.join(CLAIM_FILE);

        let csr_pem = if key_path.is_file() && csr_path.is_file() {
            // Never mint a second keypair over a live claim — reload verbatim.
            fs::read_to_string(&csr_path)?
        } else {
            let (key_pem, csr_pem) = generate_keypair_and_csr(identity)?;
            write_atomic_0600(&key_path, key_pem.as_bytes())?;
            write_atomic_0600(&csr_path, csr_pem.as_bytes())?;
            csr_pem
        };

        let spki_fp = spki_fingerprint(&csr_pem)?;

        let claim = if claim_path.is_file() {
            let raw = fs::read_to_string(&claim_path)?;
            serde_json::from_str(&raw)?
        } else {
            let claim = ClaimRecord {
                spki_fingerprint: spki_fp.clone(),
                submitted_at: chrono::Utc::now().to_rfc3339(),
            };
            let raw = serde_json::to_string_pretty(&claim)?;
            write_atomic_0600(&claim_path, raw.as_bytes())?;
            claim
        };

        Ok(EnrollState {
            csr_pem,
            spki_fp,
            claim,
        })
    }

    /// A persisted `agent.crt` that exists, is unexpired, and has not crossed the
    /// 2/3-lifetime renewal threshold short-circuits `enroll_poll` with no
    /// network call. Returns `None` if the file is absent, expired, or due for
    /// renewal (the caller then falls through to submit/poll as usual, which
    /// re-submits the SAME persisted CSR — the renewal rule).
    pub fn valid_unexpired_cert(&self, state_dir: &Path) -> Result<Option<CertPem>> {
        let cert_path = state_dir.join(AGENT_CERT_FILE);
        if !cert_path.is_file() {
            return Ok(None);
        }
        let cert_pem = fs::read_to_string(&cert_path)?;
        match cert_lifecycle(&cert_pem)? {
            CertLifecycle::Fresh => Ok(Some(cert_pem)),
            CertLifecycle::NeedsRenewal | CertLifecycle::Expired => Ok(None),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum CertLifecycle {
    Fresh,
    NeedsRenewal,
    Expired,
}

/// Pure classification against the 2/3-lifetime renewal threshold — a function
/// of unix timestamps only, so tests can assert the boundary without minting a
/// real certificate or touching a clock/time crate.
fn classify_lifecycle(now: i64, not_before: i64, not_after: i64) -> CertLifecycle {
    if now >= not_after {
        return CertLifecycle::Expired;
    }
    let lifetime = (not_after - not_before).max(1);
    let elapsed = (now - not_before).max(0);
    if elapsed * RENEWAL_LIFETIME_DENOMINATOR >= lifetime * RENEWAL_LIFETIME_NUMERATOR {
        CertLifecycle::NeedsRenewal
    } else {
        CertLifecycle::Fresh
    }
}

/// Parse `cert_pem` and classify it against the 2/3-lifetime renewal threshold.
fn cert_lifecycle(cert_pem: &str) -> Result<CertLifecycle> {
    let (_, pem) = parse_x509_pem(cert_pem.as_bytes())
        .map_err(|e| AutoInstallError::ValidationError(format!("invalid cert PEM: {e:?}")))?;
    let (_, cert) = x509_parser::parse_x509_certificate(&pem.contents)
        .map_err(|e| AutoInstallError::ValidationError(format!("invalid cert DER: {e:?}")))?;
    let validity = cert.validity();
    let now = ASN1Time::now().timestamp();
    Ok(classify_lifecycle(
        now,
        validity.not_before.timestamp(),
        validity.not_after.timestamp(),
    ))
}

/// Generate a fresh P-256 keypair and a CSR whose SAN carries the hostname (DNS)
/// and `uaa-mac:<mac>` (URI) — mirrors what PK-01's server signs onto the issued
/// certificate.
pub fn generate_keypair_and_csr(identity: &AgentIdentity) -> Result<(KeyPem, CsrPem)> {
    let key_pair = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)
        .map_err(|e| AutoInstallError::ConfigError(format!("P-256 keypair generation failed: {e}")))?;

    let mut params = CertificateParams::new(vec![identity.hostname.clone()]).map_err(|e| {
        AutoInstallError::ConfigError(format!("CSR hostname SAN failed: {e}"))
    })?;

    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, identity.hostname.clone());
    params.distinguished_name = dn;

    let mac_uri = Ia5String::try_from(format!("uaa-mac:{}", identity.mac))
        .map_err(|e| AutoInstallError::ConfigError(format!("CSR mac SAN encoding failed: {e}")))?;
    params.subject_alt_names.push(SanType::URI(mac_uri));

    let csr = params
        .serialize_request(&key_pair)
        .map_err(|e| AutoInstallError::ConfigError(format!("CSR serialization failed: {e}")))?;
    let csr_pem = csr
        .pem()
        .map_err(|e| AutoInstallError::ConfigError(format!("CSR PEM encoding failed: {e}")))?;
    let key_pem = key_pair.serialize_pem();

    Ok((key_pem, csr_pem))
}

/// SHA-256 hex digest of the CSR's `SubjectPublicKeyInfo` (RFC 7469 SPKI-pin
/// style): the FULL DER-encoded `SubjectPublicKeyInfo` SEQUENCE — algorithm
/// identifier + the `subjectPublicKey` BIT STRING — exactly the byte range
/// `x509_parser`'s CSR parser exposes as `certification_request_info.subject_pki.raw`.
/// This is NOT just the raw EC point bytes. PK-01's server MUST hash the
/// identical byte range when it parses the incoming CSR for the two sides to
/// agree on the same fingerprint.
pub fn spki_fingerprint(csr_pem: &str) -> Result<String> {
    let (_, pem) = parse_x509_pem(csr_pem.as_bytes())
        .map_err(|e| AutoInstallError::ValidationError(format!("invalid CSR PEM: {e:?}")))?;
    let (_, csr) = X509CertificationRequest::from_der(&pem.contents)
        .map_err(|e| AutoInstallError::ValidationError(format!("invalid CSR DER: {e:?}")))?;
    let spki_der = csr.certification_request_info.subject_pki.raw;
    let digest = Sha256::digest(spki_der);
    Ok(digest.iter().map(|b| format!("{b:02x}")).collect())
}

/// Best-effort local hostname (used by the CLI when `--hostname` is not given —
/// the enroll command runs ON the host being enrolled).
pub fn local_hostname() -> Result<String> {
    nix::unistd::gethostname()
        .map_err(|e| AutoInstallError::SystemError(format!("gethostname failed: {e}")))?
        .into_string()
        .map_err(|_| AutoInstallError::SystemError("hostname is not valid UTF-8".to_string()))
}

// ── Persistence helpers ─────────────────────────────────────────────────────

fn ensure_state_dir(dir: &Path) -> Result<()> {
    if !dir.is_dir() {
        fs::create_dir_all(dir)?;
    }
    let mut perms = fs::metadata(dir)?.permissions();
    perms.set_mode(0o700);
    fs::set_permissions(dir, perms)?;
    Ok(())
}

/// Write `contents` to `path` atomically: a 0600 temp file in the SAME
/// directory, then rename over the destination.
fn write_atomic_0600(path: &Path, contents: &[u8]) -> Result<()> {
    let dir = path.parent().ok_or_else(|| {
        AutoInstallError::ConfigError(format!("no parent directory for {}", path.display()))
    })?;
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    tmp.write_all(contents)?;
    tmp.flush()?;
    let mut perms = tmp.as_file().metadata()?.permissions();
    perms.set_mode(0o600);
    tmp.as_file().set_permissions(perms)?;
    tmp.persist(path)
        .map_err(|e| AutoInstallError::IoError(e.error))?;
    Ok(())
}

fn persist_cert(state_dir: &Path, cert_pem: &str) -> Result<()> {
    write_atomic_0600(&state_dir.join(AGENT_CERT_FILE), cert_pem.as_bytes())
}

/// Pin the install CA: missing/unreadable = FAIL-CLOSED, a typed error the
/// caller retries on. NEVER fall back to system roots or plain HTTP.
fn load_pinned_ca(ca_path: &Path) -> Result<CertPem> {
    fs::read_to_string(ca_path).map_err(|e| {
        AutoInstallError::ConfigError(format!(
            "install CA not found or unreadable at {} (fail-closed: refusing to fall back to \
             system roots or plain HTTP): {e}",
            ca_path.display()
        ))
    })
}

// ── Backoff ──────────────────────────────────────────────────────────────

/// Pure backoff schedule: 30s, 60s, 120s, 240s, then capped at 300s (5m) for
/// every subsequent attempt. A pure function so tests can assert the schedule
/// without sleeping.
pub fn backoff_delay(attempt: u32) -> Duration {
    let multiplier = 1u64.checked_shl(attempt).unwrap_or(u64::MAX);
    let secs = BACKOFF_BASE_SECS.saturating_mul(multiplier);
    Duration::from_secs(secs.min(BACKOFF_CAP_SECS))
}

// ── Sleep seam ───────────────────────────────────────────────────────────

/// Clock/sleep seam — production uses [`TokioSleeper`], tests use a mock that
/// records durations without actually sleeping.
#[async_trait::async_trait]
pub trait Sleeper: Send + Sync {
    async fn sleep(&self, duration: Duration);
}

/// Production [`Sleeper`] backed by `tokio::time::sleep`.
pub struct TokioSleeper;

#[async_trait::async_trait]
impl Sleeper for TokioSleeper {
    async fn sleep(&self, duration: Duration) {
        tokio::time::sleep(duration).await;
    }
}

// ── Transport ────────────────────────────────────────────────────────────

/// Request body for `POST /enroll/csr` (JSON mirror of `uaa.enroll.v1.SubmitCsr`).
#[derive(Debug, Clone, Serialize)]
pub struct SubmitCsrRequest {
    pub csr_pem: String,
    pub claimed_hostname: String,
    pub claimed_mac: String,
}

/// Response body for `POST /enroll/csr`.
#[derive(Debug, Clone, Deserialize)]
pub struct SubmitCsrResponse {
    pub spki_fingerprint: String,
    pub state: String,
}

/// Response body for `GET /enroll/credential/<fp>` (200 case; a 404 is
/// represented as `Ok(None)` by [`EnrollTransport::get_credential`]).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct GetCredentialResponse {
    /// One of: pending|approved|issued|rejected|revoked|superseded.
    pub state: String,
    #[serde(default)]
    pub cert_pem: String,
    #[serde(default)]
    pub ca_pem: String,
}

/// The `:15002` JSON enrollment plane, behind a trait so tests never open a
/// socket. `get_credential` returns `Ok(None)` for a 404 (unknown SPKI fp) —
/// never an `Err` — so the state machine can distinguish "server lost state,
/// re-submit" from a real transport failure.
#[async_trait::async_trait]
pub trait EnrollTransport: Send + Sync {
    async fn submit_csr(&self, req: SubmitCsrRequest) -> Result<SubmitCsrResponse>;
    async fn get_credential(&self, spki_fingerprint: &str) -> Result<Option<GetCredentialResponse>>;
}

/// Production [`EnrollTransport`]: reqwest with rustls, system roots DISABLED,
/// the pinned install CA as the ONLY trust root (Decision 7). NEVER the
/// CockroachDB CA (Decision 6).
pub struct ReqwestEnrollTransport {
    client: reqwest::Client,
    base_url: reqwest::Url,
}

impl ReqwestEnrollTransport {
    pub fn new(base_url: reqwest::Url, pinned_ca_pem: &str) -> Result<Self> {
        let ca_cert = reqwest::Certificate::from_pem(pinned_ca_pem.as_bytes()).map_err(|e| {
            AutoInstallError::ConfigError(format!("pinned CA is not a valid PEM certificate: {e}"))
        })?;
        let client = reqwest::ClientBuilder::new()
            .tls_built_in_root_certs(false)
            .add_root_certificate(ca_cert)
            .build()
            .map_err(AutoInstallError::from)?;
        Ok(Self { client, base_url })
    }

    fn join(&self, path: &str) -> Result<reqwest::Url> {
        self.base_url.join(path).map_err(|e| {
            AutoInstallError::ConfigError(format!("invalid enrollment endpoint {path}: {e}"))
        })
    }
}

#[async_trait::async_trait]
impl EnrollTransport for ReqwestEnrollTransport {
    async fn submit_csr(&self, req: SubmitCsrRequest) -> Result<SubmitCsrResponse> {
        let url = self.join("/enroll/csr")?;
        let resp = self.client.post(url).json(&req).send().await?;
        let resp = resp.error_for_status()?;
        Ok(resp.json::<SubmitCsrResponse>().await?)
    }

    async fn get_credential(&self, spki_fingerprint: &str) -> Result<Option<GetCredentialResponse>> {
        let url = self.join(&format!("/enroll/credential/{spki_fingerprint}"))?;
        let resp = self.client.get(url).send().await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let resp = resp.error_for_status()?;
        Ok(Some(resp.json::<GetCredentialResponse>().await?))
    }
}

// ── State machine ────────────────────────────────────────────────────────

enum PollOutcome {
    Pending,
    Issued(Credential),
    Held,
}

async fn poll_once(
    transport: &dyn EnrollTransport,
    spki_fp: &str,
    submit_req: &SubmitCsrRequest,
    ca_pem: &str,
) -> Result<PollOutcome> {
    match transport.get_credential(spki_fp).await? {
        None => {
            // Unknown-fp 404 while we hold a local claim = server lost state.
            // Re-submit, do not error out.
            tracing::warn!(
                "enroll: server has no record of spki={spki_fp} (404) — re-submitting CSR"
            );
            transport.submit_csr(submit_req.clone()).await?;
            Ok(PollOutcome::Pending)
        }
        Some(resp) => match resp.state.as_str() {
            "issued" => Ok(PollOutcome::Issued(Credential {
                spki_fingerprint: spki_fp.to_string(),
                cert_pem: resp.cert_pem,
                ca_pem: if resp.ca_pem.is_empty() {
                    ca_pem.to_string()
                } else {
                    resp.ca_pem
                },
            })),
            "pending" | "approved" => Ok(PollOutcome::Pending),
            "rejected" | "revoked" | "superseded" => Ok(PollOutcome::Held),
            other => Err(AutoInstallError::ValidationError(format!(
                "unexpected enrollment state from server: {other}"
            ))),
        },
    }
}

/// The persistent, restart-resumable poll loop (spec C6): short-circuits on a
/// valid persisted cert, fail-closes on a missing/unreadable pinned CA,
/// submits the CSR (idempotent), then polls with exponential backoff — holding
/// at a fixed 1h interval on a terminal (rejected/revoked/superseded) state and
/// re-submitting on an unknown-fp 404. Generic over [`EnrollTransport`] and
/// [`Sleeper`] so tests run with a scripted mock and no real sleeping.
pub async fn enroll_poll_with(
    identity: &AgentIdentity,
    transport: &dyn EnrollTransport,
    sleeper: &dyn Sleeper,
    ca_path: &Path,
    state_dir: &Path,
) -> Result<Credential> {
    let ca_pem = load_pinned_ca(ca_path)?;
    let enroll_state = EnrollState::load_or_init(state_dir, identity)?;

    if let Some(cert_pem) = enroll_state.valid_unexpired_cert(state_dir)? {
        tracing::info!(
            "enroll: persisted cert for spki={} is still valid — short-circuiting, no network call",
            enroll_state.spki_fp
        );
        return Ok(Credential {
            spki_fingerprint: enroll_state.spki_fp,
            cert_pem,
            ca_pem,
        });
    }

    let submit_req = SubmitCsrRequest {
        csr_pem: enroll_state.csr_pem.clone(),
        claimed_hostname: identity.hostname.clone(),
        claimed_mac: identity.mac.clone(),
    };
    tracing::info!(
        "enroll: submitting CSR for spki={} (idempotent upsert)",
        enroll_state.spki_fp
    );
    transport.submit_csr(submit_req.clone()).await?;

    let mut attempt: u32 = 0;
    loop {
        match poll_once(transport, &enroll_state.spki_fp, &submit_req, &ca_pem).await? {
            PollOutcome::Issued(credential) => {
                persist_cert(state_dir, &credential.cert_pem)?;
                tracing::info!(
                    "enroll: credential issued for spki={} — persisted {}",
                    enroll_state.spki_fp,
                    state_dir.join(AGENT_CERT_FILE).display()
                );
                return Ok(credential);
            }
            PollOutcome::Pending => {
                let delay = backoff_delay(attempt);
                tracing::info!(
                    "enroll: pending (spki={}, attempt={attempt}) — backing off {}s",
                    enroll_state.spki_fp,
                    delay.as_secs()
                );
                sleeper.sleep(delay).await;
                attempt = attempt.saturating_add(1);
            }
            PollOutcome::Held => {
                tracing::error!(
                    "enroll: held in a terminal state (rejected/revoked/superseded) for spki={} \
                     — operator re-approval required; retrying hourly",
                    enroll_state.spki_fp
                );
                sleeper.sleep(HOLD_INTERVAL).await;
                // Fixed interval, not exponential — do not advance `attempt`.
            }
        }
    }
}

/// Production entry point: builds the reqwest transport (pinned CA, system
/// roots disabled) and the real tokio sleeper, then runs [`enroll_poll_with`].
pub async fn enroll_poll(
    identity: &AgentIdentity,
    endpoint: &str,
    ca_path: &Path,
    state_dir: &Path,
) -> Result<Credential> {
    let ca_pem = load_pinned_ca(ca_path)?;
    let base_url = reqwest::Url::parse(endpoint).map_err(|e| {
        AutoInstallError::ConfigError(format!("invalid enrollment endpoint {endpoint}: {e}"))
    })?;
    let transport = ReqwestEnrollTransport::new(base_url, &ca_pem)?;
    enroll_poll_with(identity, &transport, &TokioSleeper, ca_path, state_dir).await
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;
    use x509_parser::extensions::{GeneralName, ParsedExtension};

    fn test_identity() -> AgentIdentity {
        AgentIdentity {
            hostname: "testhost".to_string(),
            mac: "aa:bb:cc:dd:ee:ff".to_string(),
        }
    }

    fn write_dummy_ca(dir: &Path) -> PathBuf {
        let path = dir.join("install-ca.crt");
        fs::write(&path, "-----BEGIN CERTIFICATE-----\ndummy\n-----END CERTIFICATE-----\n")
            .unwrap();
        path
    }

    /// Generates a real, ephemeral rcgen-signed cert (self-signed is fine — the
    /// short-circuit test only cares that it parses and is "fresh"). Uses
    /// rcgen's default validity window (roughly year 1975 to year 4096), which
    /// is always `CertLifecycle::Fresh` by `classify_lifecycle` — no need to
    /// pull in a `time`-crate dependency just to compute a relative window.
    fn ephemeral_cert_pem() -> String {
        let key_pair = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
        let params = CertificateParams::default();
        let cert = params.self_signed(&key_pair).unwrap();
        cert.pem()
    }

    // ── MockTransport ──────────────────────────────────────────────────

    #[derive(Default)]
    struct MockTransport {
        get_script: Mutex<VecDeque<Option<GetCredentialResponse>>>,
        submit_calls: AtomicUsize,
        get_calls: AtomicUsize,
    }

    impl MockTransport {
        fn with_script(script: Vec<Option<GetCredentialResponse>>) -> Self {
            Self {
                get_script: Mutex::new(script.into_iter().collect()),
                ..Default::default()
            }
        }

        fn submit_call_count(&self) -> usize {
            self.submit_calls.load(Ordering::SeqCst)
        }

        fn get_call_count(&self) -> usize {
            self.get_calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait::async_trait]
    impl EnrollTransport for MockTransport {
        async fn submit_csr(&self, _req: SubmitCsrRequest) -> Result<SubmitCsrResponse> {
            self.submit_calls.fetch_add(1, Ordering::SeqCst);
            Ok(SubmitCsrResponse {
                spki_fingerprint: String::new(),
                state: "pending".to_string(),
            })
        }

        async fn get_credential(
            &self,
            _spki_fingerprint: &str,
        ) -> Result<Option<GetCredentialResponse>> {
            self.get_calls.fetch_add(1, Ordering::SeqCst);
            self.get_script.lock().unwrap().pop_front().ok_or_else(|| {
                AutoInstallError::ValidationError("mock get_credential queue exhausted".into())
            })
        }
    }

    fn issued(cert_pem: &str) -> Option<GetCredentialResponse> {
        Some(GetCredentialResponse {
            state: "issued".to_string(),
            cert_pem: cert_pem.to_string(),
            ca_pem: "ca-pem-from-server".to_string(),
        })
    }

    fn pending() -> Option<GetCredentialResponse> {
        Some(GetCredentialResponse {
            state: "pending".to_string(),
            cert_pem: String::new(),
            ca_pem: String::new(),
        })
    }

    fn rejected() -> Option<GetCredentialResponse> {
        Some(GetCredentialResponse {
            state: "rejected".to_string(),
            cert_pem: String::new(),
            ca_pem: String::new(),
        })
    }

    // ── MockSleeper ────────────────────────────────────────────────────

    #[derive(Default)]
    struct MockSleeper {
        durations: Mutex<Vec<Duration>>,
    }

    impl MockSleeper {
        fn durations(&self) -> Vec<Duration> {
            self.durations.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl Sleeper for MockSleeper {
        async fn sleep(&self, duration: Duration) {
            self.durations.lock().unwrap().push(duration);
        }
    }

    // ── Tests ──────────────────────────────────────────────────────────

    #[test]
    fn test_keypair_csr_sans() {
        let identity = test_identity();
        let (key_pem, csr_pem) = generate_keypair_and_csr(&identity).expect("keypair+csr");
        assert!(key_pem.contains("PRIVATE KEY"));
        assert!(csr_pem.contains("BEGIN CERTIFICATE REQUEST"));

        let (_, pem) = parse_x509_pem(csr_pem.as_bytes()).unwrap();
        let (_, csr) = X509CertificationRequest::from_der(&pem.contents).unwrap();
        let mut found_dns = false;
        let mut found_uri = false;
        for ext in csr.requested_extensions().into_iter().flatten() {
            if let ParsedExtension::SubjectAlternativeName(san) = ext {
                for name in &san.general_names {
                    match name {
                        GeneralName::DNSName(dns) if *dns == identity.hostname => found_dns = true,
                        GeneralName::URI(uri) if *uri == format!("uaa-mac:{}", identity.mac) => {
                            found_uri = true
                        }
                        _ => {}
                    }
                }
            }
        }
        assert!(found_dns, "expected DNS SAN = hostname");
        assert!(found_uri, "expected URI SAN = uaa-mac:<mac>");
    }

    #[test]
    fn test_spki_fp_stable_across_reload() {
        let dir = tempfile::tempdir().unwrap();
        let identity = test_identity();

        let first = EnrollState::load_or_init(dir.path(), &identity).unwrap();
        let first_fp = first.spki_fp.clone();
        let first_csr = first.csr_pem.clone();
        drop(first);

        let second = EnrollState::load_or_init(dir.path(), &identity).unwrap();
        assert_eq!(first_fp, second.spki_fp, "SPKI fp must be stable across reload");
        assert_eq!(first_csr, second.csr_pem, "CSR bytes must be identical (no re-mint)");
    }

    #[tokio::test]
    async fn test_missing_ca_fail_closed() {
        let dir = tempfile::tempdir().unwrap();
        let missing_ca = dir.path().join("no-such-ca.crt");
        let transport = MockTransport::default();
        let sleeper = MockSleeper::default();

        let result = enroll_poll_with(
            &test_identity(),
            &transport,
            &sleeper,
            &missing_ca,
            dir.path(),
        )
        .await;

        assert!(result.is_err(), "missing CA must fail closed");
        assert_eq!(transport.submit_call_count(), 0);
        assert_eq!(transport.get_call_count(), 0);
    }

    #[test]
    fn test_backoff_schedule_30s_to_5m_cap() {
        let expected_secs = [30, 60, 120, 240, 300, 300, 300, 300, 300, 300];
        for (attempt, expected) in expected_secs.iter().enumerate() {
            let delay = backoff_delay(attempt as u32);
            assert_eq!(
                delay,
                Duration::from_secs(*expected),
                "attempt {attempt} expected {expected}s, got {delay:?}"
            );
        }
    }

    #[tokio::test]
    async fn test_pending_then_issued_persists_cert() {
        let dir = tempfile::tempdir().unwrap();
        let ca_path = write_dummy_ca(dir.path());
        let cert_content = "-----BEGIN CERTIFICATE-----\nISSUED\n-----END CERTIFICATE-----\n";
        let transport =
            MockTransport::with_script(vec![pending(), pending(), issued(cert_content)]);
        let sleeper = MockSleeper::default();

        let credential =
            enroll_poll_with(&test_identity(), &transport, &sleeper, &ca_path, dir.path())
                .await
                .expect("pending,pending,issued must resolve Ok");

        assert_eq!(credential.cert_pem, cert_content);
        assert_eq!(transport.get_call_count(), 3);
        assert_eq!(transport.submit_call_count(), 1);

        let cert_path = dir.path().join(AGENT_CERT_FILE);
        assert!(cert_path.is_file());
        let mode = fs::metadata(&cert_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "agent.crt must be mode 0600");
        assert_eq!(fs::read_to_string(&cert_path).unwrap(), cert_content);

        // Backoff must have been used for the two `pending` responses.
        assert_eq!(sleeper.durations(), vec![Duration::from_secs(30), Duration::from_secs(60)]);
    }

    #[tokio::test]
    async fn test_rejected_holds_1h() {
        let dir = tempfile::tempdir().unwrap();
        let ca_path = write_dummy_ca(dir.path());
        let cert_content = "-----BEGIN CERTIFICATE-----\nRECOVERED\n-----END CERTIFICATE-----\n";
        // rejected, then (operator re-approves) issued — the loop must survive
        // the transition back to `issued`.
        let transport = MockTransport::with_script(vec![rejected(), issued(cert_content)]);
        let sleeper = MockSleeper::default();

        let credential =
            enroll_poll_with(&test_identity(), &transport, &sleeper, &ca_path, dir.path())
                .await
                .expect("rejected must hold, not error — and recover on re-approval");

        assert_eq!(credential.cert_pem, cert_content);
        assert_eq!(
            sleeper.durations(),
            vec![HOLD_INTERVAL],
            "rejected must hold at exactly the fixed 1h interval"
        );
    }

    #[tokio::test]
    async fn test_404_with_claim_resubmits() {
        let dir = tempfile::tempdir().unwrap();
        let ca_path = write_dummy_ca(dir.path());
        let cert_content = "-----BEGIN CERTIFICATE-----\nRESUBMITTED\n-----END CERTIFICATE-----\n";
        // None == 404 unknown-fp: server lost state.
        let transport = MockTransport::with_script(vec![None, issued(cert_content)]);
        let sleeper = MockSleeper::default();

        let credential =
            enroll_poll_with(&test_identity(), &transport, &sleeper, &ca_path, dir.path())
                .await
                .expect("404-with-claim must re-submit, not error");

        assert_eq!(credential.cert_pem, cert_content);
        // Initial submit + one resubmit after the 404.
        assert_eq!(transport.submit_call_count(), 2);
        assert_eq!(transport.get_call_count(), 2);
    }

    #[tokio::test]
    async fn test_valid_cert_short_circuits_no_network() {
        let dir = tempfile::tempdir().unwrap();
        let ca_path = write_dummy_ca(dir.path());
        let identity = test_identity();

        // Pre-populate key/csr/claim, then a fresh, unexpired cert.
        EnrollState::load_or_init(dir.path(), &identity).unwrap();
        let cert_pem = ephemeral_cert_pem();
        persist_cert(dir.path(), &cert_pem).unwrap();

        let transport = MockTransport::default();
        let sleeper = MockSleeper::default();

        let credential =
            enroll_poll_with(&identity, &transport, &sleeper, &ca_path, dir.path())
                .await
                .expect("valid persisted cert must short-circuit");

        assert_eq!(credential.cert_pem, cert_pem);
        assert_eq!(transport.submit_call_count(), 0);
        assert_eq!(transport.get_call_count(), 0);
        assert!(sleeper.durations().is_empty());
    }

    #[test]
    fn test_cert_lifecycle_thresholds() {
        // lifetime = 900s; pure function of (now, not_before, not_after) — no
        // clock/time crate needed to pin down the 2/3-lifetime boundary.
        let not_before = 0i64;
        let not_after = 900i64;

        assert_eq!(classify_lifecycle(0, not_before, not_after), CertLifecycle::Fresh);
        assert_eq!(classify_lifecycle(599, not_before, not_after), CertLifecycle::Fresh);
        // Exactly 2/3 through (600/900) and beyond -> due for renewal.
        assert_eq!(
            classify_lifecycle(600, not_before, not_after),
            CertLifecycle::NeedsRenewal
        );
        assert_eq!(
            classify_lifecycle(899, not_before, not_after),
            CertLifecycle::NeedsRenewal
        );
        // At/after not_after -> expired.
        assert_eq!(classify_lifecycle(900, not_before, not_after), CertLifecycle::Expired);
        assert_eq!(classify_lifecycle(1000, not_before, not_after), CertLifecycle::Expired);
    }

    #[test]
    fn test_spki_fingerprint_matches_across_equivalent_parses() {
        // The fingerprint must be a pure function of the CSR bytes (same CSR
        // parsed twice yields the same fp) — this is the property PK-01's
        // server-side computation depends on.
        let identity = test_identity();
        let (_, csr_pem) = generate_keypair_and_csr(&identity).unwrap();
        let fp1 = spki_fingerprint(&csr_pem).unwrap();
        let fp2 = spki_fingerprint(&csr_pem).unwrap();
        assert_eq!(fp1, fp2);
        assert_eq!(fp1.len(), 64, "sha256 hex digest is 64 chars");
    }
}
