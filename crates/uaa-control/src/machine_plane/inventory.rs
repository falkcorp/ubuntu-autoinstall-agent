// file: crates/uaa-control/src/machine_plane/inventory.rs
// version: 1.2.0
// guid: 76633c84-b337-47da-ab77-11cbf0f4b3b5
// last-edited: 2026-07-13

//! Machine-plane operator/inventory endpoints (`:25000` parity, spec Decision 12).
//!
//! Exact parity with `scripts/autoinstall-agent.py` for: `/api/certs/<hostname>`,
//! `/api/flip/<hostname>`, `/api/approve/<mac>`, `/api/deregister/<mac>`,
//! `/api/registry`, `/api/events`, the six `/api/yubikeys*` routes, the two
//! `/api/tang*` routes, and the machine-plane catch-all `404 {"error":"not
//! found"}` (Python `:558,711`).
//!
//! # Coordinator wiring (read before touching any other file)
//!
//! This module is purely additive and self-contained (install-plane IP-03; the
//! CT-01 stub this fills). `crate::db::registry::RegistryStore` (CT-02) has no
//! method to delete a machine row or to set a yubikey's `approved_at`/`revoked_at`
//! (the `yubikeys` table — `migrations/0001_init.sql` — has no such columns), so
//! this module follows the same wave-4 de-collision pattern `reinstall.rs` (CT-06)
//! documents: it declares its own narrow local [`Registry`] seam instead of
//! blocking on / bending a shared trait it cannot edit.
//!   * Machines route through `crate::db::store` ([`read_snapshot`]/
//!     [`write_snapshot`]/[`StatePaths`]) — the SAME on-disk snapshot
//!     `machine_plane::lifecycle` (IP-02) reads and writes, so a machine
//!     registered via `/api/register` is immediately visible to `/api/approve`,
//!     `/api/deregister`, `/api/certs`, and `/api/flip` here.
//!   * YubiKeys/tang are legacy standalone JSON stores, exactly mirroring
//!     Python's own `YUBIKEY_REGISTRY_FILE`/`TANG_REGISTRY_FILE`
//!     (`scripts/autoinstall-agent.py` `:37-38`) — NOT the CT-01 `SnapshotDoc`
//!     fields of the same name. `YubikeyRow` (`db::mod`) cannot carry
//!     `approved_at`/`revoked_at` without a migration change (out of scope: a
//!     single-file task cannot touch `db/mod.rs`), so this module keeps its own
//!     [`YubikeyEntry`] shape instead. TODO(coordinator): once CT-02's schema
//!     grows `approved_at`/`revoked_at` columns, unify onto `RegistryStore`.
//!   * `/api/events` reads the WAL (`wal.jsonl`, via the shared [`StatePaths`]),
//!     NOT Python's `EVENTS_LOG` file — nothing in this codebase writes that
//!     legacy path anymore, which left this endpoint permanently empty. This
//!     matches `dashboard::FileRegistry::list_events`, which reads the same
//!     WAL; the two used to show different (one live, one always-empty) data.
//!
//! `cockroach cert create-node` is shelled out ONLY through the
//! [`uaa_core::network::CommandExecutor`] seam — never a raw std-library process
//! spawn directly — production wires [`LocalClient`]; tests inject a mock that
//! writes fake `node.crt`/`node.key` into the certs-dir it is pointed at.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::{Path as AxumPath, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use uaa_core::network::{CommandExecutor, LocalClient};

use crate::db::{
    store::{read_snapshot, write_snapshot, StatePaths, WalEntry},
    MachineRow, MachineStatus, TangServerRow,
};

use super::lifecycle::{flip_ipxe_content, normalize_mac};

/// Default iPXE boot-menu directory (mirrors Python's `IPXE_BOOT_DIR`, `:31`).
const IPXE_BOOT_DIR: &str = "/var/www/html/ipxe/boot";
/// Legacy YubiKey registry file (mirrors Python's `YUBIKEY_REGISTRY_FILE`, `:38`).
const YUBIKEY_REGISTRY_FILE: &str = "/var/log/cockroach-autoinstall/yubikey-registry.json";
/// Legacy tang-server registry file (mirrors Python's `TANG_REGISTRY_FILE`, `:39`).
const TANG_REGISTRY_FILE: &str = "/var/log/cockroach-autoinstall/tang-registry.json";
/// CockroachDB node-cert CA (parity-frozen legacy — NOT the install CA of spec
/// Decision 6; Python `:41`). Serves node-cert issuance for the DB cluster only.
const COCKROACH_CA_CRT: &str = "/var/lib/cockroach-autoinstall/.cockroach-ca/ca.crt";
/// CockroachDB node-cert CA key (Python `:42`).
const COCKROACH_CA_KEY: &str = "/var/lib/cockroach-autoinstall/.cockroach-ca/ca.key";

// ── YubiKey entry (own shape — see module doc) ───────────────────────────────

/// A registered YubiKey. Own shape (not [`crate::db::YubikeyRow`]) because the
/// `yubikeys` table has no `approved_at`/`revoked_at` columns; see module doc.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct YubikeyEntry {
    pub fingerprint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpg_pubkey: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_pubkey: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial: Option<String>,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registered_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_at: Option<i64>,
}

impl YubikeyEntry {
    /// The listing/approve-echo view: identical to the full row minus
    /// `gpg_pubkey` (Python `:422`, `:445` — never leak the raw key blob).
    fn without_gpg_pubkey(&self) -> Value {
        let mut v = serde_json::to_value(self).unwrap_or_else(|_| json!({}));
        if let Some(obj) = v.as_object_mut() {
            obj.remove("gpg_pubkey");
        }
        v
    }
}

// ── Registry seam (mockable; no direct DB/process access) ───────────────────

/// Persistence seam for the inventory handlers — see the module-level
/// coordinator-wiring note for why this is a local trait rather than
/// `crate::db::registry::RegistryStore`.
#[async_trait::async_trait]
pub trait Registry: Send + Sync {
    /// Look up a machine row by normalized MAC.
    async fn get_machine(&self, mac: &str) -> Option<MachineRow>;
    /// Look up a machine row by hostname (first match; mirrors Python's
    /// linear `for e in reg.values()` scans, `:295`, `:321`).
    async fn find_machine_by_hostname(&self, hostname: &str) -> Option<MachineRow>;
    /// The whole machine registry (Python `:401-403`).
    async fn list_machines(&self) -> Vec<MachineRow>;
    /// Set `status=approved` + `approved_at`; returns the updated row, or
    /// `None` if `mac` is not registered.
    async fn approve_machine(&self, mac: &str, approved_at: String) -> Option<MachineRow>;
    /// Remove the row for `mac` and return it (for the hostname in the
    /// deregister message), or `None` if `mac` was not registered.
    async fn delete_machine(&self, mac: &str) -> Option<MachineRow>;
    /// Last `limit` events, oldest-first, read from the WAL (`wal.jsonl`) —
    /// the same file `machine_plane::lifecycle::Registry::append_event`
    /// writes to and `dashboard::FileRegistry::list_events` reads (NOT
    /// Python's `EVENTS_LOG`, which nothing writes anymore). A missing WAL
    /// yields an empty vec (never a 500 — Decision 12 collections-vs-single-
    /// resources convention); a corrupt individual line is skipped rather
    /// than zeroing the whole window.
    async fn list_events(&self, limit: usize) -> Vec<Value>;
    /// All registered YubiKeys, keyed by fingerprint.
    async fn list_yubikeys(&self) -> Vec<YubikeyEntry>;
    /// A single YubiKey by (already-uppercased) fingerprint.
    async fn get_yubikey(&self, fingerprint: &str) -> Option<YubikeyEntry>;
    /// Set `status=approved` + `approved_at`; `None` if unknown.
    async fn approve_yubikey(&self, fingerprint: &str, at: i64) -> Option<YubikeyEntry>;
    /// Set `status=revoked` + `revoked_at`; `None` if unknown.
    async fn revoke_yubikey(&self, fingerprint: &str, at: i64) -> Option<YubikeyEntry>;
    /// Upsert from `POST /api/yubikeys/register`: `status`/`registered_at` are
    /// PRESERVED from any existing row (default pending/now); every other
    /// field — including `approved_at`/`revoked_at` — is reset, mirroring
    /// Python's full-dict-literal replace (`:676-684`).
    async fn upsert_yubikey_register(&self, entry: YubikeyEntry) -> YubikeyEntry;
    /// The whole tang-server registry (Python `:487-490`).
    async fn list_tang(&self) -> Vec<TangServerRow>;
    /// Full-replace upsert keyed by hostname (Python `tang[hostname] = {...}`,
    /// `:699` — real last-seen-wins overwrite, not insert-if-absent).
    async fn upsert_tang(&self, row: TangServerRow);
}

