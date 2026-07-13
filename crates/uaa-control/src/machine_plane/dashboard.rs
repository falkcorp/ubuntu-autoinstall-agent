// file: crates/uaa-control/src/machine_plane/dashboard.rs
// version: 1.1.1
// guid: e4da51dc-1d66-4fd2-8de7-e09458eb455e
// last-edited: 2026-07-13

//! Machine-plane status dashboard (`:25000` parity, spec Decision 12).
//!
//! Exact-shape port of `scripts/autoinstall-agent.py`'s `render_dashboard` +
//! `GET /dashboard` (Python `:115-152`, `:380-398`): a single-page,
//! display-only status HTML with four sections — agent-binary presence, the
//! machine registry table, placed-config inventory (metadata only, NEVER file
//! contents), and the last 20 events. No external assets, no `<script>`, no
//! forms; every interpolated value is HTML-escaped.
//!
//! # Registry data model (constellation port, not a literal Python mirror)
//!
//! Python read two flat JSON files (`REGISTRY_FILE`, `EVENTS_LOG`). This port
//! reads the CT-01 snapshot+WAL degraded-mode layer (`crate::db::store`)
//! instead: [`crate::db::MachineRow`]s from the same on-disk snapshot
//! `machine_plane::{seeds,lifecycle}` read/write, and events from the WAL
//! (`wal.jsonl`) — the only store `lifecycle::Registry::append_event` actually
//! writes. `machine_plane::inventory`'s `/api/events` reads the SAME WAL (see
//! its own `FileRegistry::list_events`) rather than Python's `EVENTS_LOG`,
//! which nothing in this codebase populates anymore — that used to leave
//! `/api/events` permanently empty while this dashboard showed live events;
//! both now read the same source.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use serde_json::Value;

use crate::db::{
    store::{read_snapshot, StatePaths, WalEntry},
    MachineRow,
};

/// Webroot base for placed cloud-init configs (mirrors Python's
/// `CLOUD_INIT_BASE`, `:32`; duplicated from `seeds.rs`, which keeps it
/// private — a 1-line const copy across disjoint machine-plane files is the
/// same accepted-cost REUSE pattern `inventory.rs` documents for `mac_to_hex`).
const CLOUD_INIT_BASE: &str = "/var/www/html/cloud-init";
/// Served `uaa` agent binary path (mirrors Python's `UAA_BINARY_PATH`, `:33`).
const UAA_BINARY_PATH: &str = "/var/www/html/uaa/uaa-amd64";
/// How many trailing events the dashboard renders (mirrors Python's
/// `render_dashboard` header, `:146`: `"Last %d events" % len(events)`).
const EVENT_WINDOW: usize = 20;

// ── Registry seam (read-only; mockable) ──────────────────────────────────

/// Read-only persistence seam for the dashboard. A local trait rather than
/// reusing `machine_plane::{lifecycle,inventory}::Registry` — neither exposes
/// exactly `list_machines` + WAL-backed `list_events` together, and this
/// module's own de-collision cost (documented in `inventory.rs`'s module doc)
/// is one narrow trait, not a shared one two other files would need to grow.
#[async_trait::async_trait]
pub trait Registry: Send + Sync {
    /// Every machine row in the shared snapshot (Python `:132-136`'s
    /// `sorted(registry.items())` scan — sorting is the caller's job here).
    async fn list_machines(&self) -> Vec<MachineRow>;
    /// The last `limit` telemetry events, oldest-first within the window
    /// (Python `:383`'s `readlines()[-20:]`). See the module doc for why this
    /// reads the WAL, not the dead legacy events file.
    async fn list_events(&self, limit: usize) -> Vec<Value>;
}

