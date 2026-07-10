// file: crates/uaa-control/src/machine_plane/lifecycle.rs
// version: 1.1.1
// guid: f1273168-f053-480d-8baf-aa653555cb85
// last-edited: 2026-07-10

//! Machine-plane checkin + install-event ingest (WAL append when degraded).
//!
//! Exact parity with `scripts/autoinstall-agent.py` (spec Decision 12) for the
//! lifecycle POST endpoints: `/api/register`, `/api/checkin`, `/api/webhook`,
//! `/api/finalreport`, `/api/hardware-info`, `/api/cloud-init`.
//!
//! Every response is JSON; invalid request bodies get `400 {"error": "invalid
//! json"}` (Python `:564-568`). Persistence never talks to CockroachDB directly —
//! everything goes through the [`Registry`] trait, whose real implementation
//! ([`FileRegistry`]) is backed by CT-01's snapshot+WAL degraded-mode layer
//! (`crate::db::store`), so checkin/install-event ingest is fail-OPEN (spec
//! Decision 4): it always lands in the WAL, never 503s. Unit tests substitute an
//! in-memory `MockRegistry` — no live CRDB, no network.

use std::path::PathBuf;
use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use serde_json::{json, Value};

use crate::db::{
    store::{read_snapshot, wal_append, write_snapshot, StatePaths},
    BootTarget, MachineRow, MachineStatus,
};

/// Default iPXE boot-menu directory (mirrors Python's `IPXE_BOOT_DIR`).
const IPXE_BOOT_DIR: &str = "/var/www/html/ipxe/boot";
/// Default directory for webhook-uploaded log files (mirrors Python's `FILES_DIR`).
const FILES_DIR: &str = "/var/log/cockroach-autoinstall/files";

// ── Pure parity functions ──────────────────────────────────────────────────

/// MAC normalization: lowercase, `-`→`:`, `.`→`:` (Python `normalize_mac`, `:72-73`).
/// Applied on register AND checkin so `AA-BB-...` and `aa:bb:...` are one machine.
pub fn normalize_mac(mac: &str) -> String {
    mac.to_lowercase().replace(['-', '.'], ":")
}

/// Strip separators for the `mac-<hexmac>.ipxe` filename convention (Python
/// `mac_to_hex`, `:75-76`).
fn mac_to_hex(mac: &str) -> String {
    mac.to_lowercase().replace([':', '-'], "")
}

/// Exact port of Python's `webhook_should_flip` (`:154-168`).
///
/// True only for FINAL successful-install webhook payloads. cloud-init
/// `reporting.sh` posts status `"finished"`/`"complete"` (always final, the
/// widened tuple at `:164`). The Rust `uaa` installer posts `event_type:
/// "status_update"` with status `"running"` (start + per-phase), `"failed"`, and
/// `"success"` (final at `progress` 100) — a `status_update` may flip only on a
/// final result: `status == "success"` OR `progress == 100` (accepts a JSON
/// number, integer or float).
pub fn webhook_should_flip(data: &Value) -> bool {
    let status = data.get("status").and_then(Value::as_str).unwrap_or("");
    let name = data.get("name").and_then(Value::as_str).unwrap_or("");
    if !name.is_empty() && matches!(status, "finished" | "complete" | "success") {
        if data.get("event_type").and_then(Value::as_str) == Some("status_update") {
            let progress_100 = data
                .get("progress")
                .and_then(Value::as_f64)
                .map(|p| p == 100.0)
                .unwrap_or(false);
            return status == "success" || progress_100;
        }
        return true;
    }
    false
}

/// Exact port of the iPXE flip regex from Python's `flip_ipxe` (`:170-177`):
/// `re.sub(r"set menu-default \S+", f"set menu-default {target}", content)`.
pub fn flip_ipxe_content(content: &str, target: &str) -> String {
    let re = regex::Regex::new(r"set menu-default \S+").expect("static regex is valid");
    re.replace_all(content, |_: &regex::Captures| format!("set menu-default {target}"))
        .into_owned()
}

// ── Registry seam (mockable; no direct DB/process access) ───────────────────