/// Real [`Registry`]: machines via CT-01's snapshot (shared with
/// `machine_plane::lifecycle`); yubikeys/tang/events via their own legacy JSON
/// files (see module doc for why these are not the `SnapshotDoc` fields of the
/// same name).
pub struct FileRegistry {
    machine_paths: StatePaths,
    yubikey_file: PathBuf,
    tang_file: PathBuf,
}

impl FileRegistry {
    pub fn new(machine_paths: StatePaths, yubikey_file: PathBuf, tang_file: PathBuf) -> Self {
        Self {
            machine_paths,
            yubikey_file,
            tang_file,
        }
    }

    fn load_yubikeys(&self) -> HashMap<String, YubikeyEntry> {
        load_json_map(&self.yubikey_file)
    }

    fn save_yubikeys(&self, map: &HashMap<String, YubikeyEntry>) {
        save_json_map(&self.yubikey_file, map);
    }

    fn load_tang(&self) -> HashMap<String, TangServerRow> {
        load_json_map(&self.tang_file)
    }

    fn save_tang(&self, map: &HashMap<String, TangServerRow>) {
        save_json_map(&self.tang_file, map);
    }
}

/// Load a `HashMap<String, T>` from a JSON file. A missing OR corrupt file
/// yields an EMPTY map (never a panic) — mirrors Python's `load_*_registry`
/// bare `except: return {}` (`:62-65`, `:79-83`, `:91-95`).
fn load_json_map<T: serde::de::DeserializeOwned>(path: &Path) -> HashMap<String, T> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
        Err(_) => HashMap::new(),
    }
}

/// Atomically persist a `HashMap<String, T>` to a JSON file: tmp write + rename
/// (mirrors Python's `save_*_registry` tmp+`os.replace` idiom, `:67-70`).
fn save_json_map<T: Serialize>(path: &Path, map: &HashMap<String, T>) {
    if let Some(parent) = path.parent() {
        if let Err(err) = std::fs::create_dir_all(parent) {
            tracing::error!(%err, path = %parent.display(), "failed to create registry dir");
            return;
        }
    }
    let bytes = match serde_json::to_vec_pretty(map) {
        Ok(b) => b,
        Err(err) => {
            tracing::error!(%err, "failed to serialize registry");
            return;
        }
    };
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    if let Err(err) = std::fs::write(&tmp, &bytes) {
        tracing::error!(%err, path = %tmp.display(), "failed to write registry tmp file");
        return;
    }
    if let Err(err) = std::fs::rename(&tmp, path) {
        tracing::error!(%err, path = %path.display(), "failed to rename registry tmp file into place");
    }
}

#[async_trait::async_trait]
impl Registry for FileRegistry {
    async fn get_machine(&self, mac: &str) -> Option<MachineRow> {
        let doc = read_snapshot(&self.machine_paths);
        doc.machines.into_iter().find(|m| m.mac == mac)
    }

    async fn find_machine_by_hostname(&self, hostname: &str) -> Option<MachineRow> {
        let doc = read_snapshot(&self.machine_paths);
        doc.machines.into_iter().find(|m| m.hostname == hostname)
    }

    async fn list_machines(&self) -> Vec<MachineRow> {
        read_snapshot(&self.machine_paths).machines
    }

    async fn approve_machine(&self, mac: &str, approved_at: String) -> Option<MachineRow> {
        let mut doc = read_snapshot(&self.machine_paths);
        let row = doc.machines.iter_mut().find(|m| m.mac == mac)?;
        row.status = MachineStatus::Approved;
        row.approved_at = Some(approved_at);
        let updated = row.clone();
        if let Err(err) = write_snapshot(&self.machine_paths, &doc) {
            tracing::error!(%err, "failed to persist machine approval");
        }
        Some(updated)
    }

    async fn delete_machine(&self, mac: &str) -> Option<MachineRow> {
        let mut doc = read_snapshot(&self.machine_paths);
        let idx = doc.machines.iter().position(|m| m.mac == mac)?;
        let removed = doc.machines.remove(idx);
        if let Err(err) = write_snapshot(&self.machine_paths, &doc) {
            tracing::error!(%err, "failed to persist machine deregistration");
        }
        Some(removed)
    }

    async fn list_events(&self, limit: usize) -> Vec<Value> {
        let content = match std::fs::read_to_string(&self.machine_paths.wal) {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };
        let mut events: Vec<Value> = Vec::new();
        for line in content.lines() {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<WalEntry>(line) {
                Ok(entry) if entry.kind == "event" => events.push(entry.payload),
                _ => continue,
            }
        }
        let start = events.len().saturating_sub(limit);
        events.split_off(start)
    }

    async fn list_yubikeys(&self) -> Vec<YubikeyEntry> {
        self.load_yubikeys().into_values().collect()
    }

    async fn get_yubikey(&self, fingerprint: &str) -> Option<YubikeyEntry> {
        self.load_yubikeys().get(fingerprint).cloned()
    }

    async fn approve_yubikey(&self, fingerprint: &str, at: i64) -> Option<YubikeyEntry> {
        let mut map = self.load_yubikeys();
        let entry = map.get_mut(fingerprint)?;
        entry.status = "approved".to_string();
        entry.approved_at = Some(at);
        let updated = entry.clone();
        self.save_yubikeys(&map);
        Some(updated)
    }

    async fn revoke_yubikey(&self, fingerprint: &str, at: i64) -> Option<YubikeyEntry> {
        let mut map = self.load_yubikeys();
        let entry = map.get_mut(fingerprint)?;
        entry.status = "revoked".to_string();
        entry.revoked_at = Some(at);
        let updated = entry.clone();
        self.save_yubikeys(&map);
        Some(updated)
    }

    async fn upsert_yubikey_register(&self, mut entry: YubikeyEntry) -> YubikeyEntry {
        let mut map = self.load_yubikeys();
        let (status, registered_at) = match map.get(&entry.fingerprint) {
            Some(existing) => (
                existing.status.clone(),
                existing.registered_at.or(Some(now_epoch_i64())),
            ),
            None => ("pending".to_string(), Some(now_epoch_i64())),
        };
        entry.status = status;
        entry.registered_at = registered_at;
        entry.approved_at = None;
        entry.revoked_at = None;
        map.insert(entry.fingerprint.clone(), entry.clone());
        self.save_yubikeys(&map);
        entry
    }

    async fn list_tang(&self) -> Vec<TangServerRow> {
        self.load_tang().into_values().collect()
    }

    async fn upsert_tang(&self, row: TangServerRow) {
        let mut map = self.load_tang();
        map.insert(row.hostname.clone(), row);
        self.save_tang(&map);
    }
}

// ── Pure parity helpers ───────────────────────────────────────────────────────

/// Strip separators for the `mac-<hexmac>.ipxe` filename convention (Python
/// `mac_to_hex`, `:75-76`); duplicated from `lifecycle.rs` (private there) —
/// two 1-line copies of a pure function across disjoint wave-4 files is the
/// REUSE note's accepted cost (`:51` of the task brief).
fn mac_to_hex(mac: &str) -> String {
    mac.to_lowercase().replace([':', '-'], "")
}

