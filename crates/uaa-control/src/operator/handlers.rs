// file: crates/uaa-control/src/operator/handlers.rs
// version: 1.9.0
// guid: e94ff17e-4e1b-4672-8940-1fe111b56861
// last-edited: 2026-07-19

//! Operator API request handlers (`:15000`, mounted under `/api/*` ahead of
//! [`super::web_ui`]'s SPA fallback).
//!
//! This is a first vertical slice, not the full CT-07 scope: `GET
//! /api/machines` (+ single-machine GET, + approve) is real, backed by the
//! same CT-01 snapshot `machine_plane::{seeds,lifecycle}` read/write.
//! Enrollments (`GET /api/enrollments`, approve, reject) and audit (`GET
//! /api/audit`, `GET /api/audit/verify`) are ALSO now real — wired against
//! PK-01's `crate::enroll` state machine and CT-01's `crate::audit` chain,
//! the same logic + tests that already existed, just not previously exposed
//! over HTTP. Discovery (`GET /api/discovered`, dismiss) is now backed by
//! `crate::discovered`'s file store (`discovered-macs.json`), fed by the
//! dnsmasq-journal follower's ingest on the machine plane.
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

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::{
    extract::{Extension, Path as AxumPath, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::audit::{self, AuditStore, MemAuditStore};
use crate::auth::{
    self, AuthConfig, AuthState, BootstrapTokenState, GithubApi, RealGithubApi, Role,
};
use crate::ca::InstallCa;
use crate::db::{
    store::{read_snapshot, write_snapshot, StatePaths},
    AuditEventRow as DbAuditEventRow, BootTarget, EnrollmentRow as DbEnrollmentRow,
    HostGroupRow, HostProfileRow, HostnameAllocationRow, MachineRow as DbMachineRow,
    MachineStatus,
};
use crate::enroll::{self, EnrollmentStore, MemEnrollmentStore};
use crate::machine_plane::lifecycle::normalize_mac;
use crate::profiles::drift;
use crate::profiles::store::{ProfileStore, SnapshotProfileStore};
use ring::rand::SecureRandom;
use uaa_core::network::ssh_installer::config::ApplicationSpec;
use uaa_core::profile::validate::validate as validate_profiles;
use uaa_core::profile::{HostGroupProfile, HostProfile, InstallationConfigPartial};

use crate::profiles::convert::{group_row_to_profile, profile_row_to_profile};

use super::api_types::{
    AllocationView, ApiErrorBody, AuditVerifyResult, DriftView, HostGroupView, HostProfileView,
    MachineRow, ReviewResultView,
};

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
                app_reports: Vec::new(),
                last_app_status_at: None,
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

/// A write failed `profile::validate` (DS-PRF-03). The message already names
/// EVERY violated rule, joined by `; ` — see that function's doc — never just
/// the first, so a weak operator doesn't fix one error per round-trip. Also
/// used for the profile routes' other 400s (immutable rename, undeletable
/// standalone group, unbound `rebind` identity) — same status, same body
/// shape, just a caller-supplied message instead of `validate`'s.
fn validation_error(message: impl Into<String>) -> Response {
    json_response(
        StatusCode::BAD_REQUEST,
        ApiErrorBody {
            message: message.into(),
        },
    )
}

/// The profile store could not be read (spec: fail CLOSED, never an empty
/// list — see `AppState::profile_store`'s doc and `profiles::store`'s module
/// doc). Deliberately distinct from [`internal_error`]'s 500: 503 tells a
/// caller/retry-loop this is a transient availability problem with the
/// store, not a bug in the request it sent.
fn store_unavailable(what: &str) -> Response {
    json_response(
        StatusCode::SERVICE_UNAVAILABLE,
        ApiErrorBody {
            message: format!("{what}: profile store is unavailable"),
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
    /// DS-OPS-01. Deliberately NEVER the in-memory test double from
    /// `profiles::store` in production — that type is
    /// `#[cfg(test)]`-gated for exactly this reason (see that module's
    /// doc): an empty profile store is a fleet-wide-rename bug waiting to
    /// happen, so a store construction failure must never "degrade" to
    /// one. `SnapshotProfileStore::new` itself cannot fail (it does no I/O
    /// at construction — see its doc); what CAN fail is a later read/write
    /// through it, which every profile handler below surfaces as a 503 to
    /// the request that hit it, never a router-construction-time failure
    /// (same isolate-failure-per-request principle as `ca_dir` above).
    profile_store: Arc<dyn ProfileStore>,
}

fn default_state() -> AppState {
    AppState {
        webroot: Arc::new(PathBuf::from(CLOUD_INIT_BASE)),
        registry: Arc::new(FileRegistry::new(StatePaths::default())),
        enrollment_store: Arc::new(MemEnrollmentStore::new()),
        audit_store: Arc::new(MemAuditStore::new()),
        ca_dir: Arc::new(PathBuf::from(CA_DIR)),
        profile_store: Arc::new(SnapshotProfileStore::new(StatePaths::default())),
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
/// enabled, generates the process's one outstanding token and writes it to
/// [`BOOTSTRAP_TOKEN_FILE`] (0600) so an operator with server access can
/// retrieve it. The raw token is deliberately NEVER logged — this file is the
/// only place it's written to, since log output (journald, any shipped log
/// aggregation) is a much wider-reach, longer-retained, less access-controlled
/// surface than one 0600 file.
fn default_bootstrap_state(auth_state: &AuthState) -> Arc<BootstrapTokenState> {
    let state_dir = auth_state.config().state_dir.clone();
    let bootstrap = Arc::new(BootstrapTokenState::from_env(&state_dir));
    if let Some(token) = bootstrap.generate() {
        let path = state_dir.join(BOOTSTRAP_TOKEN_FILE);
        match write_bootstrap_token_file(&path, &token) {
            Ok(()) => {
                tracing::warn!(
                    path = %path.display(),
                    "operator plane bootstrap admin token generated (single-use, 15-minute \
                     TTL); read the token from this file and POST {{\"token\": \"...\"}} to \
                     /api/auth/bootstrap to log in as admin until a real GitHub OAuth app is \
                     configured (UAA_GITHUB_CLIENT_ID/_SECRET)"
                );
            }
            Err(err) => {
                tracing::error!(%err, path = %path.display(), "failed to write bootstrap token file");
            }
        }
    }
    bootstrap
}

/// Writes `token` to `path` at 0600, set atomically at file-creation time (no
/// window where it's briefly world/group-readable) rather than via a
/// write-then-chmod sequence. Removes any pre-existing file at `path` first
/// so a local attacker who planted a symlink there can't redirect the write;
/// `create_new` then fails closed (rather than silently following a symlink)
/// if anything reappears at that path between the removal and the open.
fn write_bootstrap_token_file(path: &Path, token: &str) -> std::io::Result<()> {
    use std::io::Write as _;

    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(err),
    }

    let mut open_options = std::fs::OpenOptions::new();
    open_options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        open_options.mode(0o600);
    }
    let mut file = open_options.open(path)?;
    file.write_all(format!("{token}\n").as_bytes())
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
            .route("/api/groups", get(handle_list_groups))
            .route("/api/groups/:name", get(handle_get_group))
            .route(
                "/api/groups/:name/profiles",
                get(handle_list_group_profiles),
            )
            .route(
                "/api/groups/:name/allocations",
                get(handle_list_group_allocations),
            )
            .route("/api/drift", get(handle_list_drift))
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
            .route(
                "/api/groups",
                post(handle_create_group),
            )
            .route(
                "/api/groups/:name",
                axum::routing::put(handle_update_group).delete(handle_delete_group),
            )
            .route(
                "/api/groups/:name/profiles",
                post(handle_create_group_profile),
            )
            .route("/api/groups/:name/rebind", post(handle_rebind))
            .route("/api/drift/:object_id/accept", post(handle_accept_drift))
            .route("/api/drift/:object_id/revert", post(handle_revert_drift))
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

// ── /api/discovered (real, against crate::discovered's file-backed inbox) ──
//
// Backed by `discovered-macs.json`, populated on the machine plane (:25000) by
// the ARP/NDP neighbor-table scanner's `POST /api/discovered`. See `crate::discovered`.

async fn handle_list_discovered(State(_state): State<AppState>) -> Response {
    json_response(StatusCode::OK, crate::discovered::DiscoveredStore::default().list())
}

async fn handle_dismiss_discovered(
    State(_state): State<AppState>,
    AxumPath(mac): AxumPath<String>,
) -> Response {
    if crate::discovered::DiscoveredStore::default().dismiss(&mac) {
        StatusCode::NO_CONTENT.into_response()
    } else {
        not_found("discovered MAC not found")
    }
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

// ── /api/groups + /api/profiles (DS-OPS-01) ───────────────────────────────
//
// CRUD over `HostGroupRow`/`HostProfileRow` plus `rebind`, wired through
// `ProfileStore` (DS-REG-02/03). Reads at `Role::Viewer`, mutations at
// `Role::Operator` — see `build_router`. Every write runs `profile::validate`
// (DS-PRF-03) over the FULL groups+profiles snapshot (not just the row being
// written) because `validate`'s load-bearing rule,
// `check_global_hostname_uniqueness`, spans every group — see that
// function's doc.

/// Request body for `POST /api/groups` and `PUT /api/groups/:name`.
/// Deliberately typed as `InstallationConfigPartial`/`Vec<ApplicationSpec>`
/// (not `serde_json::Value`) so a malformed `defaults`/`applications`
/// payload is rejected at the JSON-body-parsing layer with a precise serde
/// error before `profile::validate` ever runs. Lives next to the handler
/// that parses it, not in `api_types.rs` — that module is response-view-only
/// (see its doc); mirrors `auth::BootstrapLoginBody`'s convention.
#[derive(Debug, Deserialize)]
struct GroupWriteBody {
    name: String,
    hostname_pattern: String,
    is_standalone: bool,
    #[serde(default)]
    defaults: InstallationConfigPartial,
    #[serde(default)]
    applications: Vec<ApplicationSpec>,
}

/// Request body for `POST /api/groups/:name/profiles`.
#[derive(Debug, Deserialize)]
struct ProfileWriteBody {
    identity: String,
    #[serde(default)]
    hostname_override: Option<String>,
    #[serde(default)]
    overrides: InstallationConfigPartial,
    #[serde(default)]
    applications: Vec<ApplicationSpec>,
}

/// Request body for `POST /api/groups/:name/rebind` (spec D18 — the
/// NIC-replacement runbook).
#[derive(Debug, Deserialize)]
struct RebindBody {
    old_identity: String,
    new_identity: String,
}

/// Current wall-clock time as an RFC3339 string, for `created_at`/`updated_at`
/// stamps. Duplicated from `profiles::store`'s private `now_rfc3339` — see
/// this crate's established per-file-duplication convention (e.g.
/// `CLOUD_INIT_BASE`, `mac_to_hex` above) rather than exporting a one-line
/// helper across a module boundary.
fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// A simple SHA-256 content hash over `value`'s canonical JSON encoding, for
/// `HostGroupRow`/`HostProfileRow::content_hash` (change-detection metadata,
/// not a security boundary — nothing authenticates against this hash).
fn content_hash<T: serde::Serialize>(value: &T) -> Vec<u8> {
    let bytes = serde_json::to_vec(value).unwrap_or_default();
    Sha256::digest(&bytes).to_vec()
}

fn group_to_view(row: &HostGroupRow) -> HostGroupView {
    HostGroupView {
        id: row.id,
        name: row.name.clone(),
        hostname_pattern: row.hostname_pattern.clone(),
        is_standalone: row.is_standalone,
        defaults: row.defaults.clone(),
        applications: row.applications.clone(),
        version: row.version,
        created_at: row.created_at.clone(),
        updated_at: row.updated_at.clone(),
    }
}

fn profile_to_view(row: &HostProfileRow) -> HostProfileView {
    HostProfileView {
        id: row.id,
        group_id: row.group_id,
        identity: row.identity.clone(),
        hostname_override: row.hostname_override.clone(),
        overrides: row.overrides.clone(),
        applications: row.applications.clone(),
        version: row.version,
        created_at: row.created_at.clone(),
        updated_at: row.updated_at.clone(),
    }
}

fn allocation_to_view(row: &HostnameAllocationRow) -> AllocationView {
    AllocationView {
        identity: row.identity.clone(),
        index: row.index,
        hostname: row.hostname.clone(),
        allocated_at: row.allocated_at.clone(),
        released_at: row.released_at.clone(),
        rebound_to: row.rebound_to.clone(),
    }
}

// `group_row_to_profile` / `profile_row_to_profile` were relocated to
// `crate::profiles::convert` (DS-OPS-03) so registry resolution
// (`resolve_from_registry`) can share them without depending on this HTTP
// handler module. They remain `pub(crate)` — never `pub`. Imported at the top.

/// Every group + every profile currently in the store (needed because
/// `profile::validate`'s global hostname-uniqueness check spans ALL groups,
/// not just the one being written). Fails CLOSED via `?` — an unreadable
/// store here becomes the caller's 503, never an empty snapshot that would
/// let a colliding write through.
async fn load_all_groups_and_profiles(
    store: &dyn ProfileStore,
) -> anyhow::Result<(Vec<HostGroupRow>, Vec<HostProfileRow>)> {
    let groups = store.list_groups().await?;
    let mut profiles = Vec::new();
    for g in &groups {
        profiles.extend(store.list_profiles(g.id).await?);
    }
    Ok((groups, profiles))
}

/// Runs `profile::validate` (DS-PRF-03) over a full groups+profiles
/// snapshot, converting rows to the typed tier first. Shared by every
/// mutating profile handler so create/update-group and create-profile all
/// validate against the SAME full-snapshot rule, not a narrower one.
fn validate_snapshot(groups: &[HostGroupRow], profiles: &[HostProfileRow]) -> Result<(), String> {
    let group_profiles: Vec<HostGroupProfile> = groups
        .iter()
        .map(group_row_to_profile)
        .collect::<Result<_, _>>()?;

    let names: HashMap<Uuid, &str> = groups.iter().map(|g| (g.id, g.name.as_str())).collect();
    let host_profiles: Vec<HostProfile> = profiles
        .iter()
        .map(|p| {
            let group_name = names.get(&p.group_id).copied().unwrap_or_default();
            profile_row_to_profile(p, group_name)
        })
        .collect::<Result<_, _>>()?;

    validate_profiles(&group_profiles, &host_profiles).map_err(|e| e.to_string())
}

async fn handle_list_groups(State(state): State<AppState>) -> Response {
    match state.profile_store.list_groups().await {
        Ok(mut rows) => {
            rows.sort_by(|a, b| a.name.cmp(&b.name));
            let views: Vec<_> = rows.iter().map(group_to_view).collect();
            json_response(StatusCode::OK, views)
        }
        Err(err) => {
            tracing::error!(%err, "profile store unreadable listing groups");
            store_unavailable("listing groups")
        }
    }
}

async fn handle_get_group(State(state): State<AppState>, AxumPath(name): AxumPath<String>) -> Response {
    match state.profile_store.get_group(&name).await {
        Ok(Some(row)) => json_response(StatusCode::OK, group_to_view(&row)),
        Ok(None) => not_found("group not found"),
        Err(err) => {
            tracing::error!(%err, %name, "profile store unreadable looking up group");
            store_unavailable("looking up group")
        }
    }
}

async fn handle_list_group_profiles(
    State(state): State<AppState>,
    AxumPath(name): AxumPath<String>,
) -> Response {
    let group = match state.profile_store.get_group(&name).await {
        Ok(Some(row)) => row,
        Ok(None) => return not_found("group not found"),
        Err(err) => {
            tracing::error!(%err, %name, "profile store unreadable looking up group");
            return store_unavailable("looking up group");
        }
    };
    match state.profile_store.list_profiles(group.id).await {
        Ok(rows) => {
            let views: Vec<_> = rows.iter().map(profile_to_view).collect();
            json_response(StatusCode::OK, views)
        }
        Err(err) => {
            tracing::error!(%err, %name, "profile store unreadable listing profiles");
            store_unavailable("listing profiles")
        }
    }
}

async fn handle_list_group_allocations(
    State(state): State<AppState>,
    AxumPath(name): AxumPath<String>,
) -> Response {
    let group = match state.profile_store.get_group(&name).await {
        Ok(Some(row)) => row,
        Ok(None) => return not_found("group not found"),
        Err(err) => {
            tracing::error!(%err, %name, "profile store unreadable looking up group");
            return store_unavailable("looking up group");
        }
    };
    match state.profile_store.list_allocations(group.id).await {
        Ok(rows) => {
            let views: Vec<_> = rows.iter().map(allocation_to_view).collect();
            json_response(StatusCode::OK, views)
        }
        Err(err) => {
            tracing::error!(%err, %name, "profile store unreadable listing allocations");
            store_unavailable("listing allocations")
        }
    }
}

async fn handle_create_group(
    State(state): State<AppState>,
    Extension(session): Extension<auth::Session>,
    Json(body): Json<GroupWriteBody>,
) -> Response {
    let (mut groups, profiles) =
        match load_all_groups_and_profiles(state.profile_store.as_ref()).await {
            Ok(v) => v,
            Err(err) => {
                tracing::error!(%err, "profile store unreadable creating group");
                return store_unavailable("creating group");
            }
        };

    let now = now_rfc3339();
    let row = HostGroupRow {
        id: Uuid::new_v4(),
        name: body.name.clone(),
        hostname_pattern: body.hostname_pattern.clone(),
        is_standalone: body.is_standalone,
        defaults: serde_json::to_value(&body.defaults).unwrap_or(serde_json::Value::Null),
        applications: serde_json::to_value(&body.applications).unwrap_or(serde_json::Value::Null),
        content_hash: content_hash(&(&body.defaults, &body.applications)),
        version: 1,
        created_at: Some(now.clone()),
        updated_at: Some(now),
    };

    // Validate against the FULL snapshot including the new group — this is
    // where a duplicate/colliding hostname across groups gets caught.
    groups.push(row.clone());
    if let Err(message) = validate_snapshot(&groups, &profiles) {
        return validation_error(message);
    }

    if let Err(err) = state.profile_store.put_group(row.clone(), &session.login).await {
        tracing::error!(%err, "failed to persist new group");
        return store_unavailable("creating group");
    }
    json_response(StatusCode::CREATED, group_to_view(&row))
}

async fn handle_update_group(
    State(state): State<AppState>,
    Extension(session): Extension<auth::Session>,
    AxumPath(name): AxumPath<String>,
    Json(body): Json<GroupWriteBody>,
) -> Response {
    // Names are immutable (spec D2) — a PUT naming a different group than the
    // path is a rename attempt, rejected before touching the store.
    if body.name != name {
        return validation_error(format!(
            "group name is immutable: path names {name:?}, body names {:?}; \
             create a new group and rebind hosts instead (spec D2)",
            body.name
        ));
    }

    let (mut groups, profiles) =
        match load_all_groups_and_profiles(state.profile_store.as_ref()).await {
            Ok(v) => v,
            Err(err) => {
                tracing::error!(%err, %name, "profile store unreadable updating group");
                return store_unavailable("updating group");
            }
        };

    let Some(existing) = groups.iter().find(|g| g.name == name).cloned() else {
        return not_found("group not found");
    };

    let updated = HostGroupRow {
        id: existing.id,
        name: existing.name.clone(),
        hostname_pattern: body.hostname_pattern.clone(),
        is_standalone: body.is_standalone,
        defaults: serde_json::to_value(&body.defaults).unwrap_or(serde_json::Value::Null),
        applications: serde_json::to_value(&body.applications).unwrap_or(serde_json::Value::Null),
        content_hash: content_hash(&(&body.defaults, &body.applications)),
        version: existing.version + 1,
        created_at: existing.created_at.clone(),
        updated_at: Some(now_rfc3339()),
    };

    if let Some(slot) = groups.iter_mut().find(|g| g.id == existing.id) {
        *slot = updated.clone();
    }
    if let Err(message) = validate_snapshot(&groups, &profiles) {
        return validation_error(message);
    }

    if let Err(err) = state.profile_store.put_group(updated.clone(), &session.login).await {
        tracing::error!(%err, %name, "failed to persist group update");
        return store_unavailable("updating group");
    }
    json_response(StatusCode::OK, group_to_view(&updated))
}

async fn handle_delete_group(
    State(state): State<AppState>,
    Extension(_session): Extension<auth::Session>,
    AxumPath(name): AxumPath<String>,
) -> Response {
    let group = match state.profile_store.get_group(&name).await {
        Ok(Some(row)) => row,
        Ok(None) => return not_found("group not found"),
        Err(err) => {
            tracing::error!(%err, %name, "profile store unreadable deleting group");
            return store_unavailable("deleting group");
        }
    };
    if group.is_standalone {
        return validation_error(
            "the standalone group cannot be deleted (spec D3)".to_string(),
        );
    }
    if let Err(err) = state.profile_store.delete_group(&name).await {
        tracing::error!(%err, %name, "failed to delete group");
        return store_unavailable("deleting group");
    }
    StatusCode::NO_CONTENT.into_response()
}

async fn handle_create_group_profile(
    State(state): State<AppState>,
    Extension(session): Extension<auth::Session>,
    AxumPath(name): AxumPath<String>,
    Json(body): Json<ProfileWriteBody>,
) -> Response {
    let (groups, mut profiles) =
        match load_all_groups_and_profiles(state.profile_store.as_ref()).await {
            Ok(v) => v,
            Err(err) => {
                tracing::error!(%err, %name, "profile store unreadable creating profile");
                return store_unavailable("creating profile");
            }
        };

    let Some(group) = groups.iter().find(|g| g.name == name) else {
        return not_found("group not found");
    };

    let now = now_rfc3339();
    let row = HostProfileRow {
        id: Uuid::new_v4(),
        group_id: group.id,
        identity: normalize_mac(&body.identity),
        hostname_override: body.hostname_override.clone(),
        overrides: serde_json::to_value(&body.overrides).unwrap_or(serde_json::Value::Null),
        applications: serde_json::to_value(&body.applications).unwrap_or(serde_json::Value::Null),
        content_hash: content_hash(&(&body.overrides, &body.applications)),
        version: 1,
        created_at: Some(now.clone()),
        updated_at: Some(now),
    };

    profiles.push(row.clone());
    if let Err(message) = validate_snapshot(&groups, &profiles) {
        return validation_error(message);
    }

    if let Err(err) = state.profile_store.put_profile(row.clone(), &session.login).await {
        tracing::error!(%err, %name, "failed to persist new profile");
        return store_unavailable("creating profile");
    }
    json_response(StatusCode::CREATED, profile_to_view(&row))
}

async fn handle_rebind(
    State(state): State<AppState>,
    Extension(session): Extension<auth::Session>,
    AxumPath(name): AxumPath<String>,
    Json(body): Json<RebindBody>,
) -> Response {
    let group = match state.profile_store.get_group(&name).await {
        Ok(Some(row)) => row,
        Ok(None) => return not_found("group not found"),
        Err(err) => {
            tracing::error!(%err, %name, "profile store unreadable looking up group for rebind");
            return store_unavailable("looking up group");
        }
    };

    match state
        .profile_store
        .rebind(
            state.audit_store.as_ref(),
            &session.login,
            group.id,
            &body.old_identity,
            &body.new_identity,
        )
        .await
    {
        Ok(row) => json_response(StatusCode::OK, allocation_to_view(&row)),
        Err(err) => {
            // Every `rebind` failure mode (unbound `old_identity`,
            // already-bound `new_identity`) is a client input problem —
            // never silently allocate instead (spec D18 / this task's edge
            // semantics). The error message already names the offending
            // identity.
            validation_error(err.to_string())
        }
    }
}

// ── /api/drift review routes (DS-OPS-02) ──────────────────────────────────
//
// A thin HTTP layer over DS-REG-05's `crate::profiles::drift::{scan_drift,
// accept_drift, revert_drift}` — this module does NOT compute drift, does
// NOT re-derive "last good version", and does NOT call `audit::record` (both
// drift.rs actions already audit via `append_in_txn`; a second `record()`
// call here would double-log against a mutation `record`'s own no-op
// contract forbids). List at `Role::Viewer`, accept/revert at
// `Role::Operator` — see `build_router`. The actor for accept/revert is the
// authenticated session's login (`Extension<auth::Session>`), mirroring
// `handle_approve_enrollment`/`handle_rebind` above — never a placeholder.

/// The normative note carried on every revert response (see
/// `api_types::ReviewResultView::note`'s doc): v1 has no re-render, so a
/// revert changes a stored row and leaves the deployed host exactly as
/// drifted as it was. An operator who reads "reverted" as "fleet fixed" is
/// the failure mode this wording exists to prevent (spec D11).
const REVERT_NOTE: &str = "revert restores the stored INTENT, not the deployed machine: the \
     host remains exactly as drifted as it was, and re-deploying it to apply this change is a \
     separate operator action.";

/// Hex-encode `bytes` for the wire (`DriftView::stored_hash`/`actual_hash`).
/// A local one-liner, not a shared helper — this module's established
/// per-file-duplication convention (see `now_rfc3339`'s doc above) for a
/// utility this small.
fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn drift_to_view(report: &drift::DriftReport) -> DriftView {
    DriftView {
        object_kind: report.object_kind.clone(),
        object_id: report.object_id,
        stored_hash: hex_encode(&report.stored_hash),
        actual_hash: hex_encode(&report.actual_hash),
        seen_count: report.seen_count,
    }
}

/// Parse a `:object_id` path segment. A malformed UUID can never name a
/// known object, so it is reported the same way an unknown-but-well-formed
/// one is — 404 — rather than a separate 400 branch for what is, from the
/// caller's perspective, the same "no such object" outcome. Returns `None`
/// (not `Result<_, Response>`) so this stays a small `Copy` type — clippy's
/// `result_large_err` flags a `Response`-carrying `Err` variant.
fn parse_object_id(raw: &str) -> Option<Uuid> {
    Uuid::parse_str(raw).ok()
}

/// Maps an `accept_drift`/`revert_drift` `Err` to its HTTP shape per this
/// task's edge semantics: unknown object (from `find_review_target`) is 404;
/// "not drifted" and "no good version to revert to" (both from drift.rs,
/// already naming the object) are 400. drift.rs returns plain `anyhow::Error`
/// with no typed variants, so this distinguishes by the one message shape
/// that is actually a lookup failure — every other message these two
/// functions can produce is a client-facing 400 by construction (each names
/// the object and the specific reason). A store-level I/O failure inside
/// `find_review_target`/`list_versions` would also fall through to this 400
/// branch rather than a 503 — an acknowledged gap (drift.rs has no separate
/// "store unavailable" error shape to key off of), flagged here rather than
/// silently mis-mapped.
fn review_error_response(err: anyhow::Error) -> Response {
    let message = err.to_string();
    if message.contains("is not a known host group or profile") {
        not_found(&message)
    } else {
        validation_error(message)
    }
}

async fn handle_list_drift(State(state): State<AppState>) -> Response {
    match drift::scan_drift(state.profile_store.as_ref()).await {
        Ok(reports) => {
            let views: Vec<_> = reports.iter().map(drift_to_view).collect();
            json_response(StatusCode::OK, views)
        }
        Err(err) => {
            tracing::error!(%err, "profile store unreadable scanning for drift");
            store_unavailable("scanning for drift")
        }
    }
}

async fn handle_accept_drift(
    State(state): State<AppState>,
    Extension(session): Extension<auth::Session>,
    AxumPath(object_id_raw): AxumPath<String>,
) -> Response {
    let Some(object_id) = parse_object_id(&object_id_raw) else {
        return not_found("unknown object id");
    };
    match drift::accept_drift(
        state.profile_store.as_ref(),
        state.audit_store.as_ref(),
        object_id,
        &session.login,
    )
    .await
    {
        Ok(row) => json_response(
            StatusCode::OK,
            ReviewResultView {
                object_kind: row.object_kind,
                object_id: row.object_id,
                version: row.version,
                note: None,
            },
        ),
        Err(err) => {
            tracing::info!(%err, %object_id, "drift accept rejected");
            review_error_response(err)
        }
    }
}

async fn handle_revert_drift(
    State(state): State<AppState>,
    Extension(session): Extension<auth::Session>,
    AxumPath(object_id_raw): AxumPath<String>,
) -> Response {
    let Some(object_id) = parse_object_id(&object_id_raw) else {
        return not_found("unknown object id");
    };
    match drift::revert_drift(
        state.profile_store.as_ref(),
        state.audit_store.as_ref(),
        object_id,
        &session.login,
    )
    .await
    {
        Ok(row) => json_response(
            StatusCode::OK,
            ReviewResultView {
                object_kind: row.object_kind,
                object_id: row.object_id,
                version: row.version,
                note: Some(REVERT_NOTE.to_string()),
            },
        ),
        Err(err) => {
            tracing::info!(%err, %object_id, "drift revert rejected");
            review_error_response(err)
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
            app_reports: Vec::new(),
            last_app_status_at: None,
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
            profile_store: Arc::new(crate::profiles::store::MemProfileStore::new()),
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
            profile_store: Arc::new(crate::profiles::store::MemProfileStore::new()),
        }
    }

    /// Same as [`test_state`] but with a caller-supplied `profile_store` —
    /// needed by the profile-route tests below (a shared `MemProfileStore`
    /// pre-seeded with a group, or the deliberately-failing store used by
    /// `test_store_unreadable_returns_503_not_empty`).
    fn test_state_with_profile_store(
        webroot: PathBuf,
        registry: Arc<dyn Registry>,
        audit_store: Arc<dyn AuditStore>,
        profile_store: Arc<dyn ProfileStore>,
    ) -> AppState {
        let ca_dir = webroot.join("ca");
        AppState {
            webroot: Arc::new(webroot),
            registry,
            enrollment_store: Arc::new(MemEnrollmentStore::new()),
            audit_store,
            ca_dir: Arc::new(ca_dir),
            profile_store,
        }
    }

    /// A stand-in authenticated principal for tests that call a protected
    /// handler function directly (bypassing the router, so `auth::require_role`
    /// never runs to insert a real one).
    fn test_session() -> Extension<auth::Session> {
        Extension(auth::Session {
            login: "test-operator".to_string(),
            role: Role::Operator,
            is_bootstrap: false,
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

    // ── /api/groups + /api/profiles (DS-OPS-01) ───────────────────────────

    use crate::db::ProfileVersionRow;

    /// A `ProfileStore` that fails every call — the double
    /// `test_store_unreadable_returns_503_not_open` needs to prove the
    /// routes fail CLOSED (503), never an empty list, when the store cannot
    /// be read.
    struct FailingProfileStore;

    #[async_trait::async_trait]
    impl ProfileStore for FailingProfileStore {
        async fn list_groups(&self) -> anyhow::Result<Vec<HostGroupRow>> {
            Err(anyhow::anyhow!("simulated store failure"))
        }
        async fn get_group(&self, _name: &str) -> anyhow::Result<Option<HostGroupRow>> {
            Err(anyhow::anyhow!("simulated store failure"))
        }
        async fn put_group(&self, _row: HostGroupRow, _actor: &str) -> anyhow::Result<()> {
            Err(anyhow::anyhow!("simulated store failure"))
        }
        async fn delete_group(&self, _name: &str) -> anyhow::Result<()> {
            Err(anyhow::anyhow!("simulated store failure"))
        }
        async fn list_profiles(&self, _group_id: Uuid) -> anyhow::Result<Vec<HostProfileRow>> {
            Err(anyhow::anyhow!("simulated store failure"))
        }
        async fn put_profile(&self, _row: HostProfileRow, _actor: &str) -> anyhow::Result<()> {
            Err(anyhow::anyhow!("simulated store failure"))
        }
        async fn list_allocations(
            &self,
            _group_id: Uuid,
        ) -> anyhow::Result<Vec<HostnameAllocationRow>> {
            Err(anyhow::anyhow!("simulated store failure"))
        }
        async fn allocate_index(
            &self,
            _group_id: Uuid,
            _identity: &str,
        ) -> anyhow::Result<HostnameAllocationRow> {
            Err(anyhow::anyhow!("simulated store failure"))
        }
        async fn rebind(
            &self,
            _audit: &dyn AuditStore,
            _actor: &str,
            _group_id: Uuid,
            _old_identity: &str,
            _new_identity: &str,
        ) -> anyhow::Result<HostnameAllocationRow> {
            Err(anyhow::anyhow!("simulated store failure"))
        }
        async fn list_versions(&self, _object_id: Uuid) -> anyhow::Result<Vec<ProfileVersionRow>> {
            Err(anyhow::anyhow!("simulated store failure"))
        }
        async fn put_version(&self, _row: ProfileVersionRow) -> anyhow::Result<()> {
            Err(anyhow::anyhow!("simulated store failure"))
        }
    }

    fn sample_group_row(id: Uuid, name: &str, is_standalone: bool) -> HostGroupRow {
        HostGroupRow {
            id,
            name: name.to_string(),
            hostname_pattern: "{name}-{index:03}".to_string(),
            is_standalone,
            defaults: serde_json::json!({}),
            applications: serde_json::json!([]),
            content_hash: vec![],
            version: 1,
            created_at: None,
            updated_at: None,
        }
    }

    /// THE worst defect this task could ship: every mutating profile route
    /// must be `Role::Operator`-gated. Drives the real router (real
    /// `auth::require_role` middleware), unauthenticated, and asserts 401 —
    /// never 200/201/204, which would mean the route was added outside
    /// `build_router`'s role-grouping convention (see that fn's doc).
    #[tokio::test]
    async fn test_profile_mutations_require_operator() {
        let (router, _token, _dir) = test_full_router();

        for (method, uri) in [
            ("POST", "/api/groups"),
            ("PUT", "/api/groups/some-group"),
            ("DELETE", "/api/groups/some-group"),
            ("POST", "/api/groups/some-group/profiles"),
            ("POST", "/api/groups/some-group/rebind"),
        ] {
            let req = Request::builder()
                .method(method)
                .uri(uri)
                .body(Body::empty())
                .unwrap();
            let resp = router.clone().oneshot(req).await.unwrap();
            assert_eq!(
                resp.status(),
                StatusCode::UNAUTHORIZED,
                "{method} {uri} must require auth, got {}",
                resp.status()
            );
        }
    }

    #[tokio::test]
    async fn test_profile_reads_require_viewer() {
        let (router, _token, _dir) = test_full_router();

        for uri in [
            "/api/groups",
            "/api/groups/some-group",
            "/api/groups/some-group/profiles",
            "/api/groups/some-group/allocations",
        ] {
            let resp = router.clone().oneshot(get(uri)).await.unwrap();
            assert_eq!(
                resp.status(),
                StatusCode::UNAUTHORIZED,
                "{uri} must require auth, got {}",
                resp.status()
            );
        }
    }

    #[tokio::test]
    async fn test_create_group_validates() {
        let dir = tempdir().unwrap();
        let state = test_state_with_profile_store(
            dir.path().to_path_buf(),
            Arc::new(MockRegistry::default()),
            Arc::new(MemAuditStore::new()),
            Arc::new(crate::profiles::store::MemProfileStore::new()),
        );

        // Two violations at once: no `{index}` placeholder AND no
        // is_standalone=true group anywhere — the body must name BOTH, not
        // just the first (DS-PRF-03 collects every violation).
        let body = Json(GroupWriteBody {
            name: "bad-group".to_string(),
            hostname_pattern: "static-name".to_string(),
            is_standalone: false,
            defaults: InstallationConfigPartial::default(),
            applications: Vec::new(),
        });

        let resp = handle_create_group(State(state), test_session(), body).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let msg = body_json(resp).await["message"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(
            msg.contains("static-name"),
            "must name the hostname_pattern violation, got: {msg}"
        );
        assert!(
            msg.contains("is_standalone"),
            "must name the missing-standalone violation, got: {msg}"
        );
    }

    #[tokio::test]
    async fn test_valid_group_create_succeeds() {
        // Anti-over-suppression: proves an authorized operator submitting a
        // VALID group still succeeds — without this, an over-strict
        // validator or a mis-wired role gate would make the API reject
        // everything while every negative test above still passes.
        let dir = tempdir().unwrap();
        let state = test_state_with_profile_store(
            dir.path().to_path_buf(),
            Arc::new(MockRegistry::default()),
            Arc::new(MemAuditStore::new()),
            Arc::new(crate::profiles::store::MemProfileStore::new()),
        );

        let body = Json(GroupWriteBody {
            name: "standalone".to_string(),
            hostname_pattern: "{name}-{index:03}".to_string(),
            is_standalone: true,
            defaults: InstallationConfigPartial::default(),
            applications: Vec::new(),
        });

        let resp = handle_create_group(State(state), test_session(), body).await;
        assert_eq!(resp.status(), StatusCode::CREATED);
        let created = body_json(resp).await;
        assert_eq!(created["name"], "standalone");
        assert_eq!(created["version"], 1);
    }

    #[tokio::test]
    async fn test_delete_standalone_rejected() {
        let dir = tempdir().unwrap();
        let profile_store: Arc<dyn ProfileStore> =
            Arc::new(crate::profiles::store::MemProfileStore::new());
        profile_store
            .put_group(sample_group_row(Uuid::new_v4(), "standalone", true), "test-operator")
            .await
            .unwrap();

        let state = test_state_with_profile_store(
            dir.path().to_path_buf(),
            Arc::new(MockRegistry::default()),
            Arc::new(MemAuditStore::new()),
            profile_store,
        );

        let resp = handle_delete_group(
            State(state),
            test_session(),
            AxumPath("standalone".to_string()),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_rename_rejected() {
        let dir = tempdir().unwrap();
        let state = test_state_with_profile_store(
            dir.path().to_path_buf(),
            Arc::new(MockRegistry::default()),
            Arc::new(MemAuditStore::new()),
            Arc::new(crate::profiles::store::MemProfileStore::new()),
        );

        let body = Json(GroupWriteBody {
            name: "different-name".to_string(),
            hostname_pattern: "{name}-{index:03}".to_string(),
            is_standalone: true,
            defaults: InstallationConfigPartial::default(),
            applications: Vec::new(),
        });

        let resp = handle_update_group(
            State(state),
            test_session(),
            AxumPath("original-name".to_string()),
            body,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_get_unknown_group_404() {
        let dir = tempdir().unwrap();
        let state = test_state(dir.path().to_path_buf(), Arc::new(MockRegistry::default()));
        let resp = handle_get_group(State(state), AxumPath("no-such-group".to_string())).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_rebind_audited() {
        let dir = tempdir().unwrap();
        let store = crate::profiles::store::MemProfileStore::new();
        let group_id = Uuid::new_v4();
        store
            .put_group(sample_group_row(group_id, "len-serv", false), "test-operator")
            .await
            .unwrap();
        store
            .allocate_index(group_id, "aa:bb:cc:dd:ee:01")
            .await
            .unwrap();

        let profile_store: Arc<dyn ProfileStore> = Arc::new(store);
        let audit_store: Arc<dyn AuditStore> = Arc::new(MemAuditStore::new());
        let state = test_state_with_profile_store(
            dir.path().to_path_buf(),
            Arc::new(MockRegistry::default()),
            audit_store.clone(),
            profile_store,
        );

        let body = Json(RebindBody {
            old_identity: "aa:bb:cc:dd:ee:01".to_string(),
            new_identity: "aa:bb:cc:dd:ee:02".to_string(),
        });

        let resp = handle_rebind(
            State(state),
            test_session(),
            AxumPath("len-serv".to_string()),
            body,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);

        let events = audit_store.list_events(0).await.unwrap();
        assert_eq!(events.len(), 1, "rebind must append exactly one audit event");
        assert_eq!(
            events[0].actor, "test-operator",
            "the audit actor must be the session's login, never a placeholder"
        );
        assert_eq!(events[0].action, "registry.rebind");
    }

    #[tokio::test]
    async fn test_rebind_unbound_old_identity_400() {
        let dir = tempdir().unwrap();
        let store = crate::profiles::store::MemProfileStore::new();
        let group_id = Uuid::new_v4();
        store
            .put_group(sample_group_row(group_id, "len-serv", false), "test-operator")
            .await
            .unwrap();

        let state = test_state_with_profile_store(
            dir.path().to_path_buf(),
            Arc::new(MockRegistry::default()),
            Arc::new(MemAuditStore::new()),
            Arc::new(store),
        );

        let body = Json(RebindBody {
            old_identity: "aa:bb:cc:dd:ee:99".to_string(),
            new_identity: "aa:bb:cc:dd:ee:98".to_string(),
        });

        let resp = handle_rebind(
            State(state),
            test_session(),
            AxumPath("len-serv".to_string()),
            body,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let msg = body_json(resp).await["message"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(
            msg.contains("aa:bb:cc:dd:ee:99"),
            "must name the unbound identity, got: {msg}"
        );
    }

    #[tokio::test]
    async fn test_store_unreadable_returns_503_not_empty() {
        let dir = tempdir().unwrap();
        let state = test_state_with_profile_store(
            dir.path().to_path_buf(),
            Arc::new(MockRegistry::default()),
            Arc::new(MemAuditStore::new()),
            Arc::new(FailingProfileStore),
        );

        let resp = handle_list_groups(State(state)).await;
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = body_json(resp).await;
        assert!(
            body.is_object() && !body.is_array(),
            "an unreadable store must be an error object, NEVER an empty \
             list (which would read as 'no groups' and re-allocate the \
             fleet from 1), got: {body}"
        );
        assert!(body["message"].is_string());
    }

    // ── /api/drift (DS-OPS-02) ─────────────────────────────────────────────

    /// A drifted group: stored `content_hash` is the empty vec, which never
    /// equals the real canonical hash `drift.rs` computes over any actual
    /// body — so injecting this raw (bypassing `put_group`, which would
    /// recompute the hash and capture a version) is always drifted, with
    /// zero captured versions.
    fn drifted_group_row(id: Uuid) -> HostGroupRow {
        let mut row = sample_group_row(id, "len-serv", false);
        row.content_hash = Vec::new();
        row
    }

    #[tokio::test]
    async fn test_drift_review_requires_operator() {
        let (router, _token, _dir) = test_full_router();
        let id = Uuid::new_v4();

        for (method, uri) in [
            ("POST", format!("/api/drift/{id}/accept")),
            ("POST", format!("/api/drift/{id}/revert")),
        ] {
            let req = Request::builder()
                .method(method)
                .uri(&uri)
                .body(Body::empty())
                .unwrap();
            let resp = router.clone().oneshot(req).await.unwrap();
            assert_eq!(
                resp.status(),
                StatusCode::UNAUTHORIZED,
                "{method} {uri} must require auth, got {}",
                resp.status()
            );
        }
    }

    #[tokio::test]
    async fn test_drift_list_requires_viewer() {
        let (router, _token, _dir) = test_full_router();
        let resp = router.oneshot(get("/api/drift")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_drift_list_empty_is_200_not_404() {
        // Anti-over-suppression: no drift anywhere must be the healthy 200
        // [] answer, never a 404 that would read as a broken endpoint.
        let dir = tempdir().unwrap();
        let state = test_state_with_profile_store(
            dir.path().to_path_buf(),
            Arc::new(MockRegistry::default()),
            Arc::new(MemAuditStore::new()),
            Arc::new(crate::profiles::store::MemProfileStore::new()),
        );

        let resp = handle_list_drift(State(state)).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp).await;
        assert_eq!(body, serde_json::json!([]));
    }

    #[tokio::test]
    async fn test_review_non_drifted_object_400() {
        let dir = tempdir().unwrap();
        let profile_store: Arc<dyn ProfileStore> =
            Arc::new(crate::profiles::store::MemProfileStore::new());
        let id = Uuid::new_v4();
        // put_group recomputes the hash, so this live row is NOT drifted.
        profile_store
            .put_group(sample_group_row(id, "len-serv", false), "op")
            .await
            .unwrap();

        let state = test_state_with_profile_store(
            dir.path().to_path_buf(),
            Arc::new(MockRegistry::default()),
            Arc::new(MemAuditStore::new()),
            profile_store,
        );

        let resp = handle_accept_drift(
            State(state),
            test_session(),
            AxumPath(id.to_string()),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let msg = body_json(resp).await["message"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(
            msg.contains("no drift to review") && msg.contains(&id.to_string()),
            "must name why AND which object, got: {msg}"
        );
    }

    #[tokio::test]
    async fn test_revert_without_good_version_400() {
        let dir = tempdir().unwrap();
        let store = crate::profiles::store::MemProfileStore::new();
        let id = Uuid::new_v4();
        // Drifted, but no version was ever captured for it (raw inject, no
        // put_group) — there is no last-good body to restore.
        store.inject_group_raw(drifted_group_row(id));
        let profile_store: Arc<dyn ProfileStore> = Arc::new(store);
        let state = test_state_with_profile_store(
            dir.path().to_path_buf(),
            Arc::new(MockRegistry::default()),
            Arc::new(MemAuditStore::new()),
            profile_store,
        );

        let resp = handle_revert_drift(
            State(state),
            test_session(),
            AxumPath(id.to_string()),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let msg = body_json(resp).await["message"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(
            msg.contains(&id.to_string()),
            "must name the object, got: {msg}"
        );
    }

    #[tokio::test]
    async fn test_unknown_object_404() {
        let dir = tempdir().unwrap();
        let state = test_state_with_profile_store(
            dir.path().to_path_buf(),
            Arc::new(MockRegistry::default()),
            Arc::new(MemAuditStore::new()),
            Arc::new(crate::profiles::store::MemProfileStore::new()),
        );
        let id = Uuid::new_v4();

        let resp = handle_accept_drift(
            State(state),
            test_session(),
            AxumPath(id.to_string()),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_accept_on_drifted_object_succeeds() {
        // Anti-over-suppression: the happy path — an authenticated operator
        // reviewing an ACTUALLY drifted object — must succeed.
        let dir = tempdir().unwrap();
        let store = crate::profiles::store::MemProfileStore::new();
        let id = Uuid::new_v4();
        store.inject_group_raw(drifted_group_row(id));
        let profile_store: Arc<dyn ProfileStore> = Arc::new(store);

        let state = test_state_with_profile_store(
            dir.path().to_path_buf(),
            Arc::new(MockRegistry::default()),
            Arc::new(MemAuditStore::new()),
            profile_store,
        );

        let resp = handle_accept_drift(
            State(state),
            test_session(),
            AxumPath(id.to_string()),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp).await;
        assert_eq!(body["object_id"], id.to_string());
        assert_eq!(body["object_kind"], "host_group");
        assert!(body["note"].is_null(), "accept must not carry the revert note");
    }

    #[tokio::test]
    async fn test_revert_response_states_intent_not_machine() {
        let dir = tempdir().unwrap();
        let store = crate::profiles::store::MemProfileStore::new();
        let id = Uuid::new_v4();
        // A good version exists (captured by put_group), then the live row
        // is tampered out-of-band — the shape revert_drift needs to succeed.
        store
            .put_group(sample_group_row(id, "len-serv", false), "op")
            .await
            .unwrap();
        let mut tampered = sample_group_row(id, "len-serv", false);
        tampered.content_hash = vec![0xde, 0xad, 0xbe, 0xef];
        store.inject_group_raw(tampered);
        let profile_store: Arc<dyn ProfileStore> = Arc::new(store);

        let state = test_state_with_profile_store(
            dir.path().to_path_buf(),
            Arc::new(MockRegistry::default()),
            Arc::new(MemAuditStore::new()),
            profile_store,
        );

        let resp = handle_revert_drift(
            State(state),
            test_session(),
            AxumPath(id.to_string()),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp).await;
        let note = body["note"].as_str().expect("revert response must carry a note");
        assert!(
            note.contains("INTENT") && note.contains("machine") && note.to_lowercase().contains("re-deploy"),
            "the note must state revert restores intent (not the machine) and that \
             re-deploying is a separate action, got: {note}"
        );
    }

    #[tokio::test]
    async fn test_review_uses_session_actor() {
        // The audit event's actor must be the SESSION's login, never a
        // placeholder — mirrors test_rebind_audited's assertion above.
        let dir = tempdir().unwrap();
        let store = crate::profiles::store::MemProfileStore::new();
        let id = Uuid::new_v4();
        store.inject_group_raw(drifted_group_row(id));
        let profile_store: Arc<dyn ProfileStore> = Arc::new(store);
        let audit_store: Arc<dyn AuditStore> = Arc::new(MemAuditStore::new());

        let state = test_state_with_profile_store(
            dir.path().to_path_buf(),
            Arc::new(MockRegistry::default()),
            audit_store.clone(),
            profile_store,
        );

        let resp = handle_accept_drift(
            State(state),
            test_session(),
            AxumPath(id.to_string()),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);

        let events = audit_store.list_events(0).await.unwrap();
        assert!(
            events
                .iter()
                .any(|e| e.actor == "test-operator" && e.action == "registry.drift.accept"),
            "the audit event's actor must be the session's login, not a placeholder"
        );
    }
}