/// Persistence seam for the lifecycle handlers. The real implementation
/// ([`FileRegistry`]) is backed by CT-01's snapshot+WAL layer; tests substitute an
/// in-memory mock. Handlers NEVER open a DB connection or spawn a process.
#[async_trait::async_trait]
pub trait Registry: Send + Sync {
    /// Look up a machine row by normalized MAC.
    async fn get_machine(&self, mac: &str) -> Option<MachineRow>;
    /// Upsert (full replace) a machine row.
    async fn upsert_machine(&self, machine: MachineRow);
    /// Resolve the MAC currently registered under `hostname`, if any (used to
    /// build the `mac-<hexmac>.ipxe` candidate path, mirroring Python's
    /// `find_ipxe_file_by_hostname`, `:103-113`).
    async fn find_mac_by_hostname(&self, hostname: &str) -> Option<String>;
    /// Append a generic telemetry/log event (webhook / finalreport /
    /// hardware-info / cloud-init), mirroring Python's `log_event` → `events.jsonl`
    /// (`received_at` prepended, `:257-259`).
    async fn append_event(&self, payload: Value);
    /// Record a final install-history entry (constellation addition; spec CRDB
    /// schema). Mints a fresh `event_id` UUID AT INGEST — the WAL-replay dedup key
    /// (Decision 4a) — and returns it. Also updates the machine's `installed_at` +
    /// `last_install_status`.
    async fn record_install_event(&self, mac: &str, name: &str, status: &str, finished_at: &str) -> uuid::Uuid;
}

/// Real [`Registry`]: backed by CT-01's local snapshot (`SnapshotDoc.machines`)
/// for machine rows and the WAL (`wal_append`) for telemetry/install-history
/// ingest. Never touches CockroachDB directly (spec Decision 4: telemetry
/// ingestion is fail-OPEN — a local file append always succeeds barring IO
/// errors, so this path never 503s).
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
    async fn get_machine(&self, mac: &str) -> Option<MachineRow> {
        let doc = read_snapshot(&self.paths);
        doc.machines.into_iter().find(|m| m.mac == mac)
    }

    async fn upsert_machine(&self, machine: MachineRow) {
        let mut doc = read_snapshot(&self.paths);
        match doc.machines.iter_mut().find(|m| m.mac == machine.mac) {
            Some(existing) => *existing = machine,
            None => doc.machines.push(machine),
        }
        if let Err(err) = write_snapshot(&self.paths, &doc) {
            tracing::error!(%err, "failed to persist machine snapshot");
        }
    }

    async fn find_mac_by_hostname(&self, hostname: &str) -> Option<String> {
        let doc = read_snapshot(&self.paths);
        doc.machines
            .into_iter()
            .find(|m| m.hostname == hostname)
            .map(|m| m.mac)
    }

    async fn append_event(&self, payload: Value) {
        let mut payload = payload;
        if let Some(obj) = payload.as_object_mut() {
            obj.insert("received_at".to_string(), json!(now_epoch_i64()));
        }
        if let Err(err) = wal_append(&self.paths, "event", payload) {
            tracing::error!(%err, "failed to append webhook/log event to WAL");
        }
    }

    async fn record_install_event(&self, mac: &str, name: &str, status: &str, finished_at: &str) -> uuid::Uuid {
        let payload = json!({
            "mac": mac,
            "name": name,
            "status": status,
            "finished_at": finished_at,
        });
        let event_id = match wal_append(&self.paths, "install_history", payload) {
            Ok(event_id) => event_id,
            Err(err) => {
                tracing::error!(%err, "failed to append install_history to WAL");
                uuid::Uuid::new_v4()
            }
        };

        // Best-effort: reflect the final status on the machine row too.
        let mut doc = read_snapshot(&self.paths);
        if let Some(row) = doc.machines.iter_mut().find(|m| m.mac == mac) {
            row.installed_at = Some(finished_at.to_string());
            row.last_install_status = Some(status.to_string());
            if let Err(err) = write_snapshot(&self.paths, &doc) {
                tracing::error!(%err, "failed to persist installed_at/last_install_status");
            }
        }

        event_id
    }
}

// ── Router / handler wiring ──────────────────────────────────────────────────

#[derive(Clone)]
struct AppState {
    registry: Arc<dyn Registry>,
    ipxe_dir: Arc<PathBuf>,
    files_dir: Arc<PathBuf>,
}

fn default_state() -> AppState {
    AppState {
        registry: Arc::new(FileRegistry::new(StatePaths::default())),
        ipxe_dir: Arc::new(PathBuf::from(IPXE_BOOT_DIR)),
        files_dir: Arc::new(PathBuf::from(FILES_DIR)),
    }
}

/// The lifecycle sub-router. Merged into `machine_plane::router()` by the
/// coordinator with `.merge(lifecycle::router())` (one line, owned by CT-01's
/// `mod.rs` — never edited here). Standalone-testable: building this router
/// touches no filesystem/network state (all IO is deferred to request time).
pub fn router() -> Router {
    build_router(default_state())
}

fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/api/register", post(handle_register))
        .route("/api/checkin", post(handle_checkin))
        .route("/api/webhook", post(handle_webhook))
        .route("/api/finalreport", post(handle_finalreport))
        .route("/api/hardware-info", post(handle_hardware_info))
        .route("/api/cloud-init", post(handle_cloud_init))
        .with_state(state)
}

// ── HTTP helpers ──────────────────────────────────────────────────────────

fn json_response(code: StatusCode, body: Value) -> Response {
    (code, Json(body)).into_response()
}

/// Parse the raw request body as JSON, or short-circuit with the Python-parity
/// `400 {"error": "invalid json"}` response (`:564-568`).
#[allow(clippy::result_large_err)]
fn parse_json(body: &Bytes) -> Result<Value, Response> {
    serde_json::from_slice(body)
        .map_err(|_| json_response(StatusCode::BAD_REQUEST, json!({"error": "invalid json"})))
}

fn now_epoch_i64() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn now_epoch_string() -> String {
    now_epoch_i64().to_string()
}

// ── /api/register (Python `:571-596`) ────────────────────────────────────────

async fn handle_register(State(state): State<AppState>, body: Bytes) -> Response {
    let data = match parse_json(&body) {
        Ok(v) => v,
        Err(r) => return r,
    };

    let mac = normalize_mac(data.get("mac").and_then(Value::as_str).unwrap_or(""));
    let hostname = data
        .get("hostname")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let ip = data
        .get("ip")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let server_type = data
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("lenovo")
        .to_string();

    if mac.is_empty() || hostname.is_empty() {
        return json_response(
            StatusCode::BAD_REQUEST,
            json!({"ok": false, "error": "mac and hostname required"}),
        );
    }

    let existing = state.registry.get_machine(&mac).await;

    // PRESERVE status/registered_at/tpm_ek (default pending/now/null); OVERWRITE
    // hostname/ip/type. Everything else carries forward unchanged (Python's model
    // has no other fields to preserve or reset).
    let status = existing
        .as_ref()
        .map(|m| m.status.clone())
        .unwrap_or(MachineStatus::Pending);
    let registered_at = existing
        .as_ref()
        .and_then(|m| m.registered_at.clone())
        .unwrap_or_else(now_epoch_string);
    let tpm_ek = existing.as_ref().and_then(|m| m.tpm_ek.clone());
    let boot_target = existing
        .as_ref()
        .map(|m| m.boot_target.clone())
        .unwrap_or(BootTarget::LocalDisk);
    let approved_at = existing.as_ref().and_then(|m| m.approved_at.clone());
    let last_seen = existing.as_ref().and_then(|m| m.last_seen.clone());
    let last_ip = existing.as_ref().and_then(|m| m.last_ip.clone());
    let installed_at = existing.as_ref().and_then(|m| m.installed_at.clone());
    let last_install_status = existing.as_ref().and_then(|m| m.last_install_status.clone());

    let row = MachineRow {
        mac: mac.clone(),
        hostname,
        ip: Some(ip),
        r#type: server_type,
        status: status.clone(),
        boot_target,
        tpm_ek,
        registered_at: Some(registered_at),
        approved_at,
        last_seen,
        last_ip,
        installed_at,
        last_install_status,
        updated_at: Some(now_epoch_string()),
    };
    state.registry.upsert_machine(row).await;

    let status_str: String = status.into();
    tracing::info!(%mac, %status_str, "REGISTER");
    json_response(
        StatusCode::OK,
        json!({
            "ok": true,
            "status": status_str,
            "message": format!(
                "Registered. Approve with: curl http://172.16.2.30:25000/api/approve/{mac}"
            ),
        }),
    )
}

// ── /api/checkin (Python `:599-621`) ─────────────────────────────────────────