/// Exact port of Python's `find_ipxe_file_by_hostname` (`:103-113`): registry
/// hostname match first (candidate path is NOT required to exist yet —
/// existence is checked by the caller), then a fallback scan of `*.ipxe` files
/// for a `set hostname <name>` content match.
async fn resolve_ipxe_path(
    registry: &dyn Registry,
    ipxe_dir: &Path,
    hostname: &str,
) -> Option<PathBuf> {
    if let Some(m) = registry.find_machine_by_hostname(hostname).await {
        return Some(ipxe_dir.join(format!("mac-{}.ipxe", mac_to_hex(&m.mac))));
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

/// Exact port of Python's `flip_ipxe(hostname, target)` (`:170-177`), general
/// over `target` (unlike `lifecycle.rs`'s webhook-only flip, which is hardwired
/// to `boot-local-disk`). A missing file (or any IO error) is reported, never
/// panics.
async fn flip_ipxe(
    registry: &dyn Registry,
    ipxe_dir: &Path,
    hostname: &str,
    target: &str,
) -> (bool, String) {
    let path = match resolve_ipxe_path(registry, ipxe_dir, hostname).await {
        Some(p) if p.exists() => p,
        _ => return (false, format!("No iPXE file found for {hostname}")),
    };
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(err) => return (false, format!("flip failed: {err}")),
    };
    let new_content = flip_ipxe_content(&content, target);
    match std::fs::write(&path, new_content) {
        Ok(()) => (true, format!("Flipped {hostname} to {target}")),
        Err(err) => (false, format!("flip failed: {err}")),
    }
}

/// Single-quote shell escaping for argv values interpolated into the
/// [`CommandExecutor`] string-shaped command (it runs via `bash -c`). Prevents
/// shell injection from attacker-controlled `hostname`/`ip` URL segments.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// argv flag-smuggling guard. `hostname`/`ip` flow as POSITIONAL arguments into a
/// flag-parsing CLI (`cockroach cert create-node <host>...`). Shell-quoting stops
/// shell injection but NOT argv smuggling: a value like `--certs-dir=/evil` is a
/// single well-quoted token that cockroach's flag parser still reads as a flag,
/// overriding the real `--certs-dir`/`--ca-key`. Reject anything that isn't a
/// plain host/IP token — empty, leading `-`, or a char outside `[A-Za-z0-9._:-]`.
fn reject_flaglike(kind: &str, s: &str) -> Result<(), String> {
    if s.is_empty()
        || s.starts_with('-')
        || !s
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b':'))
    {
        return Err(format!(
            "invalid {kind} for cert generation (possible argument injection): {s:?}"
        ));
    }
    Ok(())
}

/// Exact port of Python's `generate_certs` (`:236-254`): stage `ca.crt` into a
/// fresh tempdir, shell out `cockroach cert create-node <ip> <hostname>
/// <hostname>.jf.local localhost 127.0.0.1 --certs-dir=<tmpdir>
/// --ca-key=<ca_key>` through the [`CommandExecutor`] seam, base64 the three
/// resulting files. The tempdir is removed on every exit path via
/// [`tempfile::TempDir`]'s `Drop` (mirrors Python's `try/finally`
/// `shutil.rmtree`).
pub async fn generate_certs(
    executor: &mut (dyn CommandExecutor + Send),
    ca_crt: &Path,
    ca_key: &Path,
    hostname: &str,
    ip: &str,
) -> Result<BTreeMap<String, String>, String> {
    // Fail-closed BEFORE staging or shelling out: identity values must be plain
    // host/IP tokens, never flag-like (argv smuggling into cockroach's parser).
    reject_flaglike("hostname", hostname)?;
    reject_flaglike("ip", ip)?;

    let tmpdir = tempfile::tempdir().map_err(|e| e.to_string())?;
    let dest_ca = tmpdir.path().join("ca.crt");
    std::fs::copy(ca_crt, &dest_ca).map_err(|e| e.to_string())?;

    let certs_dir = tmpdir.path().display().to_string();
    let command = format!(
        "cockroach cert create-node {} {} {} localhost 127.0.0.1 --certs-dir={} --ca-key={}",
        shell_quote(ip),
        shell_quote(hostname),
        shell_quote(&format!("{hostname}.jf.local")),
        shell_quote(&certs_dir),
        shell_quote(&ca_key.display().to_string()),
    );

    let (exit_code, _stdout, stderr) = executor
        .execute_with_error_collection(&command, "cockroach cert create-node")
        .await
        .map_err(|e| e.to_string())?;
    if exit_code != 0 {
        return Err(stderr);
    }

    let mut certs = BTreeMap::new();
    for fname in ["ca.crt", "node.crt", "node.key"] {
        let bytes = std::fs::read(tmpdir.path().join(fname)).map_err(|e| e.to_string())?;
        certs.insert(
            fname.to_string(),
            base64::engine::general_purpose::STANDARD.encode(bytes),
        );
    }
    Ok(certs)
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

// ── Router / handler wiring ──────────────────────────────────────────────────

/// Factory for a fresh boxed [`CommandExecutor`] per cert-issuance call
/// (the trait's methods take `&mut self`, so a shared instance would need
/// external locking; a factory keeps each request's executor independent).
type ExecutorFactory = dyn Fn() -> Box<dyn CommandExecutor + Send> + Send + Sync;

#[derive(Clone)]
struct AppState {
    registry: Arc<dyn Registry>,
    ipxe_dir: Arc<PathBuf>,
    ca_crt: Arc<PathBuf>,
    ca_key: Arc<PathBuf>,
    executor_factory: Arc<ExecutorFactory>,
}

fn default_state() -> AppState {
    AppState {
        registry: Arc::new(FileRegistry::new(
            StatePaths::default(),
            PathBuf::from(YUBIKEY_REGISTRY_FILE),
            PathBuf::from(TANG_REGISTRY_FILE),
        )),
        ipxe_dir: Arc::new(PathBuf::from(IPXE_BOOT_DIR)),
        ca_crt: Arc::new(PathBuf::from(COCKROACH_CA_CRT)),
        ca_key: Arc::new(PathBuf::from(COCKROACH_CA_KEY)),
        executor_factory: Arc::new(|| {
            Box::new(LocalClient::new()) as Box<dyn CommandExecutor + Send>
        }),
    }
}

/// The inventory sub-router. Merged into `machine_plane::router()` by the
/// coordinator with `.merge(inventory::router())` (one line, owned by CT-01's
/// `mod.rs` — never edited here). Also carries the machine-plane catch-all
/// `404 {"error":"not found"}` fallback (Python `:558,711`); no sibling
/// submodule sets one today (grep-verified before writing this file).
pub fn router() -> Router {
    build_router(default_state())
}

fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/api/certs/:hostname", get(handle_certs))
        .route("/api/flip/:hostname", get(handle_flip))
        .route("/api/approve/:mac", get(handle_approve))
        .route("/api/deregister/:mac", get(handle_deregister))
        .route("/api/registry", get(handle_registry))
        .route("/api/events", get(handle_events))
        .route("/api/yubikeys", get(handle_yubikeys_list))
        .route("/api/yubikeys/ssh-keys", get(handle_yubikeys_ssh_keys))
        .route("/api/yubikeys/approve/:fp", get(handle_yubikey_approve))
        .route("/api/yubikeys/:fp/pubkey", get(handle_yubikey_pubkey))
        .route("/api/yubikeys/revoke/:fp", get(handle_yubikey_revoke))
        .route("/api/tang/servers", get(handle_tang_servers))
        .route("/api/yubikeys/register", post(handle_yubikey_register))
        .route("/api/tang/checkin", post(handle_tang_checkin))
        .fallback(handle_not_found)
        .with_state(state)
}

fn json_response(code: StatusCode, body: Value) -> Response {
    (code, Json(body)).into_response()
}

/// Machine-plane catch-all (Python `:558` GET fallthrough, `:711` POST
/// fallthrough) — deliberately `{"error":"not found"}`, NOT the `{"ok":false,
/// "error":...}` shape every matched-route error uses.
async fn handle_not_found() -> Response {
    json_response(StatusCode::NOT_FOUND, json!({"error": "not found"}))
}

