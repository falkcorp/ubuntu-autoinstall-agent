// file: crates/uaa-control/src/operator/handlers.rs
// version: 1.4.0
// guid: e94ff17e-4e1b-4672-8940-1fe111b56861
// last-edited: 2026-07-13

//! Operator API request handlers (`:15001`, mounted under `/api/*` ahead of
//! [`super::web_ui`]'s SPA fallback).
//!
//! This is a first vertical slice, not the full CT-07 scope: `GET
//! /api/machines` (+ single-machine GET, + approve) is real, backed by the
//! same CT-01 snapshot `machine_plane::{seeds,lifecycle}` read/write.
//! Enrollments (`GET /api/enrollments`, approve, reject) and audit (`GET
//! /api/audit`, `GET /api/audit/verify`) are ALSO now real — wired against
//! PK-01's `crate::enroll` state machine and CT-01's `crate::audit` chain,
//! the same logic + tests that already existed, just not previously exposed
//! over HTTP. Discovery (`GET /api/discovered`, dismiss) is still stubbed:
//! unlike enrollments/audit, no backend exists ANYWHERE in the crate yet for
//! `discovered_macs` — this is unbuilt feature work, not a wiring gap.
//!
//! Enrollments/audit currently run against IN-MEMORY stores
//! ([`crate::enroll::MemEnrollmentStore`], [`crate::audit::MemAuditStore`]),
//! not a database — state (pending enrollments, the audit chain) does NOT
//! survive a `uaa-control` restart. This is a known, deliberate limitation,
//! not an oversight: no `PgEnrollmentStore` exists in this crate yet, and
//! wiring `PgAuditStore` (which DOES already exist) would need DB connection
//! plumbing this crate's `main.rs`/`listeners::serve` doesn't have today.
//! Flagged here rather than silently shipped as if it were durable.
//!
//! # Auth (2026-07-13)
//!
//! Every route below is now gated by `crate::auth`'s (CT-03) RBAC middleware:
//! reads require [`crate::auth::Role::Viewer`], mutations
//! [`crate::auth::Role::Operator`], and the one self-service admin action
//! (`POST /api/auth/bootstrap/disable`) requires [`crate::auth::Role::Admin`].
//! [`router`] builds its own `Arc<AuthState>` + `Arc<BootstrapTokenState>` from
//! the environment and layers them as `Extension`s over the whole sub-router —
//! see `crate::auth`'s module doc (in particular its "Bootstrap admin token"
//! section) for the full login story, including the temporary,
//! disable-able exception it documents to spec Decision 8 while no GitHub
//! OAuth app exists yet. `enroll::approve`/`reject`'s audit actor is now the
//! real logged-in principal (`Extension<auth::Session>`, inserted by
//! `auth::require_role`'s middleware) instead of a placeholder string.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::{
    extract::{Extension, Path as AxumPath, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};

use crate::audit::{self, AuditStore, MemAuditStore};
use crate::auth::{
    self, AuthConfig, AuthState, BootstrapTokenState, GithubApi, RealGithubApi, Role,
};
use crate::ca::InstallCa;
use crate::db::{
    store::{read_snapshot, write_snapshot, StatePaths},
    AuditEventRow as DbAuditEventRow, BootTarget, EnrollmentRow as DbEnrollmentRow,
    MachineRow as DbMachineRow, MachineStatus,
};
use crate::enroll::{self, EnrollmentStore, MemEnrollmentStore};
use crate::machine_plane::lifecycle::normalize_mac;
use ring::rand::SecureRandom;

use super::api_types::{ApiErrorBody, AuditVerifyResult, MachineRow};

/// Webroot base for placed cloud-init configs (mirrors `machine_plane::seeds`'
/// `CLOUD_INIT_BASE`; duplicated per-file — see that module's REUSE note).
const CLOUD_INIT_BASE: &str = "/var/www/html/cloud-init";
/// Install CA persistence dir (mirrors `crate::ca::InstallCa::load_or_create`'s
/// own doc comment for its production default).
const CA_DIR: &str = "/var/lib/uaa/ca";
/// Where a freshly generated bootstrap token is also written (0600) so a human
/// with SSH/server access can read it without grepping the service log —
/// mirrors [`crate::auth::AuthConfig::state_dir`]'s HMAC-key file convention.
const BOOTSTRAP_TOKEN_FILE: &str = "operator-bootstrap-token";

// ── Registry seam (read + narrow write; mockable) ────────────────────────

#[async_trait::async_trait]
pub trait Registry: Send + Sync {
    async fn list_machines(&self) -> Vec<DbMachineRow>;
    async fn get_machine(&self, mac: &str) -> Option<DbMachineRow>;
    async fn upsert_machine(&self, machine: DbMachineRow);
    async fn approve_machine(&self, mac: &str, approved_at: String) -> Option<DbMachineRow>;
}

/// Real [`Registry`]: the SAME on-disk snapshot `machine_plane::{seeds,lifecycle}`
/// read/write, so a machine visible here is visible everywhere else too.
pub struct FileRegistry {
    paths: StatePaths,
}

impl FileRegistry {
    pub fn new(paths: StatePaths) -> Self {
        Self { paths }
    }
}

#[async_trait::async_trait]
impl Registry for FileRegistry {
    async fn list_machines(&self) -> Vec<DbMachineRow> {
        read_snapshot(&self.paths).machines
    }

    async fn get_machine(&self, mac: &str) -> Option<DbMachineRow> {
        read_snapshot(&self.paths)
            .machines
            .into_iter()
            .find(|m| m.mac == mac)
    }

    async fn upsert_machine(&self, machine: DbMachineRow) {
        let mut doc = read_snapshot(&self.paths);
        match doc.machines.iter_mut().find(|m| m.mac == machine.mac) {
            Some(existing) => *existing = machine,
            None => doc.machines.push(machine),
        }
        if let Err(err) = write_snapshot(&self.paths, &doc) {
            tracing::error!(%err, "failed to persist machine snapshot");
        }
    }