async fn handle_checkin(State(state): State<AppState>, body: Bytes) -> Response {
    let data = match parse_json(&body) {
        Ok(v) => v,
        Err(r) => return r,
    };

    let mac = normalize_mac(data.get("mac").and_then(Value::as_str).unwrap_or(""));
    let tpm_ek = data
        .get("tpm_ek")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let ip = data
        .get("ip")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    let mut entry = match state.registry.get_machine(&mac).await {
        Some(e) => e,
        None => {
            tracing::info!(%mac, "CHECKIN DENIED - not registered");
            return json_response(
                StatusCode::FORBIDDEN,
                json!({"ok": false, "error": "Not registered"}),
            );
        }
    };

    // Order matches Python exactly: lookup -> unknown 403 (above) -> first-bind
    // -> mismatch-403 (return BEFORE touching last_seen, and WITHOUT persisting)
    // -> update last_seen/last_ip -> 200.
    let existing_ek_is_set = entry
        .tpm_ek
        .as_deref()
        .map(|ek| !ek.is_empty())
        .unwrap_or(false);
    if !tpm_ek.is_empty() {
        if !existing_ek_is_set {
            entry.tpm_ek = Some(tpm_ek.clone());
            tracing::info!(%mac, "CHECKIN TPM bound");
        } else if entry.tpm_ek.as_deref() != Some(tpm_ek.as_str()) {
            tracing::warn!(%mac, "CHECKIN TPM MISMATCH");
            return json_response(
                StatusCode::FORBIDDEN,
                json!({"ok": false, "error": "TPM mismatch - MAC may be spoofed"}),
            );
        }
    }

    entry.last_seen = Some(now_epoch_string());
    entry.last_ip = Some(ip);
    entry.updated_at = Some(now_epoch_string());
    let status_str: String = entry.status.clone().into();
    let approved = matches!(entry.status, MachineStatus::Approved);
    state.registry.upsert_machine(entry).await;

    tracing::info!(%mac, %status_str, "CHECKIN");
    json_response(
        StatusCode::OK,
        json!({"ok": true, "status": status_str, "approved": approved}),
    )
}

// ── /api/webhook (Python `:624-650`) ─────────────────────────────────────────

async fn handle_webhook(State(state): State<AppState>, body: Bytes) -> Response {
    let data = match parse_json(&body) {
        Ok(v) => v,
        Err(r) => return r,
    };

    // (a) Append the event FIRST, before any flip attempt (Python order).
    state.registry.append_event(data.clone()).await;

    let status = data
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let name = data
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    if webhook_should_flip(&data) {
        // (b) A missing iPXE file (USB-only host) or ANY flip error is logged and
        // SWALLOWED — the webhook itself still succeeds (`:633-635`).
        let (ok, msg) = flip_ipxe(state.registry.as_ref(), &state.ipxe_dir, &name).await;
        tracing::info!(%name, %status, flip_ok = ok, %msg, "WEBHOOK auto-flip");

        // (c) Record install_history regardless of flip outcome (final success is
        // final success even if the iPXE file could not be located).
        let mac = state
            .registry
            .find_mac_by_hostname(&name)
            .await
            .unwrap_or_default();
        let finished_at = now_epoch_string();
        let event_id = state
            .registry
            .record_install_event(&mac, &name, &status, &finished_at)
            .await;
        tracing::info!(%event_id, %mac, %name, "install_history recorded");
    } else {
        tracing::info!(
            %name,
            event_type = ?data.get("event_type"),
            %status,
            "WEBHOOK (no flip)"
        );
    }

    // (d) decode files[] to the CT-01 files dir; per-file failures are swallowed.
    if let Some(files) = data.get("files").and_then(Value::as_array) {
        for f in files {
            save_webhook_file(&state.files_dir, &name, f);
        }
    }

    json_response(StatusCode::OK, json!({"ok": true}))
}

fn save_webhook_file(files_dir: &std::path::Path, hostname: &str, f: &Value) {
    let raw_path = f.get("path").and_then(Value::as_str).unwrap_or("unknown");
    let sanitized = raw_path.replace('/', "_");
    let ts = now_epoch_string();
    let hostname = if hostname.is_empty() { "unknown" } else { hostname };
    let out = files_dir.join(format!("{hostname}-{ts}-{sanitized}"));
    let content_b64 = f.get("content").and_then(Value::as_str).unwrap_or("");

    use base64::Engine;
    match base64::engine::general_purpose::STANDARD.decode(content_b64) {
        Ok(bytes) => {
            if let Some(parent) = out.parent() {
                if let Err(err) = std::fs::create_dir_all(parent) {
                    tracing::warn!(%err, path = %parent.display(), "failed to create files dir");
                    return;
                }
            }
            match std::fs::write(&out, bytes) {
                Ok(()) => tracing::info!(path = %out.display(), "Saved log"),
                Err(err) => tracing::warn!(%err, path = %out.display(), "Failed to save log"),
            }
        }
        Err(err) => tracing::warn!(%err, %raw_path, "Failed to decode webhook file content"),
    }
}