#[derive(Debug, Deserialize)]
struct CertsQuery {
    #[serde(default)]
    ip: Option<String>,
    #[serde(default)]
    mac: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FlipQuery {
    #[serde(default)]
    target: Option<String>,
}

// ── /api/certs/<hostname> (Python `:286-312`) ────────────────────────────────

async fn handle_certs(
    State(state): State<AppState>,
    AxumPath(hostname): AxumPath<String>,
    Query(q): Query<CertsQuery>,
) -> Response {
    let ip = q.ip.unwrap_or_else(|| "127.0.0.1".to_string());
    let mac_param = q.mac.unwrap_or_default();

    // Order matches Python exactly: mac-param lookup -> hostname-scan fallback.
    let mut entry = None;
    if !mac_param.is_empty() {
        let mac = normalize_mac(&mac_param);
        entry = state.registry.get_machine(&mac).await;
    }
    if entry.is_none() {
        entry = state.registry.find_machine_by_hostname(&hostname).await;
    }
    let entry = match entry {
        None => {
            tracing::info!(%hostname, %ip, "CERTS DENIED - not registered");
            return json_response(
                StatusCode::FORBIDDEN,
                json!({"ok": false, "error": "Not registered. Run register-len-server.sh first."}),
            );
        }
        Some(e) => e,
    };
    if entry.status != MachineStatus::Approved {
        let status_str: String = entry.status.clone().into();
        tracing::info!(%hostname, status = %status_str, "CERTS DENIED - not approved");
        return json_response(
            StatusCode::FORBIDDEN,
            json!({"ok": false, "error": format!("Pending approval. Status: {status_str}.")}),
        );
    }

    let mut executor = (state.executor_factory)();
    match generate_certs(
        executor.as_mut(),
        &state.ca_crt,
        &state.ca_key,
        &hostname,
        &ip,
    )
    .await
    {
        Err(err) => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({"ok": false, "error": err}),
        ),
        Ok(certs) => {
            tracing::info!(%hostname, %ip, "CERTS issued");
            json_response(StatusCode::OK, json!({"ok": true, "certs": certs}))
        }
    }
}

// ── /api/flip/<hostname> (Python `:315-329`) ─────────────────────────────────

async fn handle_flip(
    State(state): State<AppState>,
    AxumPath(hostname): AxumPath<String>,
    Query(q): Query<FlipQuery>,
) -> Response {
    let target = q.target.unwrap_or_else(|| "boot-local-disk".to_string());
    if target == "custom-autoinstall" {
        let approved = state
            .registry
            .find_machine_by_hostname(&hostname)
            .await
            .map(|e| e.status == MachineStatus::Approved)
            .unwrap_or(false);
        if !approved {
            tracing::info!(%hostname, "FLIP TO INSTALL DENIED - not approved");
            return json_response(
                StatusCode::FORBIDDEN,
                json!({"ok": false, "error": "Flip to reinstall requires approved status"}),
            );
        }
    }
    let (ok, msg) = flip_ipxe(state.registry.as_ref(), &state.ipxe_dir, &hostname, &target).await;
    tracing::info!(%hostname, %target, %ok, %msg, "FLIP");
    let code = if ok {
        StatusCode::OK
    } else {
        StatusCode::NOT_FOUND
    };
    json_response(code, json!({"ok": ok, "message": msg}))
}

// ── /api/approve/<mac> (Python `:332-344`) ───────────────────────────────────

async fn handle_approve(
    State(state): State<AppState>,
    AxumPath(mac_raw): AxumPath<String>,
) -> Response {
    let mac = normalize_mac(&mac_raw);
    if state.registry.get_machine(&mac).await.is_none() {
        return json_response(
            StatusCode::NOT_FOUND,
            json!({"ok": false, "error": "MAC not registered"}),
        );
    }
    match state
        .registry
        .approve_machine(&mac, now_epoch_string())
        .await
    {
        None => json_response(
            StatusCode::NOT_FOUND,
            json!({"ok": false, "error": "MAC not registered"}),
        ),
        Some(row) => {
            tracing::info!(%mac, hostname = %row.hostname, "APPROVED");
            json_response(
                StatusCode::OK,
                json!({"ok": true, "message": format!("Approved {mac}"), "entry": row}),
            )
        }
    }
}

// ── /api/deregister/<mac> (Python `:347-359`) ────────────────────────────────

async fn handle_deregister(
    State(state): State<AppState>,
    AxumPath(mac_raw): AxumPath<String>,
) -> Response {
    let mac = normalize_mac(&mac_raw);
    match state.registry.delete_machine(&mac).await {
        None => json_response(
            StatusCode::NOT_FOUND,
            json!({"ok": false, "error": "MAC not registered"}),
        ),
        Some(row) => {
            tracing::info!(%mac, hostname = %row.hostname, "DEREGISTERED");
            json_response(
                StatusCode::OK,
                json!({"ok": true, "message": format!("Deregistered {mac} ({})", row.hostname)}),
            )
        }
    }
}

// ── /api/registry (Python `:401-403`) ────────────────────────────────────────

async fn handle_registry(State(state): State<AppState>) -> Response {
    let machines = state.registry.list_machines().await;
    let mut map = serde_json::Map::new();
    for m in machines {
        let mac = m.mac.clone();
        map.insert(mac, serde_json::to_value(&m).unwrap_or(Value::Null));
    }
    json_response(StatusCode::OK, Value::Object(map))
}

// ── /api/events (Python `:406-412`) ──────────────────────────────────────────

async fn handle_events(State(state): State<AppState>) -> Response {
    let events = state.registry.list_events(50).await;
    json_response(StatusCode::OK, Value::Array(events))
}

// ── /api/yubikeys (Python `:418-424`) ────────────────────────────────────────

async fn handle_yubikeys_list(State(state): State<AppState>) -> Response {
    let yk = state.registry.list_yubikeys().await;
    let mut map = serde_json::Map::new();
    for entry in &yk {
        map.insert(entry.fingerprint.clone(), entry.without_gpg_pubkey());
    }
    json_response(StatusCode::OK, Value::Object(map))
}

// ── /api/yubikeys/ssh-keys (Python `:426-431`) ───────────────────────────────

async fn handle_yubikeys_ssh_keys(State(state): State<AppState>) -> Response {
    let yk = state.registry.list_yubikeys().await;
    let keys: Vec<String> = yk
        .into_iter()
        .filter(|e| e.status == "approved")
        .filter_map(|e| e.ssh_pubkey.filter(|s| !s.is_empty()))
        .collect();
    json_response(StatusCode::OK, json!({"keys": keys}))
}

// ── /api/yubikeys/approve/<fingerprint> (Python `:433-446`) ──────────────────

async fn handle_yubikey_approve(
    State(state): State<AppState>,
    AxumPath(fp_raw): AxumPath<String>,
) -> Response {
    let fp = fp_raw.to_uppercase();
    match state.registry.approve_yubikey(&fp, now_epoch_i64()).await {
        None => json_response(
            StatusCode::NOT_FOUND,
            json!({"ok": false, "error": "Fingerprint not registered"}),
        ),
        Some(entry) => {
            tracing::info!(fingerprint = %fp, comment = ?entry.comment, "YUBIKEY APPROVED");
            json_response(
                StatusCode::OK,
                json!({"ok": true, "fingerprint": fp, "entry": entry.without_gpg_pubkey()}),
            )
        }
    }
}

// ── /api/yubikeys/<FP>/pubkey (Python `:448-465`) ────────────────────────────

/// Python's route regex is `^/api/yubikeys/([A-F0-9]+)/pubkey$` — a lowercase
/// or otherwise malformed fingerprint never matches that route at all, so it
/// falls through to the generic catch-all `{"error":"not found"}`, NOT the
/// `{"ok":false,"error":"No GPG key..."}` this handler uses for a
/// well-formed-but-unknown fingerprint.
fn is_valid_fp_format(fp: &str) -> bool {
    !fp.is_empty()
        && fp
            .chars()
            .all(|c| c.is_ascii_digit() || ('A'..='F').contains(&c))
}