/// Real [`Registry`]: machines via the shared CT-01 snapshot; events via the
/// same WAL `lifecycle::Registry::append_event` writes to.
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
    async fn list_machines(&self) -> Vec<MachineRow> {
        read_snapshot(&self.paths).machines
    }

    /// Reads `wal.jsonl` directly (the file `lifecycle::Registry::append_event`
    /// appends `WalEntry{kind:"event", payload, ..}` to), keeps only
    /// `kind == "event"` entries (excludes `"install_history"`, a Rust-only
    /// addition absent from Python's `events.jsonl`), and unwraps each to its
    /// `payload` — which already carries `received_at` (set by `append_event`
    /// before the WAL write), matching the flat shape Python's dashboard
    /// table expects. Unlike `inventory::list_events` (which zeroes the whole
    /// window on one corrupt line, mirroring Python's crash-prone
    /// `[json.loads(l) for l in lines]`), a corrupt WAL line here is skipped
    /// and reading continues — the WAL legitimately carries lines destined
    /// for quarantine (`db::store::wal_replay`), and a dashboard should stay
    /// useful despite one bad line rather than going blank.
    async fn list_events(&self, limit: usize) -> Vec<Value> {
        let content = match std::fs::read_to_string(&self.paths.wal) {
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
}

fn default_state() -> AppState {
    AppState {
        webroot: Arc::new(PathBuf::from(CLOUD_INIT_BASE)),
        binary_path: Arc::new(PathBuf::from(UAA_BINARY_PATH)),
        registry: Arc::new(FileRegistry::new(StatePaths::default())),
    }
}

#[derive(Clone)]
struct AppState {
    webroot: Arc<PathBuf>,
    binary_path: Arc<PathBuf>,
    registry: Arc<dyn Registry>,
}

// ── Pure parity functions ──────────────────────────────────────────────────

/// HTML-escape one interpolated value (Python `html.escape(str(v), quote=True)`,
/// `:118-119`): `&<>"'` only, matching Python's `quote=True` escape set exactly
/// (`&amp; &lt; &gt; &quot; &#x27;`).
fn esc(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    for c in v.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            other => out.push(other),
        }
    }
    out
}

/// `esc` over an `Option<&str>`, `None` -> `""` (Python `:119`'s
/// `"" if v is None else str(v)`).
fn esc_opt(v: Option<&str>) -> String {
    esc(v.unwrap_or(""))
}

/// A stat of the served `uaa` agent binary (Python `agent_binary_status`,
/// `:47-58`). A missing file/dir is the normal, handled case — never an error.
struct BinaryStatus {
    path: String,
    present: bool,
    size: Option<u64>,
    mtime: Option<String>,
}

fn agent_binary_status(path: &Path) -> BinaryStatus {
    let meta = std::fs::metadata(path);
    match meta {
        Ok(m) => BinaryStatus {
            path: path.display().to_string(),
            present: true,
            size: Some(m.len()),
            mtime: m.modified().ok().map(format_mtime),
        },
        Err(_) => BinaryStatus {
            path: path.display().to_string(),
            present: false,
            size: None,
            mtime: None,
        },
    }
}