/// Exact port of Python's `flip_ipxe` (`:170-177`): resolve the target file by
/// hostname, then rewrite `set menu-default \S+` to the given target. A missing
/// file (or any IO error) returns `(false, "No iPXE file found for {hostname}")`
/// / a swallowed error message — never propagated.
async fn flip_ipxe(registry: &dyn Registry, ipxe_dir: &std::path::Path, hostname: &str) -> (bool, String) {
    let path = match resolve_ipxe_path(registry, ipxe_dir, hostname).await {
        Some(p) if p.exists() => p,
        _ => return (false, format!("No iPXE file found for {hostname}")),
    };

    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(err) => return (false, format!("flip failed: {err}")),
    };
    let new_content = flip_ipxe_content(&content, "boot-local-disk");
    match std::fs::write(&path, new_content) {
        Ok(()) => (true, format!("Flipped {hostname} to boot-local-disk")),
        Err(err) => (false, format!("flip failed: {err}")),
    }
}

/// Exact port of Python's `find_ipxe_file_by_hostname` (`:103-113`): registry
/// hostname match first (candidate path is NOT required to exist yet — existence
/// is checked by the caller), then a fallback scan of `*.ipxe` files for a
/// `set hostname <name>` content match.
async fn resolve_ipxe_path(
    registry: &dyn Registry,
    ipxe_dir: &std::path::Path,
    hostname: &str,
) -> Option<PathBuf> {
    if let Some(mac) = registry.find_mac_by_hostname(hostname).await {
        return Some(ipxe_dir.join(format!("mac-{}.ipxe", mac_to_hex(&mac))));
    }
    let entries = std::fs::read_dir(ipxe_dir).ok()?;
    let needle = format!("set hostname {hostname}");
    for entry in entries.flatten() {
        let p = entry.path();
        if p.extension().and_then(|e| e.to_str()) == Some("ipxe") {
            if let Ok(content) = std::fs::read_to_string(&p) {
                if content.contains(&needle) {
                    return Some(p);
                }
            }
        }
    }
    None
}

// ── /api/finalreport, /api/hardware-info, /api/cloud-init (Python `:652-656`) ─

async fn handle_finalreport(State(state): State<AppState>, body: Bytes) -> Response {
    handle_log_only(state, "/api/finalreport", body).await
}

async fn handle_hardware_info(State(state): State<AppState>, body: Bytes) -> Response {
    handle_log_only(state, "/api/hardware-info", body).await
}

async fn handle_cloud_init(State(state): State<AppState>, body: Bytes) -> Response {
    handle_log_only(state, "/api/cloud-init", body).await
}