async fn handle_yubikey_pubkey(
    State(state): State<AppState>,
    AxumPath(fp): AxumPath<String>,
) -> Response {
    if !is_valid_fp_format(&fp) {
        return handle_not_found().await;
    }
    let entry = state.registry.get_yubikey(&fp).await;
    let entry = match entry {
        Some(e) if e.gpg_pubkey.as_deref().is_some_and(|k| !k.is_empty()) => e,
        _ => {
            return json_response(
                StatusCode::NOT_FOUND,
                json!({"ok": false, "error": "No GPG key for that fingerprint"}),
            )
        }
    };
    if entry.status != "approved" {
        return json_response(
            StatusCode::FORBIDDEN,
            json!({"ok": false, "error": "YubiKey not approved"}),
        );
    }
    let body = entry.gpg_pubkey.unwrap_or_default();
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "application/pgp-keys")
        .body(axum::body::Body::from(body))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

// ── /api/yubikeys/revoke/<fingerprint> (Python `:467-480`) ───────────────────

async fn handle_yubikey_revoke(
    State(state): State<AppState>,
    AxumPath(fp_raw): AxumPath<String>,
) -> Response {
    let fp = fp_raw.to_uppercase();
    match state.registry.revoke_yubikey(&fp, now_epoch_i64()).await {
        None => json_response(
            StatusCode::NOT_FOUND,
            json!({"ok": false, "error": "Fingerprint not registered"}),
        ),
        Some(entry) => {
            tracing::info!(fingerprint = %fp, comment = ?entry.comment, "YUBIKEY REVOKED");
            json_response(
                StatusCode::OK,
                json!({"ok": true, "message": format!("Revoked {fp}")}),
            )
        }
    }
}

// ── /api/tang/servers (Python `:487-490`) ────────────────────────────────────

async fn handle_tang_servers(State(state): State<AppState>) -> Response {
    let tang = state.registry.list_tang().await;
    let mut map = serde_json::Map::new();
    for row in tang {
        map.insert(
            row.hostname.clone(),
            serde_json::to_value(&row).unwrap_or(Value::Null),
        );
    }
    json_response(StatusCode::OK, Value::Object(map))
}

// ── POST /api/yubikeys/register (Python `:660-689`) ──────────────────────────

/// Parse the raw request body as JSON, or short-circuit with the same
/// `400 {"error": "invalid json"}` shape every legacy POST route shares
/// (Python's `do_POST` dispatcher, `:564-568`).
#[allow(clippy::result_large_err)]
fn parse_json(body: &Bytes) -> Result<Value, Response> {
    serde_json::from_slice(body)
        .map_err(|_| json_response(StatusCode::BAD_REQUEST, json!({"error": "invalid json"})))
}

async fn handle_yubikey_register(State(state): State<AppState>, body: Bytes) -> Response {
    let data = match parse_json(&body) {
        Ok(v) => v,
        Err(r) => return r,
    };

    let fp = data
        .get("fingerprint")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_uppercase()
        .replace(' ', "");
    if fp.is_empty() {
        return json_response(
            StatusCode::BAD_REQUEST,
            json!({"ok": false, "error": "fingerprint required"}),
        );
    }

    let entry = YubikeyEntry {
        fingerprint: fp.clone(),
        gpg_pubkey: data
            .get("gpg_pubkey")
            .and_then(Value::as_str)
            .map(str::to_string),
        ssh_pubkey: data
            .get("ssh_pubkey")
            .and_then(Value::as_str)
            .map(str::to_string),
        comment: data
            .get("comment")
            .and_then(Value::as_str)
            .map(str::to_string),
        serial: data
            .get("serial")
            .and_then(Value::as_str)
            .map(str::to_string),
        status: String::new(), // overwritten by upsert_yubikey_register
        registered_at: None,   // overwritten by upsert_yubikey_register
        approved_at: None,
        revoked_at: None,
    };
    let saved = state.registry.upsert_yubikey_register(entry).await;

    let approve_url = format!("http://172.16.2.30:25000/api/yubikeys/approve/{fp}");
    tracing::info!(fingerprint = %fp, status = %saved.status, "YUBIKEY register");
    json_response(
        StatusCode::OK,
        json!({
            "ok": true,
            "status": saved.status,
            "message": format!("Registered. Approve with: curl {approve_url}"),
        }),
    )
}

// ── POST /api/tang/checkin (Python `:692-709`) ────────────────────────────────