/// UTC `%Y-%m-%dT%H:%M:%SZ` formatting (Python `datetime.utcfromtimestamp(...)
/// .strftime("%Y-%m-%dT%H:%M:%SZ")`, `:57`, `:230`).
fn format_mtime(t: SystemTime) -> String {
    let dt: chrono::DateTime<chrono::Utc> = t.into();
    dt.format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// A placed `<hexmac>/uaa.yaml` (Python `collect_uaa_configs`, `:204-233`).
/// METADATA ONLY — `placeholder_free` is derived from file bytes but the
/// bytes themselves are never stored on this struct or returned to a caller.
struct PlacedConfig {
    hexmac: String,
    hostname: Option<String>,
    mtime: Option<String>,
    placeholder_free: bool,
}

/// `true` iff `name` is exactly 12 lowercase hex digits (Python's
/// `re.fullmatch(r"[0-9a-f]{12}", name)`, `:219`) — the hexmac directory-name
/// convention; skips `README.md`, `reporting.sh`, `scripts/`, etc.
fn is_hexmac_dirname(name: &str) -> bool {
    name.len() == 12
        && name
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Inventory placed `<hexmac>/uaa.yaml` files under `base` (Python
/// `collect_uaa_configs`, `:204-233`). A missing root is an empty inventory,
/// not an error (`:216-217`); a hexmac dir with no placed `uaa.yaml` is
/// silently skipped (`:225-226`).
fn collect_uaa_configs(
    base: &Path,
    hex_to_hostname: &HashMap<String, String>,
) -> Vec<PlacedConfig> {
    let mut names: Vec<String> = match std::fs::read_dir(base) {
        Ok(entries) => entries
            .flatten()
            .filter_map(|e| e.file_name().into_string().ok())
            .collect(),
        Err(_) => return Vec::new(),
    };
    names.sort();

    let mut configs = Vec::new();
    for name in names {
        if !is_hexmac_dirname(&name) {
            continue;
        }
        let fpath = base.join(&name).join("uaa.yaml");
        let data = match std::fs::read(&fpath) {
            Ok(d) => d,
            Err(_) => continue,
        };
        let mtime = std::fs::metadata(&fpath)
            .ok()
            .and_then(|m| m.modified().ok())
            .map(format_mtime);
        let placeholder_free = !String::from_utf8_lossy(&data).contains("REPLACE_AT_PLACE_TIME");
        configs.push(PlacedConfig {
            hexmac: name.clone(),
            hostname: hex_to_hostname.get(&name).cloned(),
            mtime,
            placeholder_free,
        });
    }
    configs
}

/// Strip separators for the `<hexmac>` directory-name convention (mirrors
/// Python `mac_to_hex`, `:75-76`; duplicated per-file, see `inventory.rs`'s
/// REUSE note).
fn mac_to_hex(mac: &str) -> String {
    mac.to_lowercase().replace([':', '-', '.'], "")
}

/// Render the single-page, display-only status HTML (Python `render_dashboard`,
/// `:115-152`). No external assets, no `<script>`, no forms — every
/// interpolated value goes through [`esc`]/[`esc_opt`].
fn render_dashboard(
    machines: &[MachineRow],
    events: &[Value],
    configs: &[PlacedConfig],
    binary: &BinaryStatus,
) -> String {
    let mut out = String::new();
    out.push_str("<!DOCTYPE html><html><head><meta charset='utf-8'>");
    out.push_str("<title>autoinstall-agent status</title><style>");
    out.push_str(
        "body{font-family:sans-serif;margin:2em}table{border-collapse:collapse;margin-bottom:2em}",
    );
    out.push_str("th,td{border:1px solid #999;padding:4px 8px;text-align:left}th{background:#eee}");
    out.push_str("</style></head><body><h1>autoinstall-agent — install server status</h1>");

    // Agent binary.
    out.push_str("<h2>Agent binary</h2><table><tr><th>path</th><th>present</th><th>size</th><th>mtime</th></tr>");
    out.push_str(&format!(
        "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr></table>",
        esc(&binary.path),
        binary.present,
        esc_opt(binary.size.map(|s| s.to_string()).as_deref()),
        esc_opt(binary.mtime.as_deref()),
    ));

    // Registry — sorted by MAC (Python `:133`'s `sorted(registry.items())`,
    // sorted by the dict key).
    out.push_str(
        "<h2>Registry</h2><table><tr><th>hostname</th><th>mac</th><th>status</th><th>last_seen</th><th>last_ip</th></tr>",
    );
    let mut sorted_machines: Vec<&MachineRow> = machines.iter().collect();
    sorted_machines.sort_by(|a, b| a.mac.cmp(&b.mac));
    for m in sorted_machines {
        let status: String = m.status.clone().into();
        out.push_str(&format!(
            "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
            esc(&m.hostname),
            esc(&m.mac),
            esc(&status),
            esc_opt(m.last_seen.as_deref()),
            esc_opt(m.last_ip.as_deref()),
        ));
    }
    out.push_str("</table>");

    // Placed configs — METADATA ONLY, never contents, never links to contents.
    out.push_str("<h2>Placed configs</h2><table><tr><th>hexmac</th><th>hostname</th><th>mtime</th><th>ready</th></tr>");
    for c in configs {
        out.push_str(&format!(
            "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
            esc(&c.hexmac),
            esc_opt(c.hostname.as_deref()),
            esc_opt(c.mtime.as_deref()),
            if c.placeholder_free {
                "yes"
            } else {
                "PLACEHOLDER"
            },
        ));
    }
    out.push_str("</table>");

    // Last N events.
    out.push_str(&format!(
        "<h2>Last {} events</h2><table><tr><th>received_at</th><th>name</th><th>event_type</th><th>status</th><th>progress</th><th>message</th></tr>",
        events.len()
    ));
    for ev in events {
        let field = |k: &str| -> String {
            match ev.get(k) {
                None | Some(Value::Null) => String::new(),
                Some(Value::String(s)) => s.clone(),
                Some(other) => other.to_string(),
            }
        };
        out.push_str(&format!(
            "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
            esc(&field("received_at")),
            esc(&field("name")),
            esc(&field("event_type")),
            esc(&field("status")),
            esc(&field("progress")),
            esc(&field("message")),
        ));
    }
    out.push_str("</table></body></html>");
    out
}

// ── Router / handler wiring ──────────────────────────────────────────────

async fn handle_dashboard(State(state): State<AppState>) -> Response {
    let machines = state.registry.list_machines().await;
    let events = state.registry.list_events(EVENT_WINDOW).await;

    let mut hex_to_hostname = HashMap::new();
    for m in &machines {
        hex_to_hostname.insert(mac_to_hex(&m.mac), m.hostname.clone());
    }
    let configs = collect_uaa_configs(&state.webroot, &hex_to_hostname);
    let binary = agent_binary_status(&state.binary_path);

    let body = render_dashboard(&machines, &events, &configs, &binary);
    (
        StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")],
        body,
    )
        .into_response()
}

fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/dashboard", get(handle_dashboard))
        .with_state(state)
}

/// The dashboard sub-router. Merged into `machine_plane::router()` by
/// `mod.rs` with `.merge(dashboard::router())` (one line, owned by CT-01's
/// `mod.rs` — never edited here).
pub fn router() -> Router {
    build_router(default_state())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn machine(mac: &str, hostname: &str, status: crate::db::MachineStatus) -> MachineRow {
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
            last_seen: Some("1234".to_string()),
            last_ip: Some("10.0.0.9".to_string()),
            installed_at: None,
            last_install_status: None,
            updated_at: None,
        }
    }

    struct MockRegistry {
        machines: Vec<MachineRow>,
        events: Vec<Value>,
    }

    #[async_trait::async_trait]
    impl Registry for MockRegistry {
        async fn list_machines(&self) -> Vec<MachineRow> {
            self.machines.clone()
        }
        async fn list_events(&self, limit: usize) -> Vec<Value> {
            let start = self.events.len().saturating_sub(limit);
            self.events[start..].to_vec()
        }
    }

    fn test_state(webroot: PathBuf, binary_path: PathBuf, registry: MockRegistry) -> AppState {
        AppState {
            webroot: Arc::new(webroot),
            binary_path: Arc::new(binary_path),
            registry: Arc::new(registry),
        }
    }

    async fn body_string(resp: Response) -> String {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[test]
    fn test_router_builds_standalone() {
        // Constructing the router touches no filesystem/network — only requests do.
        let _ = router();
    }

    #[test]
    fn test_esc_matches_python_quote_set() {
        assert_eq!(esc("a&b<c>d\"e'f"), "a&amp;b&lt;c&gt;d&quot;e&#x27;f");
        assert_eq!(esc_opt(None), "");
        assert_eq!(esc_opt(Some("x")), "x");
    }

    #[test]
    fn test_is_hexmac_dirname() {
        assert!(is_hexmac_dirname("6c4b90bc39b3"));
        assert!(!is_hexmac_dirname("README.md"));
        assert!(!is_hexmac_dirname("scripts"));
        assert!(
            !is_hexmac_dirname("6C4B90BC39B3"),
            "uppercase must not match"
        );
        assert!(
            !is_hexmac_dirname("6c4b90bc39b"),
            "wrong length must not match"
        );
    }

    #[test]
    fn test_agent_binary_status_missing_is_handled() {
        let status = agent_binary_status(Path::new("/nonexistent/path/uaa-amd64"));
        assert!(!status.present);
        assert!(status.size.is_none());
        assert!(status.mtime.is_none());
    }

    #[test]
    fn test_agent_binary_status_present() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("uaa-amd64");
        std::fs::write(&path, b"fake-binary-bytes").unwrap();
        let status = agent_binary_status(&path);
        assert!(status.present);
        assert_eq!(status.size, Some(17));
        assert!(status.mtime.is_some());
    }

    #[test]
    fn test_collect_uaa_configs_metadata_only() {
        let dir = tempfile::tempdir().unwrap();
        let hex_dir = dir.path().join("6c4b90bc39b3");
        std::fs::create_dir_all(&hex_dir).unwrap();
        std::fs::write(
            hex_dir.join("uaa.yaml"),
            b"disk_device: /dev/nvme0n1\nreal: secret\n",
        )
        .unwrap();
        // Non-hexmac entries must be skipped (README.md, scripts/).
        std::fs::write(dir.path().join("README.md"), b"ignored").unwrap();
        std::fs::create_dir_all(dir.path().join("scripts")).unwrap();
        // A hexmac dir with no placed uaa.yaml is silently skipped.
        std::fs::create_dir_all(dir.path().join("aaaaaaaaaaaa")).unwrap();

        let mut hex_to_hostname = HashMap::new();
        hex_to_hostname.insert("6c4b90bc39b3".to_string(), "len-serv-001".to_string());

        let configs = collect_uaa_configs(dir.path(), &hex_to_hostname);
        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].hexmac, "6c4b90bc39b3");
        assert_eq!(configs[0].hostname.as_deref(), Some("len-serv-001"));
        assert!(
            configs[0].placeholder_free,
            "no REPLACE_AT_PLACE_TIME marker present"
        );
    }

    #[test]
    fn test_collect_uaa_configs_flags_placeholder() {
        let dir = tempfile::tempdir().unwrap();
        let hex_dir = dir.path().join("aabbccddeeff");
        std::fs::create_dir_all(&hex_dir).unwrap();
        std::fs::write(
            hex_dir.join("uaa.yaml"),
            b"disk_device: REPLACE_AT_PLACE_TIME\n",
        )
        .unwrap();

        let configs = collect_uaa_configs(dir.path(), &HashMap::new());
        assert_eq!(configs.len(), 1);
        assert!(!configs[0].placeholder_free);
        assert!(
            configs[0].hostname.is_none(),
            "unmapped hexmac has no known hostname"
        );
    }

    #[test]
    fn test_collect_uaa_configs_missing_root_is_empty_not_error() {
        let configs = collect_uaa_configs(Path::new("/nonexistent/webroot"), &HashMap::new());
        assert!(configs.is_empty());
    }

    #[test]
    fn test_render_dashboard_escapes_hostile_input() {
        let machines = vec![machine(
            "aa:bb:cc:dd:ee:ff",
            "<script>alert(1)</script>",
            crate::db::MachineStatus::Seen,
        )];
        let binary = BinaryStatus {
            path: "/var/www/html/uaa/uaa-amd64".to_string(),
            present: false,
            size: None,
            mtime: None,
        };
        let html = render_dashboard(&machines, &[], &[], &binary);
        assert!(
            !html.contains("<script>alert(1)</script>"),
            "hostname must be escaped, not raw"
        );
        assert!(html.contains("&lt;script&gt;"));
        assert!(
            html.contains("seen"),
            "status renders via the MachineStatus::into::<String> conversion"
        );
    }

    #[tokio::test]
    async fn test_handle_dashboard_renders_all_sections() {
        let dir = tempfile::tempdir().unwrap();
        let hex_dir = dir.path().join("aabbccddeeff");
        std::fs::create_dir_all(&hex_dir).unwrap();
        std::fs::write(hex_dir.join("uaa.yaml"), b"disk_device: /dev/nvme0n1\n").unwrap();

        let registry = MockRegistry {
            machines: vec![machine(
                "aa:bb:cc:dd:ee:ff",
                "len-serv-009",
                crate::db::MachineStatus::Approved,
            )],
            events: vec![serde_json::json!({
                "received_at": 1_700_000_000,
                "name": "len-serv-009",
                "event_type": "status_update",
                "status": "success",
                "progress": 100,
                "message": "install complete",
            })],
        };
        let state = test_state(
            dir.path().to_path_buf(),
            dir.path().join("uaa-amd64"),
            registry,
        );

        let resp = handle_dashboard(State(state)).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers()
                .get(axum::http::header::CONTENT_TYPE)
                .unwrap()
                .to_str()
                .unwrap(),
            "text/html; charset=utf-8"
        );
        let body = body_string(resp).await;
        assert!(body.contains("len-serv-009"));
        assert!(body.contains("aa:bb:cc:dd:ee:ff"));
        assert!(
            body.contains("aabbccddeeff"),
            "placed config hexmac rendered"
        );
        assert!(body.contains("install complete"), "event rendered");
        assert!(body.contains("Last 1 events"));
    }

    #[tokio::test]
    async fn test_file_registry_list_events_reads_wal_kind_event_only() {
        let dir = tempfile::tempdir().unwrap();
        let paths = StatePaths::under(dir.path());

        crate::db::store::wal_append(
            &paths,
            "event",
            serde_json::json!({"name": "h1", "received_at": 1}),
        )
        .unwrap();
        crate::db::store::wal_append(&paths, "install_history", serde_json::json!({"mac": "aa"}))
            .unwrap();
        crate::db::store::wal_append(
            &paths,
            "event",
            serde_json::json!({"name": "h2", "received_at": 2}),
        )
        .unwrap();

        let registry = FileRegistry::new(paths);
        let events = registry.list_events(20).await;
        assert_eq!(events.len(), 2, "install_history entries must be excluded");
        assert_eq!(events[0]["name"], "h1");
        assert_eq!(events[1]["name"], "h2");
    }

    #[tokio::test]
    async fn test_file_registry_list_events_skips_corrupt_line_not_whole_window() {
        let dir = tempfile::tempdir().unwrap();
        let paths = StatePaths::under(dir.path());
        crate::db::store::wal_append(&paths, "event", serde_json::json!({"name": "good-1"}))
            .unwrap();
        // Append a corrupt line directly.
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&paths.wal)
            .unwrap();
        writeln!(f, "{{ not valid json").unwrap();
        crate::db::store::wal_append(&paths, "event", serde_json::json!({"name": "good-2"}))
            .unwrap();

        let registry = FileRegistry::new(paths);
        let events = registry.list_events(20).await;
        assert_eq!(
            events.len(),
            2,
            "corrupt line is skipped, not fatal to the whole window"
        );
        assert_eq!(events[0]["name"], "good-1");
        assert_eq!(events[1]["name"], "good-2");
    }

    #[tokio::test]
    async fn test_file_registry_list_events_limit_keeps_newest() {
        let dir = tempfile::tempdir().unwrap();
        let paths = StatePaths::under(dir.path());
        for i in 0..5 {
            crate::db::store::wal_append(&paths, "event", serde_json::json!({"n": i})).unwrap();
        }
        let registry = FileRegistry::new(paths);
        let events = registry.list_events(2).await;
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["n"], 3);
        assert_eq!(events[1]["n"], 4);
    }

    #[tokio::test]
    async fn test_file_registry_list_machines_reads_shared_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let paths = StatePaths::under(dir.path());
        let mut doc = read_snapshot(&paths);
        doc.machines.push(machine(
            "11:22:33:44:55:66",
            "h1",
            crate::db::MachineStatus::Pending,
        ));
        crate::db::store::write_snapshot(&paths, &doc).unwrap();

        let registry = FileRegistry::new(paths);
        let machines = registry.list_machines().await;
        assert_eq!(machines.len(), 1);
        assert_eq!(machines[0].mac, "11:22:33:44:55:66");
    }
}