    async fn approve_machine(&self, mac: &str, approved_at: String) -> Option<DbMachineRow> {
        let mut doc = read_snapshot(&self.paths);
        let row = doc.machines.iter_mut().find(|m| m.mac == mac)?;
        row.status = MachineStatus::Approved;
        row.approved_at = Some(approved_at);
        let updated = row.clone();
        if let Err(err) = write_snapshot(&self.paths, &doc) {
            tracing::error!(%err, "failed to persist machine approval");
        }
        Some(updated)
    }
}

// ── Placed-config backfill (constellation addition) ──────────────────────
//
// "I'd like them all to be there if we have a config already" — a hexmac
// directory with a placed uaa.yaml means an operator already prepared that
// machine, even if it never contacted the wire and nobody ran
// `/api/register`. Every such hexmac without a matching registry row is
// upserted here as a durable `Seen` row (hostname parsed from the config's
// own `hostname:` field when present) so it shows up and is approvable —
// the same treatment `machine_plane::seeds::record_seen_mac` gives MACs that
// DO make contact.

/// `true` iff `name` is exactly 12 lowercase hex digits (the hexmac
/// directory-name convention; duplicated from `machine_plane::dashboard`).
fn is_hexmac_dirname(name: &str) -> bool {
    name.len() == 12
        && name
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Strip separators to the `<hexmac>` form (duplicated per-file, see
/// `machine_plane::inventory`'s REUSE note).
fn mac_to_hex(mac: &str) -> String {
    mac.to_lowercase().replace([':', '-', '.'], "")
}

/// Reconstruct a colon-separated MAC from a 12-hex-digit directory name —
/// the inverse of [`mac_to_hex`], lossless because the hexmac convention is
/// just separator-stripping.
fn hexmac_to_mac(hexmac: &str) -> Option<String> {
    if hexmac.len() != 12 || !hexmac.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let parts: Vec<&str> = (0..12).step_by(2).map(|i| &hexmac[i..i + 2]).collect();
    Some(parts.join(":"))
}

/// Best-effort `hostname:` extraction from a placed `uaa.yaml` (non-secret
/// operational metadata — never parses or exposes the rest of the file).
/// Deliberately a line scan, not a YAML parser: this is a display nicety,
/// not a config consumer, and a stray `# hostname: foo` comment line never
/// matches (comments don't start with `hostname:` after trimming).
fn parse_yaml_hostname(data: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(data);
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("hostname:") {
            let v = rest.trim().trim_matches('"').trim_matches('\'');
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    None
}

/// Every `<hexmac>` directory under `base` with a placed `uaa.yaml`, paired
/// with its best-effort parsed hostname. A missing root is an empty list,
/// not an error (mirrors `machine_plane::dashboard::collect_uaa_configs`).
fn placed_config_hexmacs(base: &Path) -> Vec<(String, Option<String>)> {
    let mut names: Vec<String> = match std::fs::read_dir(base) {
        Ok(entries) => entries
            .flatten()
            .filter_map(|e| e.file_name().into_string().ok())
            .collect(),
        Err(_) => return Vec::new(),
    };
    names.sort();

    let mut out = Vec::new();
    for name in names {
        if !is_hexmac_dirname(&name) {
            continue;
        }
        let fpath = base.join(&name).join("uaa.yaml");
        let data = match std::fs::read(&fpath) {
            Ok(d) => d,
            Err(_) => continue,
        };
        out.push((name.clone(), parse_yaml_hostname(&data)));
    }
    out
}

/// Upsert a `Seen` row for every placed config not already in `known`
/// (hexmac form). Never touches an existing row — only fills gaps.
async fn backfill_placed_configs(
    registry: &dyn Registry,
    webroot: &Path,
    known: &mut HashSet<String>,
) {
    for (hexmac, hostname) in placed_config_hexmacs(webroot) {
        if known.contains(&hexmac) {
            continue;
        }
        let Some(mac) = hexmac_to_mac(&hexmac) else {
            continue;
        };
        registry
            .upsert_machine(DbMachineRow {
                mac,
                hostname: hostname.unwrap_or_default(),
                ip: None,
                r#type: String::new(),
                status: MachineStatus::Seen,
                boot_target: BootTarget::LocalDisk,
                tpm_ek: None,
                registered_at: None,
                approved_at: None,
                last_seen: None,
                last_ip: None,
                installed_at: None,
                last_install_status: None,
                updated_at: None,
            })
            .await;
        known.insert(hexmac);
    }
}

/// `claimed_hostname` isn't stored on the row — it's re-derived from the
/// CSR's own DNS SAN (see [`enroll::hostname_from_csr`]'s doc). A malformed
/// stored CSR (should never happen — `submit_csr` rejects one that doesn't
/// parse) falls back to an empty string rather than failing the whole list.
fn to_enrollment_view(row: &DbEnrollmentRow) -> super::api_types::EnrollmentRow {
    let claimed_hostname = enroll::hostname_from_csr(&row.csr_pem).unwrap_or_default();
    super::api_types::EnrollmentRow {
        spki_fingerprint: row.spki_fingerprint.clone(),
        claimed_mac: row.mac.clone().unwrap_or_default(),
        claimed_hostname,
        state: row.state.clone().into(),
        first_seen: row.requested_at.clone().unwrap_or_default(),
    }
}

fn to_audit_view(row: &DbAuditEventRow) -> super::api_types::AuditEventRow {
    super::api_types::AuditEventRow {
        seq: row.seq,
        actor: row.actor.clone(),
        action: row.action.clone(),
        outcome: row.outcome.clone(),
        timestamp: row.at.clone().unwrap_or_default(),
        detail: row.detail.as_ref().map(|v| v.to_string()),
    }
}

fn internal_error(what: &str) -> Response {
    json_response(
        StatusCode::INTERNAL_SERVER_ERROR,
        ApiErrorBody {
            message: format!("{what} failed"),
        },
    )
}

fn to_view(row: &DbMachineRow) -> MachineRow {
    MachineRow {
        mac: row.mac.clone(),
        hostname: row.hostname.clone(),
        status: row.status.clone().into(),
        boot_target: row.boot_target.clone().into(),
        tpm_ek: row.tpm_ek.clone(),
        // PLACEHOLDER — see api_types::MachineRow::consistent doc.
        consistent: true,
        last_seen: row.last_seen.clone().unwrap_or_default(),
    }
}

fn now_epoch_string() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
        .to_string()
}

// ── HTTP helpers ──────────────────────────────────────────────────────────

fn json_response<T: serde::Serialize>(code: StatusCode, body: T) -> Response {
    (code, Json(body)).into_response()
}

fn not_implemented(what: &str) -> Response {
    json_response(
        StatusCode::NOT_IMPLEMENTED,
        ApiErrorBody {
            message: format!("{what} is not yet wired to the operator API"),
        },
    )
}

fn not_found(message: &str) -> Response {
    json_response(
        StatusCode::NOT_FOUND,
        ApiErrorBody {
            message: message.to_string(),
        },
    )
}

// ── Router / handler wiring ────────────────────────────────────────────

#[derive(Clone)]
struct AppState {
    webroot: Arc<PathBuf>,
    registry: Arc<dyn Registry>,
    enrollment_store: Arc<dyn EnrollmentStore>,
    audit_store: Arc<dyn AuditStore>,
    /// The install CA is loaded lazily, per-approval (see
    /// `handle_approve_enrollment`) rather than once here — this keeps
    /// router/state construction side-effect-free (every other field here
    /// is; matches the rest of this crate's `default_state()` functions),
    /// and means a CA-directory problem (permissions, corrupt key) fails
    /// only the specific approval request, not the whole operator plane
    /// (which also serves `/api/machines`, `/healthz`, etc. — those have no
    /// reason to go down over an enrollment-signing concern).
    ca_dir: Arc<PathBuf>,
}

fn default_state() -> AppState {
    AppState {
        webroot: Arc::new(PathBuf::from(CLOUD_INIT_BASE)),
        registry: Arc::new(FileRegistry::new(StatePaths::default())),
        enrollment_store: Arc::new(MemEnrollmentStore::new()),
        audit_store: Arc::new(MemAuditStore::new()),
        ca_dir: Arc::new(PathBuf::from(CA_DIR)),
    }
}

/// Builds the CT-03 auth backend from the environment (`UAA_GITHUB_*`,
/// `UAA_STATE_DIR`). Safe to call even with no GitHub OAuth app configured yet
/// (`client_id`/`client_secret` empty) — `RealGithubApi` is only ever invoked
/// from a real `/auth/callback` round trip, which can't complete without those
/// set.
///
/// If the HMAC signing key can't be loaded/created (e.g. `state_dir` isn't
/// writable), falls back to a fresh random in-memory-only key rather than
/// failing router construction — matching this module's existing convention
/// of keeping router/state construction side-effect-free-on-failure (see
/// [`AppState::ca_dir`]'s doc comment for the same reasoning applied to CA
/// loading). The degraded mode is a working plane whose sessions don't
/// survive a restart, not a plane that fails to start; it's also what makes
/// [`router`] callable in this crate's own tests, which run with no
/// `/var/lib/uaa` access.
fn default_auth_state() -> Arc<AuthState> {
    let config = AuthConfig::from_env();
    let hmac_key = auth::load_or_create_hmac_key(&config.state_dir).unwrap_or_else(|err| {
        tracing::error!(
            %err,
            state_dir = %config.state_dir.display(),
            "failed to load or create the operator-auth HMAC key; falling back to an \
             ephemeral in-memory key (existing sessions will not survive a restart)"
        );
        let mut key = [0u8; 32];
        ring::rand::SystemRandom::new()
            .fill(&mut key)
            .expect("system RNG unavailable");
        key
    });
    let github: Arc<dyn GithubApi> = Arc::new(RealGithubApi::new(
        config.client_id.clone(),
        config.client_secret.clone(),
        config.org.clone(),
    ));
    AuthState::new(config, github, hmac_key)
}

/// Builds the bootstrap-token stopgap (see `crate::auth`'s module doc) and, if
/// enabled, generates the process's one outstanding token — logging it and
/// writing it to [`BOOTSTRAP_TOKEN_FILE`] (0600) so an operator with server
/// access can retrieve it without grepping logs.
fn default_bootstrap_state(auth_state: &AuthState) -> Arc<BootstrapTokenState> {
    let state_dir = auth_state.config().state_dir.clone();
    let bootstrap = Arc::new(BootstrapTokenState::from_env(&state_dir));
    if let Some(token) = bootstrap.generate() {
        tracing::warn!(
            %token,
            "operator plane bootstrap admin token generated (single-use, 15-minute TTL) \
             — POST {{\"token\": \"...\"}} to /api/auth/bootstrap to log in as admin until \
             a real GitHub OAuth app is configured (UAA_GITHUB_CLIENT_ID/_SECRET)"
        );
        if let Err(err) = write_bootstrap_token_file(&state_dir, &token) {
            tracing::error!(%err, "failed to write bootstrap token file");
        }
    }
    bootstrap
}

fn write_bootstrap_token_file(state_dir: &Path, token: &str) -> std::io::Result<()> {
    let path = state_dir.join(BOOTSTRAP_TOKEN_FILE);
    std::fs::write(&path, format!("{token}\n"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

/// The operator API sub-router, mounted under `/api/*`. Merged ahead of
/// [`super::web_ui::router`]'s fallback so API paths are matched first.
pub fn router() -> Router {
    let auth_state = default_auth_state();
    let bootstrap_state = default_bootstrap_state(&auth_state);
    build_router(default_state(), auth_state, bootstrap_state)
}

/// Splits routes into four groups by the minimum [`Role`] they require (public,
/// Viewer, Operator, Admin — see this module's `# Auth` doc section), finalizes
/// each to `Router<()>` independently (some need [`AppState`], some
/// `Arc<AuthState>`, some neither), then merges and layers the auth `Extension`s
/// globally so `auth::require_role`'s middleware can find them on every route.
fn build_router(
    state: AppState,
    auth_state: Arc<AuthState>,
    bootstrap_state: Arc<BootstrapTokenState>,
) -> Router {
    let public = Router::new()
        .route("/healthz", get(handle_healthz))
        .route("/api/auth/status", get(auth::auth_status_handler))
        .route("/api/auth/bootstrap", post(auth::bootstrap_login_handler))
        .with_state(state.clone());

    let oauth = Router::new()
        .route("/auth/login", get(auth::login_handler))
        .route("/auth/callback", get(auth::callback_handler))
        .with_state(auth_state.clone());

    let viewer_routes = auth::require_role(
        Router::new()
            .route("/api/machines", get(handle_list_machines))
            .route("/api/machines/:mac", get(handle_get_machine))
            .route("/api/enrollments", get(handle_list_enrollments))
            .route("/api/discovered", get(handle_list_discovered))
            .route("/api/audit", get(handle_list_audit))
            .route("/api/audit/verify", get(handle_verify_audit))
            .with_state(state.clone()),
        Role::Viewer,
    );

    let operator_routes = auth::require_role(
        Router::new()
            .route("/api/machines/:mac/approve", post(handle_approve_machine))
            .route(
                "/api/machines/:mac/reinstall",
                post(handle_reinstall_machine),
            )
            .route(
                "/api/enrollments/:fp/approve",
                post(handle_approve_enrollment),
            )
            .route(
                "/api/enrollments/:fp/reject",
                post(handle_reject_enrollment),
            )
            .route(
                "/api/discovered/:mac/dismiss",
                post(handle_dismiss_discovered),
            )
            .with_state(state),
        Role::Operator,
    );

    // No AppState needed — stays `Router<()>` without an explicit `with_state`.
    let admin_routes = auth::require_role(
        Router::new().route(
            "/api/auth/bootstrap/disable",
            post(auth::disable_bootstrap_handler),
        ),
        Role::Admin,
    );

    public
        .merge(oauth)
        .merge(viewer_routes)
        .merge(operator_routes)
        .merge(admin_routes)
        .layer(Extension(auth_state))
        .layer(Extension(bootstrap_state))
}

/// `GET /healthz` — matched here (ahead of `web_ui`'s SPA catch-all
/// fallback) so it keeps returning the same JSON shape every other plane's
/// `listeners::health_router` does, instead of silently falling through to
/// `index.html` once the SPA fallback swallows every unmatched path.
async fn handle_healthz(State(_state): State<AppState>) -> Response {
    json_response(
        StatusCode::OK,
        serde_json::json!({ "service": "uaa-control", "listener": "operator" }),
    )
}

// ── /api/machines (real) ──────────────────────────────────────────────

async fn handle_list_machines(State(state): State<AppState>) -> Response {
    let mut known: HashSet<String> = state
        .registry
        .list_machines()
        .await
        .iter()
        .map(|m| mac_to_hex(&m.mac))
        .collect();
    backfill_placed_configs(state.registry.as_ref(), &state.webroot, &mut known).await;

    let mut machines = state.registry.list_machines().await;
    machines.sort_by(|a, b| a.mac.cmp(&b.mac));
    let views: Vec<MachineRow> = machines.iter().map(to_view).collect();
    json_response(StatusCode::OK, views)
}

async fn handle_get_machine(
    State(state): State<AppState>,
    AxumPath(mac_raw): AxumPath<String>,
) -> Response {
    let mac = normalize_mac(&mac_raw);
    match state.registry.get_machine(&mac).await {
        Some(row) => json_response(StatusCode::OK, to_view(&row)),
        None => not_found("machine not found"),
    }
}

async fn handle_approve_machine(
    State(state): State<AppState>,
    AxumPath(mac_raw): AxumPath<String>,
) -> Response {
    let mac = normalize_mac(&mac_raw);
    match state
        .registry
        .approve_machine(&mac, now_epoch_string())
        .await
    {
        Some(row) => {
            tracing::info!(%mac, hostname = %row.hostname, "OPERATOR APPROVED");
            StatusCode::NO_CONTENT.into_response()
        }
        None => not_found("machine not found"),
    }
}

async fn handle_reinstall_machine(
    State(_state): State<AppState>,
    AxumPath(_mac_raw): AxumPath<String>,
) -> Response {
    not_implemented("reinstall")
}

// ── /api/enrollments (real, against crate::enroll's state machine) ───────

async fn handle_list_enrollments(State(state): State<AppState>) -> Response {
    match state.enrollment_store.list_all().await {
        Ok(mut rows) => {
            rows.sort_by(|a, b| a.spki_fingerprint.cmp(&b.spki_fingerprint));
            let views: Vec<_> = rows.iter().map(to_enrollment_view).collect();
            json_response(StatusCode::OK, views)
        }
        Err(err) => {
            tracing::error!(%err, "failed to list enrollments");
            internal_error("listing enrollments")
        }
    }
}

async fn handle_approve_enrollment(
    State(state): State<AppState>,
    Extension(session): Extension<auth::Session>,
    AxumPath(fp): AxumPath<String>,
) -> Response {
    match state.enrollment_store.get(&fp).await {
        Ok(None) => return not_found("enrollment not registered"),
        Ok(Some(_)) => {}
        Err(err) => {
            tracing::error!(%err, %fp, "enrollment lookup failed");
            return internal_error("enrollment lookup");
        }
    }
    let ca = match InstallCa::load_or_create(&state.ca_dir) {
        Ok(ca) => ca,
        Err(err) => {
            tracing::error!(%err, ca_dir = %state.ca_dir.display(), "failed to load install CA");
            return internal_error("loading install CA");
        }
    };
    match enroll::approve(
        state.enrollment_store.as_ref(),
        &ca,
        state.audit_store.as_ref(),
        &fp,
        &session.login,
    )
    .await
    {
        Ok(row) => {
            tracing::info!(fp = %row.spki_fingerprint, "OPERATOR ENROLLMENT APPROVED");
            StatusCode::NO_CONTENT.into_response()
        }
        Err(err) => {
            tracing::error!(%err, %fp, "enrollment approval failed");
            internal_error("enrollment approval")
        }
    }
}

async fn handle_reject_enrollment(
    State(state): State<AppState>,
    Extension(session): Extension<auth::Session>,
    AxumPath(fp): AxumPath<String>,
) -> Response {
    match state.enrollment_store.get(&fp).await {
        Ok(None) => return not_found("enrollment not registered"),
        Ok(Some(_)) => {}
        Err(err) => {
            tracing::error!(%err, %fp, "enrollment lookup failed");
            return internal_error("enrollment lookup");
        }
    }
    match enroll::reject(
        state.enrollment_store.as_ref(),
        state.audit_store.as_ref(),
        &fp,
        &session.login,
    )
    .await
    {
        Ok(row) => {
            tracing::info!(fp = %row.spki_fingerprint, "OPERATOR ENROLLMENT REJECTED");
            StatusCode::NO_CONTENT.into_response()
        }
        Err(err) => {
            tracing::error!(%err, %fp, "enrollment rejection failed");
            internal_error("enrollment rejection")
        }
    }
}

// ── Stub: discovery (no backend exists anywhere in the crate yet) ────────

async fn handle_list_discovered(State(_state): State<AppState>) -> Response {
    json_response(
        StatusCode::OK,
        Vec::<super::api_types::DiscoveredMacRow>::new(),
    )
}

async fn handle_dismiss_discovered(
    State(_state): State<AppState>,
    AxumPath(_mac): AxumPath<String>,
) -> Response {
    not_implemented("discovery dismissal")
}

// ── /api/audit (real, against crate::audit's hash-chained store) ─────────

async fn handle_list_audit(State(state): State<AppState>) -> Response {
    match state.audit_store.list_events(0).await {
        Ok(events) => {
            let views: Vec<_> = events.iter().map(to_audit_view).collect();
            json_response(StatusCode::OK, views)
        }
        Err(err) => {
            tracing::error!(%err, "failed to list audit events");
            internal_error("listing audit events")
        }
    }
}

async fn handle_verify_audit(State(state): State<AppState>) -> Response {
    match state.audit_store.list_events(0).await {
        Ok(events) => {
            let checked = events.len() as i64;
            match audit::verify_chain(&events) {
                Ok(()) => json_response(
                    StatusCode::OK,
                    AuditVerifyResult {
                        ok: true,
                        checked,
                        message: None,
                    },
                ),
                Err(defect) => json_response(
                    StatusCode::OK,
                    AuditVerifyResult {
                        ok: false,
                        checked,
                        message: Some(defect.to_string()),
                    },
                ),
            }
        }
        Err(err) => {
            tracing::error!(%err, "failed to verify audit chain");
            internal_error("verifying audit chain")
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;
    use tempfile::tempdir;

    #[derive(Default)]
    struct MockRegistry {
        machines: Mutex<HashMap<String, DbMachineRow>>,
    }

    #[async_trait::async_trait]
    impl Registry for MockRegistry {
        async fn list_machines(&self) -> Vec<DbMachineRow> {
            self.machines.lock().unwrap().values().cloned().collect()
        }
        async fn get_machine(&self, mac: &str) -> Option<DbMachineRow> {
            self.machines.lock().unwrap().get(mac).cloned()
        }
        async fn upsert_machine(&self, machine: DbMachineRow) {
            self.machines
                .lock()
                .unwrap()
                .insert(machine.mac.clone(), machine);
        }
        async fn approve_machine(&self, mac: &str, approved_at: String) -> Option<DbMachineRow> {
            let mut st = self.machines.lock().unwrap();
            let row = st.get_mut(mac)?;
            row.status = MachineStatus::Approved;
            row.approved_at = Some(approved_at);
            Some(row.clone())
        }
    }

    fn base_machine(mac: &str, hostname: &str, status: MachineStatus) -> DbMachineRow {
        DbMachineRow {
            mac: mac.to_string(),
            hostname: hostname.to_string(),
            ip: Some("10.0.0.1".to_string()),
            r#type: "lenovo".to_string(),
            status,
            boot_target: BootTarget::LocalDisk,
            tpm_ek: None,
            registered_at: Some("1000".to_string()),
            approved_at: None,
            last_seen: Some("1234".to_string()),
            last_ip: None,
            installed_at: None,
            last_install_status: None,
            updated_at: None,
        }
    }

    fn test_ca() -> InstallCa {
        let dir = tempdir().unwrap();
        InstallCa::load_or_create(&dir.path().join("ca")).unwrap()
    }

    fn test_state(webroot: PathBuf, registry: Arc<dyn Registry>) -> AppState {
        // Subdir of the SAME tempdir the caller already keeps alive for the
        // test's duration — `handle_approve_enrollment` loads the CA lazily
        // per-request now, so this path must still exist when that runs.
        let ca_dir = webroot.join("ca");
        AppState {
            webroot: Arc::new(webroot),
            registry,
            enrollment_store: Arc::new(MemEnrollmentStore::new()),
            audit_store: Arc::new(MemAuditStore::new()),
            ca_dir: Arc::new(ca_dir),
        }
    }

    /// Same as [`test_state`] but shares a caller-supplied enrollment/audit
    /// store pair — needed by tests that assert on state the handler wrote
    /// (e.g. an approve/reject transition, or a resulting audit event).
    fn test_state_with_stores(
        webroot: PathBuf,
        registry: Arc<dyn Registry>,
        enrollment_store: Arc<dyn EnrollmentStore>,
        audit_store: Arc<dyn AuditStore>,
    ) -> AppState {
        let ca_dir = webroot.join("ca");
        AppState {
            webroot: Arc::new(webroot),
            registry,
            enrollment_store,
            audit_store,
            ca_dir: Arc::new(ca_dir),
        }
    }

    /// A stand-in authenticated principal for tests that call a protected
    /// handler function directly (bypassing the router, so `auth::require_role`
    /// never runs to insert a real one).
    fn test_session() -> Extension<auth::Session> {
        Extension(auth::Session {
            login: "test-operator".to_string(),
            role: Role::Operator,
        })
    }

    async fn body_json(resp: Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[test]
    fn test_router_builds_standalone() {
        // Constructing the router touches no filesystem (`ca_dir` is only
        // read at approve-request time, not here) — only requests do.
        let _ = router();
    }

    #[test]
    fn test_hexmac_to_mac_roundtrips_with_mac_to_hex() {
        assert_eq!(
            hexmac_to_mac("ac1f6b40fce2").as_deref(),
            Some("ac:1f:6b:40:fc:e2")
        );
        assert_eq!(mac_to_hex("ac:1f:6b:40:fc:e2"), "ac1f6b40fce2");
        assert_eq!(hexmac_to_mac("bad"), None);
        assert_eq!(
            hexmac_to_mac("zzzzzzzzzzzz"),
            None,
            "non-hex must not parse"
        );
    }

    #[test]
    fn test_parse_yaml_hostname_ignores_comments() {
        let data = b"# hostname: not-this-one\nhostname: unimatrixone\ndisk_device: /dev/md126\n";
        assert_eq!(parse_yaml_hostname(data).as_deref(), Some("unimatrixone"));
        assert_eq!(parse_yaml_hostname(b"disk_device: /dev/sda\n"), None);
    }

    #[tokio::test]
    async fn test_list_machines_backfills_placed_config_with_parsed_hostname() {
        let dir = tempdir().unwrap();
        let hex_dir = dir.path().join("ac1f6b40fce2");
        std::fs::create_dir_all(&hex_dir).unwrap();
        std::fs::write(
            hex_dir.join("uaa.yaml"),
            b"hostname: unimatrixone\ndisk_device: /dev/md126\n",
        )
        .unwrap();

        let registry = Arc::new(MockRegistry::default());
        let state = test_state(dir.path().to_path_buf(), registry.clone());

        let resp = handle_list_machines(State(state)).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp).await;
        let arr = body.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["mac"], "ac:1f:6b:40:fc:e2");
        assert_eq!(arr[0]["hostname"], "unimatrixone");
        assert_eq!(arr[0]["status"], "seen");

        // Persisted, not just returned — a second call must not duplicate it.
        let row = registry.get_machine("ac:1f:6b:40:fc:e2").await.unwrap();
        assert_eq!(row.status, MachineStatus::Seen);
    }

    #[tokio::test]
    async fn test_list_machines_backfill_never_overwrites_existing_row() {
        let dir = tempdir().unwrap();
        let hex_dir = dir.path().join("aabbccddeeff");
        std::fs::create_dir_all(&hex_dir).unwrap();
        std::fs::write(hex_dir.join("uaa.yaml"), b"hostname: should-be-ignored\n").unwrap();

        let registry = Arc::new(MockRegistry::default());
        registry
            .upsert_machine(base_machine(
                "aa:bb:cc:dd:ee:ff",
                "real-hostname",
                MachineStatus::Approved,
            ))
            .await;
        let state = test_state(dir.path().to_path_buf(), registry.clone());

        let resp = handle_list_machines(State(state)).await;
        let body = body_json(resp).await;
        let arr = body.as_array().unwrap();
        assert_eq!(arr.len(), 1, "existing row must not be duplicated");
        assert_eq!(
            arr[0]["hostname"], "real-hostname",
            "existing row must not be overwritten"
        );
        assert_eq!(arr[0]["status"], "approved");
    }

    #[tokio::test]
    async fn test_get_machine_not_found_404() {
        let dir = tempdir().unwrap();
        let state = test_state(dir.path().to_path_buf(), Arc::new(MockRegistry::default()));
        let resp =
            handle_get_machine(State(state), AxumPath("aa:bb:cc:dd:ee:ff".to_string())).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_approve_machine_sets_status_and_returns_204() {
        let dir = tempdir().unwrap();
        let registry = Arc::new(MockRegistry::default());
        registry
            .upsert_machine(base_machine("aa:bb:cc:dd:ee:ff", "h1", MachineStatus::Seen))
            .await;
        let state = test_state(dir.path().to_path_buf(), registry.clone());

        let resp =
            handle_approve_machine(State(state), AxumPath("aa:bb:cc:dd:ee:ff".to_string())).await;
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        let row = registry.get_machine("aa:bb:cc:dd:ee:ff").await.unwrap();
        assert_eq!(row.status, MachineStatus::Approved);
    }

    #[tokio::test]
    async fn test_approve_unknown_machine_404() {
        let dir = tempdir().unwrap();
        let state = test_state(dir.path().to_path_buf(), Arc::new(MockRegistry::default()));
        let resp =
            handle_approve_machine(State(state), AxumPath("aa:bb:cc:dd:ee:ff".to_string())).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_reinstall_stubbed_501() {
        let dir = tempdir().unwrap();
        let state = test_state(dir.path().to_path_buf(), Arc::new(MockRegistry::default()));
        let resp =
            handle_reinstall_machine(State(state), AxumPath("aa:bb:cc:dd:ee:ff".to_string())).await;
        assert_eq!(resp.status(), StatusCode::NOT_IMPLEMENTED);
    }

    #[tokio::test]
    async fn test_empty_store_list_endpoints_return_empty_arrays() {
        // discovered has no backend at all (always empty); enrollments/audit
        // are real now but a fresh MemStore is legitimately empty too.
        let dir = tempdir().unwrap();
        let state = || test_state(dir.path().to_path_buf(), Arc::new(MockRegistry::default()));

        for (resp, label) in [
            (handle_list_enrollments(State(state())).await, "enrollments"),
            (handle_list_discovered(State(state())).await, "discovered"),
            (handle_list_audit(State(state())).await, "audit"),
        ] {
            assert_eq!(resp.status(), StatusCode::OK, "{label}");
            let body = body_json(resp).await;
            assert_eq!(body.as_array().unwrap().len(), 0, "{label}");
        }
    }

    // ── /api/enrollments (real) ────────────────────────────────────────

    fn fresh_enrollment_store_and_ca() -> (Arc<dyn EnrollmentStore>, InstallCa) {
        (Arc::new(MemEnrollmentStore::new()), test_ca())
    }

    async fn submit_via_state(
        enrollment_store: &Arc<dyn EnrollmentStore>,
        ca: &InstallCa,
        audit_store: &Arc<dyn AuditStore>,
        mac: &str,
        hostname: &str,
    ) -> String {
        let identity = uaa_core::pki::AgentIdentity {
            hostname: hostname.to_string(),
            mac: mac.to_string(),
        };
        let (_key, csr_pem) = uaa_core::pki::generate_keypair_and_csr(&identity).unwrap();
        let row = enroll::submit_csr(
            enrollment_store.as_ref(),
            ca,
            audit_store.as_ref(),
            &csr_pem,
            mac,
            hostname,
        )
        .await
        .unwrap();
        row.spki_fingerprint
    }

    #[tokio::test]
    async fn test_list_enrollments_maps_pending_row_to_wire_shape() {
        let dir = tempdir().unwrap();
        let (enrollment_store, ca) = fresh_enrollment_store_and_ca();
        let audit_store: Arc<dyn AuditStore> = Arc::new(MemAuditStore::new());
        let fp = submit_via_state(
            &enrollment_store,
            &ca,
            &audit_store,
            "aa:bb:cc:dd:ee:01",
            "pending-host",
        )
        .await;
        let state = test_state_with_stores(
            dir.path().to_path_buf(),
            Arc::new(MockRegistry::default()),
            enrollment_store,
            audit_store,
        );

        let resp = handle_list_enrollments(State(state)).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp).await;
        let arr = body.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["spki_fingerprint"], fp);
        assert_eq!(arr[0]["claimed_mac"], "aa:bb:cc:dd:ee:01");
        assert_eq!(arr[0]["claimed_hostname"], "pending-host");
        assert_eq!(arr[0]["state"], "pending");
    }

    #[tokio::test]
    async fn test_approve_enrollment_issues_cert_and_records_audit_event() {
        let dir = tempdir().unwrap();
        let (enrollment_store, ca) = fresh_enrollment_store_and_ca();
        let audit_store: Arc<dyn AuditStore> = Arc::new(MemAuditStore::new());
        let fp = submit_via_state(
            &enrollment_store,
            &ca,
            &audit_store,
            "aa:bb:cc:dd:ee:02",
            "approve-host",
        )
        .await;
        let state = test_state_with_stores(
            dir.path().to_path_buf(),
            Arc::new(MockRegistry::default()),
            enrollment_store.clone(),
            audit_store.clone(),
        );

        let resp =
            handle_approve_enrollment(State(state), test_session(), AxumPath(fp.clone())).await;
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);

        let row = enrollment_store.get(&fp).await.unwrap().unwrap();
        assert_eq!(row.state, crate::db::EnrollmentState::Issued);
        assert!(row.cert_pem.is_some(), "approve must set cert_pem");

        let events = audit_store.list_events(0).await.unwrap();
        assert!(
            events.iter().any(|e| e.action == "enrollment.approve"),
            "approve must be audited"
        );
    }

    #[tokio::test]
    async fn test_approve_unknown_enrollment_404() {
        let dir = tempdir().unwrap();
        let state = test_state(dir.path().to_path_buf(), Arc::new(MockRegistry::default()));
        let resp = handle_approve_enrollment(
            State(state),
            test_session(),
            AxumPath("no-such-fp".to_string()),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_reject_enrollment_sets_rejected_state() {
        let dir = tempdir().unwrap();
        let (enrollment_store, ca) = fresh_enrollment_store_and_ca();
        let audit_store: Arc<dyn AuditStore> = Arc::new(MemAuditStore::new());
        let fp = submit_via_state(
            &enrollment_store,
            &ca,
            &audit_store,
            "aa:bb:cc:dd:ee:03",
            "reject-host",
        )
        .await;
        let state = test_state_with_stores(
            dir.path().to_path_buf(),
            Arc::new(MockRegistry::default()),
            enrollment_store.clone(),
            audit_store,
        );

        let resp =
            handle_reject_enrollment(State(state), test_session(), AxumPath(fp.clone())).await;
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        let row = enrollment_store.get(&fp).await.unwrap().unwrap();
        assert_eq!(row.state, crate::db::EnrollmentState::Rejected);
    }

    #[tokio::test]
    async fn test_reject_unknown_enrollment_404() {
        let dir = tempdir().unwrap();
        let state = test_state(dir.path().to_path_buf(), Arc::new(MockRegistry::default()));
        let resp = handle_reject_enrollment(
            State(state),
            test_session(),
            AxumPath("no-such-fp".to_string()),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    // ── /api/audit (real) ──────────────────────────────────────────────

    #[tokio::test]
    async fn test_list_and_verify_audit_after_a_real_mutation() {
        let dir = tempdir().unwrap();
        let (enrollment_store, ca) = fresh_enrollment_store_and_ca();
        let audit_store: Arc<dyn AuditStore> = Arc::new(MemAuditStore::new());
        let fp = submit_via_state(
            &enrollment_store,
            &ca,
            &audit_store,
            "aa:bb:cc:dd:ee:04",
            "audit-host",
        )
        .await;
        let state = test_state_with_stores(
            dir.path().to_path_buf(),
            Arc::new(MockRegistry::default()),
            enrollment_store,
            audit_store,
        );
        let resp =
            handle_approve_enrollment(State(state.clone()), test_session(), AxumPath(fp)).await;
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);

        let list_resp = handle_list_audit(State(state.clone())).await;
        let list_body = body_json(list_resp).await;
        let events = list_body.as_array().unwrap();
        assert!(!events.is_empty());
        assert_eq!(events[0]["seq"], 1);

        let verify_resp = handle_verify_audit(State(state)).await;
        let verify_body = body_json(verify_resp).await;
        assert_eq!(verify_body["ok"], true);
        assert_eq!(verify_body["checked"], events.len() as i64);
        assert!(verify_body["message"].is_null());
    }

    #[tokio::test]
    async fn test_healthz_matched_before_spa_fallback_would_swallow_it() {
        let dir = tempdir().unwrap();
        let state = test_state(dir.path().to_path_buf(), Arc::new(MockRegistry::default()));
        let resp = handle_healthz(State(state)).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp).await;
        assert_eq!(body["service"], "uaa-control");
        assert_eq!(body["listener"], "operator");
    }

    #[tokio::test]
    async fn test_verify_audit_stub_shape() {
        let dir = tempdir().unwrap();
        let state = test_state(dir.path().to_path_buf(), Arc::new(MockRegistry::default()));
        let resp = handle_verify_audit(State(state)).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp).await;
        assert_eq!(body["ok"], true);
        assert_eq!(body["checked"], 0);
        assert!(body["message"].is_null());
    }

    // ── Router-level auth wiring (real `build_router`, real middleware) ──────
    //
    // Everything above calls handler functions directly, bypassing
    // `auth::require_role` entirely. These tests instead build the ACTUAL
    // router `router()`'s production code path constructs, and drive it with
    // `tower::ServiceExt::oneshot` — the only way to prove the middleware is
    // actually wired onto the routes it's supposed to protect, not just that
    // the handlers behave correctly when called directly.

    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    /// Builds the same router `router()` does in production, but against a
    /// tempdir (so it never touches `/var/lib/uaa`) and with the bootstrap
    /// token forced on regardless of the ambient environment, returning the
    /// one valid raw token alongside it. The tempdir is returned too so it
    /// stays alive for the test's duration.
    fn test_full_router() -> (Router, String, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let app_state = test_state(dir.path().to_path_buf(), Arc::new(MockRegistry::default()));

        let auth_config = AuthConfig {
            client_id: String::new(),
            client_secret: String::new(),
            org: "falkcorp".to_string(),
            admin_team: "uaa-admins".to_string(),
            operator_team: "uaa-operators".to_string(),
            state_dir: dir.path().to_path_buf(),
        };
        let hmac_key = auth::load_or_create_hmac_key(&auth_config.state_dir).unwrap();
        let github: Arc<dyn GithubApi> = Arc::new(RealGithubApi::new(
            String::new(),
            String::new(),
            String::new(),
        ));
        let auth_state = AuthState::new(auth_config, github, hmac_key);

        let bootstrap_state = Arc::new(BootstrapTokenState::new(dir.path(), false));
        let token = bootstrap_state.generate().expect("enabled by construction");

        let router = build_router(app_state, auth_state, bootstrap_state);
        (router, token, dir)
    }

    fn get(uri: &str) -> Request<Body> {
        Request::builder()
            .method("GET")
            .uri(uri)
            .body(Body::empty())
            .unwrap()
    }

    fn post(uri: &str) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri(uri)
            .body(Body::empty())
            .unwrap()
    }

    fn session_cookie_from(resp: &Response) -> String {
        resp.headers()
            .get_all(axum::http::header::SET_COOKIE)
            .iter()
            .find_map(|v| {
                let s = v.to_str().ok()?;
                s.starts_with("uaa_session=")
                    .then(|| s.split(';').next().unwrap().to_string())
            })
            .expect("response must set a uaa_session cookie")
    }

    #[tokio::test]
    async fn test_unauthenticated_read_is_401_not_open() {
        // Before this wiring, `/api/machines` had zero auth at all — proves
        // that gap is closed.
        let (router, _token, _dir) = test_full_router();
        let resp = router.oneshot(get("/api/machines")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_unauthenticated_approve_enrollment_is_401_not_open() {
        // THE originally-flagged critical gap: real cert issuance with zero
        // caller authentication. Proves it's closed.
        let (router, _token, _dir) = test_full_router();
        let resp = router
            .oneshot(post("/api/enrollments/some-fp/approve"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_healthz_and_auth_status_need_no_session() {
        let (router, _token, _dir) = test_full_router();
        let healthz = router.clone().oneshot(get("/healthz")).await.unwrap();
        assert_eq!(healthz.status(), StatusCode::OK);
        let status = router.oneshot(get("/api/auth/status")).await.unwrap();
        assert_eq!(status.status(), StatusCode::OK);
        let body = body_json(status).await;
        assert_eq!(body["authenticated"], false);
        assert_eq!(body["bootstrap_token_enabled"], true);
    }

    #[tokio::test]
    async fn test_bootstrap_login_then_protected_route_succeeds() {
        let (router, token, _dir) = test_full_router();

        let login_resp = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/auth/bootstrap")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::json!({"token": token}).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(login_resp.status(), StatusCode::OK);
        let cookie = session_cookie_from(&login_resp);

        // A read (Viewer-gated)...
        let read = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/machines")
                    .header("cookie", &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(read.status(), StatusCode::OK);

        // ...and a mutation (Operator-gated) both succeed under the same
        // bootstrap-minted session, proving it carries Admin (>= both).
        let mutate = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/enrollments/no-such-fp/approve")
                    .header("cookie", &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        // 404 (unknown fingerprint), NOT 401/403 — proves the session passed
        // the auth gate and reached the real handler.
        assert_eq!(mutate.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_bootstrap_login_wrong_token_401_grants_no_session() {
        let (router, _token, _dir) = test_full_router();
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/auth/bootstrap")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({"token": "uaabs_wrong"}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert!(
            resp.headers().get(axum::http::header::SET_COOKIE).is_none(),
            "a rejected bootstrap login must never set a session cookie"
        );
    }

    #[tokio::test]
    async fn test_bootstrap_disable_then_login_endpoint_stops_accepting_tokens() {
        let (router, token, _dir) = test_full_router();

        // Log in once to get an admin session capable of calling the
        // disable endpoint.
        let login_resp = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/auth/bootstrap")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::json!({"token": token}).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let cookie = session_cookie_from(&login_resp);

        let disable_resp = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/auth/bootstrap/disable")
                    .header("cookie", &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(disable_resp.status(), StatusCode::OK);

        let status = router.oneshot(get("/api/auth/status")).await.unwrap();
        let body = body_json(status).await;
        assert_eq!(body["bootstrap_token_enabled"], false);
    }
}