async fn handle_tang_checkin(State(state): State<AppState>, body: Bytes) -> Response {
    let data = match parse_json(&body) {
        Ok(v) => v,
        Err(r) => return r,
    };

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
    let tang_url = data
        .get("tang_url")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| format!("http://{ip}"));
    let adv_keys = data.get("adv_keys").cloned();
    let adv_keys_count = adv_keys
        .as_ref()
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);

    let row = TangServerRow {
        hostname: hostname.clone(),
        ip: Some(ip.clone()),
        tang_url: Some(tang_url),
        adv_keys,
        last_seen: Some(now_epoch_string()),
    };
    state.registry.upsert_tang(row).await;

    tracing::info!(%hostname, %ip, keys = adv_keys_count, "TANG CHECKIN");
    json_response(StatusCode::OK, json!({"ok": true}))
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tempfile::tempdir;
    use uaa_core::Result as CoreResult;

    // ── in-memory mock Registry — zero filesystem, zero CRDB ─────────────

    #[derive(Default)]
    struct MockRegistry {
        machines: Mutex<HashMap<String, MachineRow>>,
        yubikeys: Mutex<HashMap<String, YubikeyEntry>>,
        tang: Mutex<HashMap<String, TangServerRow>>,
        events: Mutex<Vec<Value>>,
    }

    #[async_trait::async_trait]
    impl Registry for MockRegistry {
        async fn get_machine(&self, mac: &str) -> Option<MachineRow> {
            self.machines.lock().unwrap().get(mac).cloned()
        }
        async fn find_machine_by_hostname(&self, hostname: &str) -> Option<MachineRow> {
            self.machines
                .lock()
                .unwrap()
                .values()
                .find(|m| m.hostname == hostname)
                .cloned()
        }
        async fn list_machines(&self) -> Vec<MachineRow> {
            self.machines.lock().unwrap().values().cloned().collect()
        }
        async fn approve_machine(&self, mac: &str, approved_at: String) -> Option<MachineRow> {
            let mut st = self.machines.lock().unwrap();
            let row = st.get_mut(mac)?;
            row.status = MachineStatus::Approved;
            row.approved_at = Some(approved_at);
            Some(row.clone())
        }
        async fn delete_machine(&self, mac: &str) -> Option<MachineRow> {
            self.machines.lock().unwrap().remove(mac)
        }
        async fn list_events(&self, limit: usize) -> Vec<Value> {
            let events = self.events.lock().unwrap();
            let start = events.len().saturating_sub(limit);
            events[start..].to_vec()
        }
        async fn list_yubikeys(&self) -> Vec<YubikeyEntry> {
            self.yubikeys.lock().unwrap().values().cloned().collect()
        }
        async fn get_yubikey(&self, fingerprint: &str) -> Option<YubikeyEntry> {
            self.yubikeys.lock().unwrap().get(fingerprint).cloned()
        }
        async fn approve_yubikey(&self, fingerprint: &str, at: i64) -> Option<YubikeyEntry> {
            let mut st = self.yubikeys.lock().unwrap();
            let e = st.get_mut(fingerprint)?;
            e.status = "approved".to_string();
            e.approved_at = Some(at);
            Some(e.clone())
        }
        async fn revoke_yubikey(&self, fingerprint: &str, at: i64) -> Option<YubikeyEntry> {
            let mut st = self.yubikeys.lock().unwrap();
            let e = st.get_mut(fingerprint)?;
            e.status = "revoked".to_string();
            e.revoked_at = Some(at);
            Some(e.clone())
        }
        async fn upsert_yubikey_register(&self, mut entry: YubikeyEntry) -> YubikeyEntry {
            let mut st = self.yubikeys.lock().unwrap();
            let (status, registered_at) = match st.get(&entry.fingerprint) {
                Some(existing) => (existing.status.clone(), existing.registered_at.or(Some(1))),
                None => ("pending".to_string(), Some(1)),
            };
            entry.status = status;
            entry.registered_at = registered_at;
            entry.approved_at = None;
            entry.revoked_at = None;
            st.insert(entry.fingerprint.clone(), entry.clone());
            entry
        }
        async fn list_tang(&self) -> Vec<TangServerRow> {
            self.tang.lock().unwrap().values().cloned().collect()
        }
        async fn upsert_tang(&self, row: TangServerRow) {
            self.tang.lock().unwrap().insert(row.hostname.clone(), row);
        }
    }

    // ── mock CommandExecutor — writes fake certs, records argv ───────────

    fn extract_certs_dir(command: &str) -> Option<String> {
        // `shell_quote` wraps the VALUE, not the whole `--flag=value` token, so
        // the trim must happen AFTER stripping the flag prefix (e.g.
        // `--certs-dir='/tmp/x'` -> `'/tmp/x'` -> `/tmp/x`).
        for token in command.split_whitespace() {
            if let Some(dir) = token.strip_prefix("--certs-dir=") {
                return Some(dir.trim_matches('\'').to_string());
            }
        }
        None
    }

    // ── fixtures ───────────────────────────────────────────────────────

    fn base_machine(mac: &str, hostname: &str, status: MachineStatus) -> MachineRow {
        MachineRow {
            mac: mac.to_string(),
            hostname: hostname.to_string(),
            ip: Some("10.0.0.1".to_string()),
            r#type: "lenovo".to_string(),
            status,
            boot_target: crate::db::BootTarget::LocalDisk,
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

    fn test_state(
        registry: Arc<dyn Registry>,
        ipxe_dir: PathBuf,
        recorded: Arc<Mutex<Vec<String>>>,
    ) -> AppState {
        AppState {
            registry,
            ipxe_dir: Arc::new(ipxe_dir),
            ca_crt: Arc::new(PathBuf::new()),
            ca_key: Arc::new(PathBuf::new()),
            executor_factory: Arc::new(move || {
                let recorded = recorded.clone();
                Box::new(RecordingExecutor { recorded }) as Box<dyn CommandExecutor + Send>
            }),
        }
    }

    /// Wraps [`MockExecutor`] so its recorded argv is visible to the test after
    /// the (moved, boxed) executor is dropped.
    struct RecordingExecutor {
        recorded: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait::async_trait]
    impl CommandExecutor for RecordingExecutor {
        async fn connect(&mut self, _host: &str, _username: &str) -> CoreResult<()> {
            Ok(())
        }
        async fn execute(&mut self, _command: &str) -> CoreResult<()> {
            Ok(())
        }
        async fn execute_with_output(&mut self, _command: &str) -> CoreResult<String> {
            Ok(String::new())
        }
        async fn execute_with_error_collection(
            &mut self,
            command: &str,
            _description: &str,
        ) -> CoreResult<(i32, String, String)> {
            self.recorded.lock().unwrap().push(command.to_string());
            if let Some(dir) = extract_certs_dir(command) {
                let _ = std::fs::write(Path::new(&dir).join("node.crt"), b"FAKE-NODE-CRT");
                let _ = std::fs::write(Path::new(&dir).join("node.key"), b"FAKE-NODE-KEY");
            }
            Ok((0, String::new(), String::new()))
        }
        async fn check_silent(&mut self, _command: &str) -> CoreResult<bool> {
            Ok(true)
        }
        async fn collect_debug_info(&mut self) -> CoreResult<String> {
            Ok(String::new())
        }
        async fn upload_file(&mut self, _local: &str, _remote: &str) -> CoreResult<()> {
            Ok(())
        }
        async fn download_file(&mut self, _remote: &str, _local: &str) -> CoreResult<()> {
            Ok(())
        }
        fn disconnect(&mut self) {}
    }

    async fn body_json(resp: Response) -> Value {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    fn write_ipxe(dir: &Path, filename: &str, content: &str) -> PathBuf {
        let p = dir.join(filename);
        std::fs::write(&p, content).unwrap();
        p
    }

    #[test]
    fn test_router_builds_standalone() {
        // Constructing the router touches no filesystem/network — only requests do.
        let _ = router();
    }

    /// Mirrors the EXACT merge shape `machine_plane::mod.rs` performs at wiring
    /// time (`.route("/healthz", ...).merge(lifecycle::router()).merge(inventory::router())`)
    /// — proves no route/method collides with `lifecycle`'s POST routes and that
    /// this module's `.fallback()` merges cleanly (neither `/healthz` nor
    /// `lifecycle::router()` sets one, so axum does not see two competing
    /// fallbacks).
    #[test]
    fn test_merges_into_machine_plane_router() {
        let _: axum::Router = axum::Router::new()
            .route(
                "/healthz",
                get(|| async { Json(json!({"service": "uaa-control"})) }),
            )
            .merge(super::super::lifecycle::router())
            .merge(router());
    }

    // ── /api/certs ────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_certs_unregistered_403() {
        let dir = tempdir().unwrap();
        let registry = Arc::new(MockRegistry::default());
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let state = test_state(registry, dir.path().to_path_buf(), recorded);

        let resp = handle_certs(
            State(state),
            AxumPath("ghost-host".to_string()),
            Query(CertsQuery {
                ip: None,
                mac: None,
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        let v = body_json(resp).await;
        assert_eq!(v["ok"], false);
        assert_eq!(
            v["error"],
            "Not registered. Run register-len-server.sh first."
        );
    }

    #[tokio::test]
    async fn test_certs_unapproved_403() {
        let dir = tempdir().unwrap();
        let registry = Arc::new(MockRegistry::default());
        registry.machines.lock().unwrap().insert(
            "aa:bb:cc:dd:ee:ff".to_string(),
            base_machine("aa:bb:cc:dd:ee:ff", "pending-host", MachineStatus::Pending),
        );
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let state = test_state(registry, dir.path().to_path_buf(), recorded);

        let resp = handle_certs(
            State(state),
            AxumPath("pending-host".to_string()),
            Query(CertsQuery {
                ip: None,
                mac: None,
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        let v = body_json(resp).await;
        assert_eq!(v["ok"], false);
        assert_eq!(v["error"], "Pending approval. Status: pending.");
    }

    #[tokio::test]
    async fn test_certs_approved_issues() {
        let dir = tempdir().unwrap();
        let registry = Arc::new(MockRegistry::default());
        registry.machines.lock().unwrap().insert(
            "aa:bb:cc:dd:ee:ff".to_string(),
            base_machine("aa:bb:cc:dd:ee:ff", "ok-host", MachineStatus::Approved),
        );
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let ca_dir = tempdir().unwrap();
        std::fs::write(ca_dir.path().join("ca.crt"), b"FAKE-CA").unwrap();
        let mut state = test_state(registry, dir.path().to_path_buf(), recorded.clone());
        state.ca_crt = Arc::new(ca_dir.path().join("ca.crt"));
        state.ca_key = Arc::new(ca_dir.path().join("ca.key"));

        let resp = handle_certs(
            State(state),
            AxumPath("ok-host".to_string()),
            Query(CertsQuery {
                ip: Some("10.0.0.9".to_string()),
                mac: None,
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let v = body_json(resp).await;
        assert_eq!(v["ok"], true);
        let certs = v["certs"].as_object().unwrap();
        assert_eq!(certs.len(), 3);
        assert!(certs.contains_key("ca.crt"));
        assert!(certs.contains_key("node.crt"));
        assert!(certs.contains_key("node.key"));

        let cmds = recorded.lock().unwrap();
        assert_eq!(cmds.len(), 1);
        assert!(cmds[0].contains("cert create-node"));
        assert!(cmds[0].contains("ok-host.jf.local"));
    }

    // ── /api/flip ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_flip_install_requires_approved() {
        let dir = tempdir().unwrap();
        write_ipxe(
            dir.path(),
            "mac-aabbccddeeff.ipxe",
            "#!ipxe\nset menu-default pxe-install\nboot\n",
        );
        let registry = Arc::new(MockRegistry::default());
        registry.machines.lock().unwrap().insert(
            "aa:bb:cc:dd:ee:ff".to_string(),
            base_machine("aa:bb:cc:dd:ee:ff", "flip-host", MachineStatus::Pending),
        );
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let state = test_state(registry.clone(), dir.path().to_path_buf(), recorded);

        let resp = handle_flip(
            State(state.clone()),
            AxumPath("flip-host".to_string()),
            Query(FlipQuery {
                target: Some("custom-autoinstall".to_string()),
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        let v = body_json(resp).await;
        assert_eq!(v["error"], "Flip to reinstall requires approved status");

        // Anti-over-suppression: approve, then the SAME target must succeed.
        registry
            .approve_machine("aa:bb:cc:dd:ee:ff", "123".to_string())
            .await
            .unwrap();
        let resp2 = handle_flip(
            State(state),
            AxumPath("flip-host".to_string()),
            Query(FlipQuery {
                target: Some("custom-autoinstall".to_string()),
            }),
        )
        .await;
        assert_eq!(resp2.status(), StatusCode::OK);
        let v2 = body_json(resp2).await;
        assert_eq!(v2["ok"], true);
        let content = std::fs::read_to_string(dir.path().join("mac-aabbccddeeff.ipxe")).unwrap();
        assert!(content.contains("set menu-default custom-autoinstall"));
    }

    #[tokio::test]
    async fn test_flip_local_disk_no_registration_needed() {
        let dir = tempdir().unwrap();
        write_ipxe(
            dir.path(),
            "some-file.ipxe",
            "#!ipxe\nset hostname unreg-host\nset menu-default pxe-install\nboot\n",
        );
        let registry = Arc::new(MockRegistry::default());
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let state = test_state(registry, dir.path().to_path_buf(), recorded);

        let resp = handle_flip(
            State(state),
            AxumPath("unreg-host".to_string()),
            Query(FlipQuery { target: None }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let v = body_json(resp).await;
        assert_eq!(v["ok"], true);
        let content = std::fs::read_to_string(dir.path().join("some-file.ipxe")).unwrap();
        assert!(content.contains("set menu-default boot-local-disk"));
    }

    #[tokio::test]
    async fn test_flip_missing_file_404() {
        let dir = tempdir().unwrap();
        let registry = Arc::new(MockRegistry::default());
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let state = test_state(registry, dir.path().to_path_buf(), recorded);

        let resp = handle_flip(
            State(state),
            AxumPath("no-such-host".to_string()),
            Query(FlipQuery { target: None }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let v = body_json(resp).await;
        assert_eq!(v["ok"], false);
    }

    // ── /api/approve, /api/deregister ────────────────────────────────

    #[tokio::test]
    async fn test_approve_unknown_404() {
        let dir = tempdir().unwrap();
        let registry = Arc::new(MockRegistry::default());
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let state = test_state(registry, dir.path().to_path_buf(), recorded);

        let resp = handle_approve(State(state), AxumPath("aa:bb:cc:dd:ee:ff".to_string())).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let v = body_json(resp).await;
        assert_eq!(v["ok"], false);
        assert_eq!(v["error"], "MAC not registered");
    }

    #[tokio::test]
    async fn test_approve_sets_status() {
        let dir = tempdir().unwrap();
        let registry = Arc::new(MockRegistry::default());
        registry.machines.lock().unwrap().insert(
            "aa:bb:cc:dd:ee:ff".to_string(),
            base_machine("aa:bb:cc:dd:ee:ff", "approve-host", MachineStatus::Pending),
        );
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let state = test_state(registry, dir.path().to_path_buf(), recorded);

        let resp = handle_approve(State(state), AxumPath("AA-BB-CC-DD-EE-FF".to_string())).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let v = body_json(resp).await;
        assert_eq!(v["ok"], true);
        assert_eq!(v["message"], "Approved aa:bb:cc:dd:ee:ff");
        assert_eq!(v["entry"]["status"], "approved");
        assert!(v["entry"]["approved_at"].is_string());
    }

    #[tokio::test]
    async fn test_deregister_unknown_404() {
        let dir = tempdir().unwrap();
        let registry = Arc::new(MockRegistry::default());
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let state = test_state(registry, dir.path().to_path_buf(), recorded);

        let resp = handle_deregister(State(state), AxumPath("aa:bb:cc:dd:ee:ff".to_string())).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let v = body_json(resp).await;
        assert_eq!(v["error"], "MAC not registered");
    }

    #[tokio::test]
    async fn test_deregister_removes_only_registry_row() {
        let dir = tempdir().unwrap();
        let registry = Arc::new(MockRegistry::default());
        registry.machines.lock().unwrap().insert(
            "aa:bb:cc:dd:ee:ff".to_string(),
            base_machine("aa:bb:cc:dd:ee:ff", "gone-host", MachineStatus::Approved),
        );
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let state = test_state(registry.clone(), dir.path().to_path_buf(), recorded);

        let resp = handle_deregister(State(state), AxumPath("aa:bb:cc:dd:ee:ff".to_string())).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let v = body_json(resp).await;
        assert_eq!(v["message"], "Deregistered aa:bb:cc:dd:ee:ff (gone-host)");
        assert!(registry.get_machine("aa:bb:cc:dd:ee:ff").await.is_none());
    }

    // ── /api/registry, /api/events ───────────────────────────────────

    #[tokio::test]
    async fn test_registry_empty_map_200() {
        let dir = tempdir().unwrap();
        let registry = Arc::new(MockRegistry::default());
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let state = test_state(registry, dir.path().to_path_buf(), recorded);

        let resp = handle_registry(State(state)).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let v = body_json(resp).await;
        assert_eq!(v, json!({}));
    }

    #[tokio::test]
    async fn test_events_missing_wal_empty_200() {
        // Real FileRegistry pointed at a StatePaths whose wal.jsonl doesn't
        // exist yet — the common case before anything has been ingested.
        let dir = tempdir().unwrap();
        let fr = FileRegistry::new(
            StatePaths::under(dir.path()),
            dir.path().join("yubikeys.json"),
            dir.path().join("tang.json"),
        );
        let events = fr.list_events(50).await;
        assert!(events.is_empty());

        // Handler-level check via the mock (empty events vec -> 200 []).
        let registry = Arc::new(MockRegistry::default());
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let state = test_state(registry, dir.path().to_path_buf(), recorded);
        let resp = handle_events(State(state)).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let v = body_json(resp).await;
        assert_eq!(v, json!([]));
    }

    #[tokio::test]
    async fn test_events_reads_wal_skips_corrupt_and_non_event_lines() {
        // /api/events must read the SAME wal.jsonl the dashboard reads (not
        // the dead legacy events.jsonl nothing writes to anymore), keep only
        // kind=="event" entries, and skip a corrupt line rather than zeroing
        // the whole window.
        let dir = tempdir().unwrap();
        let paths = StatePaths::under(dir.path());
        let lines = [
            r#"{"event_id":"11111111-1111-1111-1111-111111111111","kind":"event","payload":{"msg":"first"},"at":"1"}"#,
            "not json",
            r#"{"event_id":"22222222-2222-2222-2222-222222222222","kind":"install_history","payload":{"msg":"ignored"},"at":"2"}"#,
            r#"{"event_id":"33333333-3333-3333-3333-333333333333","kind":"event","payload":{"msg":"second"},"at":"3"}"#,
        ];
        std::fs::write(&paths.wal, lines.join("\n") + "\n").unwrap();
        let fr = FileRegistry::new(
            paths,
            dir.path().join("yubikeys.json"),
            dir.path().join("tang.json"),
        );

        let events = fr.list_events(50).await;
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["msg"], "first");
        assert_eq!(events[1]["msg"], "second");
    }

    // ── yubikeys ──────────────────────────────────────────────────────

    fn sample_yubikey(
        fp: &str,
        status: &str,
        gpg: Option<&str>,
        ssh: Option<&str>,
    ) -> YubikeyEntry {
        YubikeyEntry {
            fingerprint: fp.to_string(),
            gpg_pubkey: gpg.map(str::to_string),
            ssh_pubkey: ssh.map(str::to_string),
            comment: Some("test key".to_string()),
            serial: Some("12345".to_string()),
            status: status.to_string(),
            registered_at: Some(1000),
            approved_at: None,
            revoked_at: None,
        }
    }

    #[tokio::test]
    async fn test_yubikey_listing_strips_gpg() {
        let dir = tempdir().unwrap();
        let registry = Arc::new(MockRegistry::default());
        registry.yubikeys.lock().unwrap().insert(
            "DEADBEEF".to_string(),
            sample_yubikey(
                "DEADBEEF",
                "approved",
                Some("-----BEGIN PGP PUBLIC KEY-----\nabc\n"),
                Some("ssh-ed25519 AAAA"),
            ),
        );
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let state = test_state(registry, dir.path().to_path_buf(), recorded);

        let resp = handle_yubikeys_list(State(state.clone())).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let v = body_json(resp).await;
        let entry = &v["DEADBEEF"];
        assert!(
            entry.get("gpg_pubkey").is_none(),
            "gpg_pubkey must be stripped from listing"
        );
        assert_eq!(entry["status"], "approved");

        // /pubkey still returns the armored block for an approved fp.
        let resp2 =
            handle_yubikey_pubkey(State(state.clone()), AxumPath("DEADBEEF".to_string())).await;
        assert_eq!(resp2.status(), StatusCode::OK);
        assert_eq!(
            resp2.headers().get("content-type").unwrap(),
            "application/pgp-keys"
        );
        let bytes = axum::body::to_bytes(resp2.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(String::from_utf8_lossy(&bytes).contains("BEGIN PGP PUBLIC KEY"));

        // Unapproved fp -> 403 on /pubkey.
        let state2 = state.clone();
        state2
            .registry
            .upsert_yubikey_register(sample_yubikey("ABCDEF01", "pending", Some("armored"), None))
            .await;
        let resp3 = handle_yubikey_pubkey(State(state2), AxumPath("ABCDEF01".to_string())).await;
        assert_eq!(resp3.status(), StatusCode::FORBIDDEN);
        let v3 = body_json(resp3).await;
        assert_eq!(v3["error"], "YubiKey not approved");
    }

    #[tokio::test]
    async fn test_yubikey_pubkey_lowercase_falls_through_404() {
        let dir = tempdir().unwrap();
        let registry = Arc::new(MockRegistry::default());
        registry.yubikeys.lock().unwrap().insert(
            "DEADBEEF".to_string(),
            sample_yubikey("DEADBEEF", "approved", Some("armored"), None),
        );
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let state = test_state(registry, dir.path().to_path_buf(), recorded);

        // Lowercase never matches Python's `[A-F0-9]+` route regex -> generic catch-all.
        let resp = handle_yubikey_pubkey(State(state), AxumPath("deadbeef".to_string())).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let v = body_json(resp).await;
        assert_eq!(v, json!({"error": "not found"}));
    }

    #[tokio::test]
    async fn test_yubikey_approve_and_revoke_unknown_404() {
        let dir = tempdir().unwrap();
        let registry = Arc::new(MockRegistry::default());
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let state = test_state(registry, dir.path().to_path_buf(), recorded);

        let resp = handle_yubikey_approve(State(state.clone()), AxumPath("nope".to_string())).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let v = body_json(resp).await;
        assert_eq!(v["error"], "Fingerprint not registered");

        let resp2 = handle_yubikey_revoke(State(state), AxumPath("nope".to_string())).await;
        assert_eq!(resp2.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_yubikey_ssh_keys_only_approved_nonempty() {
        let dir = tempdir().unwrap();
        let registry = Arc::new(MockRegistry::default());
        {
            let mut yk = registry.yubikeys.lock().unwrap();
            yk.insert(
                "A".to_string(),
                sample_yubikey("A", "approved", None, Some("ssh-key-a")),
            );
            yk.insert(
                "B".to_string(),
                sample_yubikey("B", "pending", None, Some("ssh-key-b")),
            );
            yk.insert(
                "C".to_string(),
                sample_yubikey("C", "approved", None, Some("")),
            );
        }
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let state = test_state(registry, dir.path().to_path_buf(), recorded);

        let resp = handle_yubikeys_ssh_keys(State(state)).await;
        let v = body_json(resp).await;
        let keys = v["keys"].as_array().unwrap();
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0], "ssh-key-a");
    }

    #[tokio::test]
    async fn test_yubikey_register_upsert_preserves_status() {
        let dir = tempdir().unwrap();
        let registry = Arc::new(MockRegistry::default());
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let state = test_state(registry.clone(), dir.path().to_path_buf(), recorded);

        let body = Bytes::from(serde_json::to_vec(&json!({"fingerprint": " dead beef "})).unwrap());
        let resp = handle_yubikey_register(State(state.clone()), body).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let v = body_json(resp).await;
        assert_eq!(v["status"], "pending");

        registry.approve_yubikey("DEADBEEF", 555).await.unwrap();

        // Re-register: status must be PRESERVED (approved), not reset to pending.
        let body2 = Bytes::from(
            serde_json::to_vec(&json!({"fingerprint": "DEADBEEF", "comment": "updated"})).unwrap(),
        );
        let resp2 = handle_yubikey_register(State(state), body2).await;
        let v2 = body_json(resp2).await;
        assert_eq!(v2["status"], "approved");
    }

    #[tokio::test]
    async fn test_yubikey_register_missing_fingerprint_400() {
        let dir = tempdir().unwrap();
        let registry = Arc::new(MockRegistry::default());
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let state = test_state(registry, dir.path().to_path_buf(), recorded);

        let body = Bytes::from(serde_json::to_vec(&json!({})).unwrap());
        let resp = handle_yubikey_register(State(state), body).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let v = body_json(resp).await;
        assert_eq!(v["error"], "fingerprint required");
    }

    // ── tang ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_tang_checkin_and_list() {
        let dir = tempdir().unwrap();
        let registry = Arc::new(MockRegistry::default());
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let state = test_state(registry, dir.path().to_path_buf(), recorded);

        let body = Bytes::from(
            serde_json::to_vec(
                &json!({"hostname": "tang1", "ip": "10.0.0.5", "adv_keys": [1, 2, 3]}),
            )
            .unwrap(),
        );
        let resp = handle_tang_checkin(State(state.clone()), body).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let v = body_json(resp).await;
        assert_eq!(v["ok"], true);

        let resp2 = handle_tang_servers(State(state)).await;
        let v2 = body_json(resp2).await;
        assert_eq!(v2["tang1"]["ip"], "10.0.0.5");
        assert_eq!(v2["tang1"]["adv_keys"].as_array().unwrap().len(), 3);
    }

    // ── invalid JSON / catch-all ──────────────────────────────────────

    #[tokio::test]
    async fn test_post_invalid_json_400() {
        let dir = tempdir().unwrap();
        let registry = Arc::new(MockRegistry::default());
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let state = test_state(registry, dir.path().to_path_buf(), recorded);

        let resp =
            handle_yubikey_register(State(state.clone()), Bytes::from_static(b"not json")).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let resp2 = handle_tang_checkin(State(state), Bytes::from_static(b"{bad")).await;
        assert_eq!(resp2.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_catch_all_404_shape() {
        let resp = handle_not_found().await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let v = body_json(resp).await;
        assert_eq!(v, json!({"error": "not found"}));
    }

    #[test]
    fn test_reject_flaglike_blocks_argv_injection() {
        // Flag-like identities must be refused before any cert command is built.
        for evil in ["--certs-dir=/evil", "-x", "a b", "a;b", "$(id)", "a/b"] {
            assert!(
                reject_flaglike("hostname", evil).is_err(),
                "allowed: {evil:?}"
            );
        }
        // Anti-over-suppression: legitimate host/IP tokens still pass.
        for good in [
            "len-serv-001",
            "len-serv-001.jf.local",
            "172.16.2.30",
            "fe80::1",
            "host_1",
        ] {
            assert!(
                reject_flaglike("hostname", good).is_ok(),
                "rejected: {good:?}"
            );
        }
    }

    #[tokio::test]
    async fn test_generate_certs_rejects_flaglike_before_exec() {
        // A flag-like hostname records ZERO executor commands (fail-closed before shell-out).
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let mut exec = RecordingExecutor {
            recorded: recorded.clone(),
        };
        let ca = Path::new("/tmp/ca.crt");
        let err = generate_certs(&mut exec, ca, ca, "--certs-dir=/evil", "172.16.2.30")
            .await
            .expect_err("flag-like hostname must be rejected");
        assert!(err.contains("argument injection"), "{err}");
        assert_eq!(recorded.lock().unwrap().len(), 0, "no command should run");
    }
}
