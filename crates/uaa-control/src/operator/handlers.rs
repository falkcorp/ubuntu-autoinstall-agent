// file: crates/uaa-control/src/operator/handlers.rs
// version: 1.2.0
// guid: e94ff17e-4e1b-4672-8940-1fe111b56861
// last-edited: 2026-07-13

//! Operator API request handlers (`:15001`, mounted under `/api/*` ahead of
//! [`super::web_ui`]'s SPA fallback).
//!
//! This is a first vertical slice, not the full CT-07 scope: `GET
//! /api/machines` (+ single-machine GET, + approve) is real, backed by the
//! same CT-01 snapshot `machine_plane::{seeds,lifecycle}` read/write.
//! Enrollments/discovery/audit and reinstall have no backing implementation
//! yet (CT-05/06 sagas, PK-01 enrollment state, CT-04 audit chain are none
//! of them wired to HTTP here) — they're stubbed to return well-formed empty
//! results / a clear "not implemented" error instead of crashing the SPA
//! page that calls them. No auth middleware is wired yet either: this plane
//! is exactly as unauthenticated right now as the legacy `:25000` plane
//! already is (which also serves the full registry, including `tpm_ek`, with
//! zero auth) — not a new exposure, but also not the end state (spec
//! Decision 19 gates this on CT-03's OAuth/RBAC landing on this router).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::{
    extract::{Path as AxumPath, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};

use crate::db::{
    store::{read_snapshot, write_snapshot, StatePaths},
    BootTarget, MachineRow as DbMachineRow, MachineStatus,
};
use crate::machine_plane::lifecycle::normalize_mac;

use super::api_types::{ApiErrorBody, AuditVerifyResult, MachineRow};

/// Webroot base for placed cloud-init configs (mirrors `machine_plane::seeds`'
/// `CLOUD_INIT_BASE`; duplicated per-file — see that module's REUSE note).
const CLOUD_INIT_BASE: &str = "/var/www/html/cloud-init";

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
}

fn default_state() -> AppState {
    AppState {
        webroot: Arc::new(PathBuf::from(CLOUD_INIT_BASE)),
        registry: Arc::new(FileRegistry::new(StatePaths::default())),
    }
}

/// The operator API sub-router, mounted under `/api/*`. Merged ahead of
/// [`super::web_ui::router`]'s fallback so API paths are matched first.
pub fn router() -> Router {
    build_router(default_state())
}

fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(handle_healthz))
        .route("/api/machines", get(handle_list_machines))
        .route("/api/machines/:mac", get(handle_get_machine))
        .route("/api/machines/:mac/approve", post(handle_approve_machine))
        .route(
            "/api/machines/:mac/reinstall",
            post(handle_reinstall_machine),
        )
        .route("/api/enrollments", get(handle_list_enrollments))
        .route(
            "/api/enrollments/:fp/approve",
            post(handle_approve_enrollment),
        )
        .route(
            "/api/enrollments/:fp/reject",
            post(handle_reject_enrollment),
        )
        .route("/api/discovered", get(handle_list_discovered))
        .route(
            "/api/discovered/:mac/dismiss",
            post(handle_dismiss_discovered),
        )
        .route("/api/audit", get(handle_list_audit))
        .route("/api/audit/verify", get(handle_verify_audit))
        .with_state(state)
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

// ── Stubs: enrollments / discovery / audit (no backing implementation yet) ─

async fn handle_list_enrollments(State(_state): State<AppState>) -> Response {
    json_response(
        StatusCode::OK,
        Vec::<super::api_types::EnrollmentRow>::new(),
    )
}

async fn handle_approve_enrollment(
    State(_state): State<AppState>,
    AxumPath(_fp): AxumPath<String>,
) -> Response {
    not_implemented("enrollment approval")
}

async fn handle_reject_enrollment(
    State(_state): State<AppState>,
    AxumPath(_fp): AxumPath<String>,
) -> Response {
    not_implemented("enrollment rejection")
}

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

async fn handle_list_audit(State(_state): State<AppState>) -> Response {
    json_response(
        StatusCode::OK,
        Vec::<super::api_types::AuditEventRow>::new(),
    )
}

async fn handle_verify_audit(State(_state): State<AppState>) -> Response {
    json_response(
        StatusCode::OK,
        AuditVerifyResult {
            ok: true,
            checked: 0,
            message: None,
        },
    )
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

    fn test_state(webroot: PathBuf, registry: Arc<dyn Registry>) -> AppState {
        AppState {
            webroot: Arc::new(webroot),
            registry,
        }
    }

    async fn body_json(resp: Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[test]
    fn test_router_builds_standalone() {
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
    async fn test_stub_list_endpoints_return_empty_arrays() {
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
}