/// Shared handler for the three log-only sinks: `log_event({"endpoint": path,
/// **data})`, reply `200 {"ok":true}` (Python `:652-656`, event shape `:653`).
async fn handle_log_only(state: AppState, endpoint: &str, body: Bytes) -> Response {
    let data = match parse_json(&body) {
        Ok(v) => v,
        Err(r) => return r,
    };

    let mut payload = json!({"endpoint": endpoint});
    if let (Some(obj), Some(data_obj)) = (payload.as_object_mut(), data.as_object()) {
        for (k, v) in data_obj {
            obj.insert(k.clone(), v.clone());
        }
    }
    state.registry.append_event(payload).await;

    tracing::info!(%endpoint, "log-only event");
    json_response(StatusCode::OK, json!({"ok": true}))
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;
    use tempfile::tempdir;

    #[derive(Debug, Clone)]
    struct RecordedInstallEvent {
        event_id: uuid::Uuid,
        mac: String,
        #[allow(dead_code)]
        name: String,
        #[allow(dead_code)]
        status: String,
        #[allow(dead_code)]
        finished_at: String,
    }

    /// In-memory mock — zero filesystem, zero CRDB.
    #[derive(Default)]
    struct MockRegistry {
        machines: Mutex<HashMap<String, MachineRow>>,
        events: Mutex<Vec<Value>>,
        install_events: Mutex<Vec<RecordedInstallEvent>>,
    }

    #[async_trait::async_trait]
    impl Registry for MockRegistry {
        async fn get_machine(&self, mac: &str) -> Option<MachineRow> {
            self.machines.lock().unwrap().get(mac).cloned()
        }
        async fn upsert_machine(&self, machine: MachineRow) {
            self.machines
                .lock()
                .unwrap()
                .insert(machine.mac.clone(), machine);
        }
        async fn find_mac_by_hostname(&self, hostname: &str) -> Option<String> {
            self.machines
                .lock()
                .unwrap()
                .values()
                .find(|m| m.hostname == hostname)
                .map(|m| m.mac.clone())
        }
        async fn append_event(&self, payload: Value) {
            self.events.lock().unwrap().push(payload);
        }
        async fn record_install_event(
            &self,
            mac: &str,
            name: &str,
            status: &str,
            finished_at: &str,
        ) -> uuid::Uuid {
            let event_id = uuid::Uuid::new_v4();
            self.install_events.lock().unwrap().push(RecordedInstallEvent {
                event_id,
                mac: mac.to_string(),
                name: name.to_string(),
                status: status.to_string(),
                finished_at: finished_at.to_string(),
            });
            // Mirror FileRegistry's side effect so tests can assert on it too.
            if let Some(row) = self.machines.lock().unwrap().get_mut(mac) {
                row.installed_at = Some(finished_at.to_string());
                row.last_install_status = Some(status.to_string());
            }
            event_id
        }
    }

    fn base_machine(mac: &str, hostname: &str) -> MachineRow {
        MachineRow {
            mac: mac.to_string(),
            hostname: hostname.to_string(),
            ip: Some("10.0.0.1".to_string()),
            r#type: "lenovo".to_string(),
            status: MachineStatus::Pending,
            boot_target: BootTarget::LocalDisk,
            tpm_ek: None,
            registered_at: Some("1000".to_string()),
            approved_at: None,
            last_seen: None,
            last_ip: None,
            installed_at: None,
            last_install_status: None,
            updated_at: None,
        }
    }

    fn test_state_with(registry: Arc<dyn Registry>, ipxe_dir: PathBuf) -> AppState {
        AppState {
            registry,
            ipxe_dir: Arc::new(ipxe_dir.clone()),
            files_dir: Arc::new(ipxe_dir),
        }
    }

    async fn body_json(resp: Response) -> Value {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    // ── pure function tests ──────────────────────────────────────────────

    #[test]
    fn test_normalize_mac() {
        assert_eq!(normalize_mac("AA-BB-CC-DD-EE-FF"), "aa:bb:cc:dd:ee:ff");
        assert_eq!(normalize_mac("aa.bb.cc.dd.ee.ff"), "aa:bb:cc:dd:ee:ff");
        assert_eq!(normalize_mac("aa:bb:cc:dd:ee:ff"), "aa:bb:cc:dd:ee:ff");
    }

    #[test]
    fn test_flip_ipxe_content_replaces_menu_default() {
        let content = "#!ipxe\nset menu-default pxe-install\nboot\n";
        let out = flip_ipxe_content(content, "boot-local-disk");
        assert_eq!(out, "#!ipxe\nset menu-default boot-local-disk\nboot\n");
    }

    #[test]
    fn test_flip_predicate_matrix() {
        let cases: Vec<(Value, bool)> = vec![
            (json!({"name": "h1", "status": "finished"}), true),
            (json!({"name": "h1", "status": "complete"}), true),
            (
                json!({"name": "h1", "status": "success", "event_type": "status_update"}),
                true,
            ),
            (
                json!({"name": "h1", "status": "running", "event_type": "status_update", "progress": 50}),
                false,
            ),
            (
                json!({"name": "h1", "status": "finished", "event_type": "status_update", "progress": 100}),
                true,
            ),
            (json!({"name": "h1", "status": "failed"}), false),
            (json!({"name": "", "status": "success"}), false),
        ];
        for (data, expected) in cases {
            assert_eq!(
                webhook_should_flip(&data),
                expected,
                "predicate mismatch for payload: {data}"
            );
        }
    }

    #[test]
    fn test_router_builds_standalone() {
        // Constructing the router touches no filesystem/network — only requests do.
        let _ = router();
    }

    // ── /api/register ────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_register_preserves_approval_and_ek() {
        let dir = tempdir().unwrap();
        let registry = Arc::new(MockRegistry::default());
        let mut seed = base_machine("aa:bb:cc:dd:ee:ff", "old-host");
        seed.status = MachineStatus::Approved;
        seed.tpm_ek = Some("ek-123".to_string());
        registry.upsert_machine(seed).await;

        let state = test_state_with(registry.clone(), dir.path().to_path_buf());
        let body = Bytes::from(
            serde_json::to_vec(&json!({
                "mac": "AA-BB-CC-DD-EE-FF",
                "hostname": "new-host",
                "ip": "10.0.0.2"
            }))
            .unwrap(),
        );
        let resp = handle_register(State(state), body).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let v = body_json(resp).await;
        assert_eq!(v["ok"], true);
        assert_eq!(v["status"], "approved");

        let updated = registry.get_machine("aa:bb:cc:dd:ee:ff").await.unwrap();
        assert_eq!(updated.status, MachineStatus::Approved, "approval preserved");
        assert_eq!(updated.tpm_ek.as_deref(), Some("ek-123"), "EK preserved");
        assert_eq!(updated.hostname, "new-host", "hostname overwritten");
        assert_eq!(updated.registered_at.as_deref(), Some("1000"), "registered_at preserved");
    }

    #[tokio::test]
    async fn test_register_missing_fields_400() {
        let dir = tempdir().unwrap();
        let state = test_state_with(Arc::new(MockRegistry::default()), dir.path().to_path_buf());
        let body = Bytes::from(serde_json::to_vec(&json!({"hostname": "h1"})).unwrap());
        let resp = handle_register(State(state), body).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let v = body_json(resp).await;
        assert_eq!(v["ok"], false);
        assert_eq!(v["error"], "mac and hostname required");
    }

    // ── /api/checkin ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_checkin_first_bind_then_mismatch_403() {
        let dir = tempdir().unwrap();
        let registry = Arc::new(MockRegistry::default());
        registry
            .upsert_machine(base_machine("aa:bb:cc:dd:ee:ff", "h1"))
            .await;
        let state = test_state_with(registry.clone(), dir.path().to_path_buf());

        // First checkin binds the EK.
        let body1 = Bytes::from(
            serde_json::to_vec(&json!({"mac": "aa:bb:cc:dd:ee:ff", "tpm_ek": "ek-1", "ip": "10.0.0.5"}))
                .unwrap(),
        );
        let resp1 = handle_checkin(State(state.clone()), body1).await;
        assert_eq!(resp1.status(), StatusCode::OK);
        let bound = registry.get_machine("aa:bb:cc:dd:ee:ff").await.unwrap();
        assert_eq!(bound.tpm_ek.as_deref(), Some("ek-1"));

        // Force a distinguishable last_seen sentinel so the "unchanged" assertion
        // below cannot pass by same-second timing coincidence.
        let mut sentinel = bound.clone();
        sentinel.last_seen = Some("sentinel-42".to_string());
        registry.upsert_machine(sentinel).await;

        // Second checkin with a DIFFERENT EK -> 403, last_seen untouched.
        let body2 = Bytes::from(
            serde_json::to_vec(&json!({"mac": "aa:bb:cc:dd:ee:ff", "tpm_ek": "ek-2", "ip": "10.0.0.6"}))
                .unwrap(),
        );
        let resp2 = handle_checkin(State(state.clone()), body2).await;
        assert_eq!(resp2.status(), StatusCode::FORBIDDEN);
        let v = body_json(resp2).await;
        assert_eq!(v["error"], "TPM mismatch - MAC may be spoofed");

        let after_mismatch = registry.get_machine("aa:bb:cc:dd:ee:ff").await.unwrap();
        assert_eq!(
            after_mismatch.last_seen.as_deref(),
            Some("sentinel-42"),
            "last_seen must be unchanged on TPM mismatch"
        );
        assert_eq!(
            after_mismatch.tpm_ek.as_deref(),
            Some("ek-1"),
            "bound EK must not be overwritten by the mismatching attempt"
        );
    }

    #[tokio::test]
    async fn test_checkin_matching_ek_ok() {
        let dir = tempdir().unwrap();
        let registry = Arc::new(MockRegistry::default());
        let mut m = base_machine("aa:bb:cc:dd:ee:ff", "h1");
        m.status = MachineStatus::Approved;
        m.tpm_ek = Some("ek-1".to_string());
        m.last_seen = Some("sentinel-old".to_string());
        registry.upsert_machine(m).await;
        let state = test_state_with(registry.clone(), dir.path().to_path_buf());

        let body = Bytes::from(
            serde_json::to_vec(&json!({"mac": "aa:bb:cc:dd:ee:ff", "tpm_ek": "ek-1", "ip": "10.0.0.9"}))
                .unwrap(),
        );
        let resp = handle_checkin(State(state), body).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let v = body_json(resp).await;
        assert_eq!(v["ok"], true);
        assert_eq!(v["approved"], true);

        let updated = registry.get_machine("aa:bb:cc:dd:ee:ff").await.unwrap();
        assert_ne!(
            updated.last_seen.as_deref(),
            Some("sentinel-old"),
            "legitimate checkin must update last_seen"
        );
        assert_eq!(updated.last_ip.as_deref(), Some("10.0.0.9"));
        assert_eq!(updated.tpm_ek.as_deref(), Some("ek-1"), "EK unchanged on match");
    }

    // ── /api/webhook ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_webhook_missing_ipxe_swallowed() {
        let dir = tempdir().unwrap();
        let registry = Arc::new(MockRegistry::default());
        // No machine registered under this hostname; no iPXE files on disk.
        let state = test_state_with(registry.clone(), dir.path().to_path_buf());

        let body = Bytes::from(
            serde_json::to_vec(&json!({
                "name": "h-missing",
                "status": "success",
                "event_type": "status_update",
                "progress": 100
            }))
            .unwrap(),
        );
        let resp = handle_webhook(State(state), body).await;
        assert_eq!(resp.status(), StatusCode::OK, "flip failure must be swallowed");
        let v = body_json(resp).await;
        assert_eq!(v["ok"], true);

        assert_eq!(
            registry.install_events.lock().unwrap().len(),
            1,
            "install_history still recorded despite missing iPXE file"
        );
    }

    #[tokio::test]
    async fn test_webhook_flip_and_history() {
        let dir = tempdir().unwrap();
        let registry = Arc::new(MockRegistry::default());
        registry
            .upsert_machine(base_machine("aa:bb:cc:dd:ee:ff", "h-present"))
            .await;

        let ipxe_path = dir.path().join("mac-aabbccddeeff.ipxe");
        std::fs::write(&ipxe_path, "#!ipxe\nset menu-default pxe-install\nboot\n").unwrap();

        let state = test_state_with(registry.clone(), dir.path().to_path_buf());
        let body = Bytes::from(
            serde_json::to_vec(&json!({
                "name": "h-present",
                "status": "success",
                "event_type": "status_update",
                "progress": 100
            }))
            .unwrap(),
        );
        let resp = handle_webhook(State(state), body).await;
        assert_eq!(resp.status(), StatusCode::OK);

        let content = std::fs::read_to_string(&ipxe_path).unwrap();
        assert!(
            content.contains("set menu-default boot-local-disk"),
            "iPXE file must be flipped: {content}"
        );

        let events = registry.install_events.lock().unwrap();
        assert_eq!(events.len(), 1, "exactly one install_history record");
        assert_ne!(events[0].event_id, uuid::Uuid::nil(), "event_id must be a minted UUID");
        assert_eq!(events[0].mac, "aa:bb:cc:dd:ee:ff");
    }

    // ── invalid JSON (shared parity across all endpoints) ────────────────

    #[tokio::test]
    async fn test_invalid_json_400() {
        let dir = tempdir().unwrap();
        let state = test_state_with(Arc::new(MockRegistry::default()), dir.path().to_path_buf());

        let resp = handle_register(State(state.clone()), Bytes::from_static(b"not json")).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let v = body_json(resp).await;
        assert_eq!(v["error"], "invalid json");

        let resp2 = handle_checkin(State(state.clone()), Bytes::from_static(b"{bad")).await;
        assert_eq!(resp2.status(), StatusCode::BAD_REQUEST);

        let resp3 = handle_webhook(State(state), Bytes::from_static(b"[1,2")).await;
        assert_eq!(resp3.status(), StatusCode::BAD_REQUEST);
    }

    // ── log-only sinks ────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_log_only_sinks_ok_and_tag_endpoint() {
        let dir = tempdir().unwrap();
        let registry = Arc::new(MockRegistry::default());
        let state = test_state_with(registry.clone(), dir.path().to_path_buf());

        for (path, resp) in [
            (
                "/api/finalreport",
                handle_finalreport(
                    State(state.clone()),
                    Bytes::from(serde_json::to_vec(&json!({"client_id": "c1"})).unwrap()),
                )
                .await,
            ),
            (
                "/api/hardware-info",
                handle_hardware_info(
                    State(state.clone()),
                    Bytes::from(serde_json::to_vec(&json!({"cpu": "x86"})).unwrap()),
                )
                .await,
            ),
            (
                "/api/cloud-init",
                handle_cloud_init(
                    State(state.clone()),
                    Bytes::from(serde_json::to_vec(&json!({"stage": "final"})).unwrap()),
                )
                .await,
            ),
        ] {
            assert_eq!(resp.status(), StatusCode::OK, "{path}");
            let v = body_json(resp).await;
            assert_eq!(v["ok"], true, "{path}");
        }

        let events = registry.events.lock().unwrap();
        assert_eq!(events.len(), 3);
        let endpoints: Vec<&str> = events
            .iter()
            .map(|e| e["endpoint"].as_str().unwrap())
            .collect();
        assert_eq!(
            endpoints,
            vec!["/api/finalreport", "/api/hardware-info", "/api/cloud-init"]
        );
    }
}
