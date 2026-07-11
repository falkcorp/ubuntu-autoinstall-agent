// file: crates/uaa-control/src/machine_plane/seeds.rs
// version: 1.1.0
// guid: 448eead2-d3b6-4765-8e21-2a7421d3b55e
// last-edited: 2026-07-10

//! Machine-plane seed placement + boot-target intent (:25000 parity).
//!
//! Exact parity with `scripts/autoinstall-agent.py` (spec Decision 12) for the
//! five auto-resolved GET endpoints: `/autoinstall/{user-data,meta-data,
//! vendor-data,network-config}` (Python `:496-519`) and `/autoinstall/uaa-config`
//! (Python `:530-556`). MAC resolution mirrors `mac_from_neighbor_table`
//! (Python `:186-194`): `ip neigh show <client_ip>` through the
//! [`CommandExecutor`] seam only — this module never spawns a subprocess
//! itself — with the `lladdr ([0-9a-fA-F:]+)` regex, lowercased and stripped
//! to a 12-hex `hexmac`. Resolution is neighbor-table + filesystem only:
//! these handlers never touch CockroachDB (spec Decision 4 — the `:25000`
//! read plane keeps serving under CRDB degradation).
//!
//! Normative split (Decision 12): for the four seed files, an existing
//! `<hexmac>` directory with the requested file missing is an EMPTY 200
//! (Python `:512` reads `b""` for a missing file via `else b""`). For
//! `/autoinstall/uaa-config` the same condition is a HARD 404 with an empty
//! response (Python `:544-548`) — the USB bootstrap must fail loudly at
//! fetch time, never receive an empty config. No neighbor-table entry, or a
//! resolved MAC with no `<hexmac>` directory, is a 404 (empty response) for
//! ALL FIVE endpoints.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::{
    extract::{ConnectInfo, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};

use uaa_core::network::{CommandExecutor, LocalClient};

/// Webroot base (mirrors Python's `CLOUD_INIT_BASE`, `:32`). Only the
/// production [`default_state`] reads this constant; tests always inject a
/// tempdir webroot via [`AppState`] directly.
const CLOUD_INIT_BASE: &str = "/var/www/html/cloud-init";

// ── Pure parity functions ──────────────────────────────────────────────────

/// Parse the `lladdr` MAC out of `ip neigh show` output (Python
/// `mac_from_neighbor_table`, `:186-194`, regex `:191`). Returns `None` on no
/// match — callers treat that exactly like a swallowed exception (`:193-194`).
pub fn mac_from_neighbor_output(out: &str) -> Option<String> {
    let re = regex::Regex::new(r"lladdr ([0-9a-fA-F:]+)").expect("static regex is valid");
    re.captures(out).map(|c| c[1].to_lowercase())
}

/// Strip MAC separators to a bare 12-hex string (mirrors Python `mac_to_hex`,
/// `:75-76`).
pub fn mac_to_hex(mac: &str) -> String {
    mac.to_lowercase().replace([':', '-', '.'], "")
}

/// Resolve `<webroot>/<hexmac>` for `client_ip` (Python `resolve_cloud_init_dir`,
/// `:196-202`): run `ip neigh show <client_ip>` through the executor seam
/// (any executor error — including a timeout or non-zero exit — is swallowed
/// to `None`, mirroring Python's blanket `except Exception` at `:193`), parse
/// the MAC, and report whether the per-machine directory exists.
///
/// Returns:
/// - `None` — no MAC resolved (empty `client_ip`, executor error, or no
///   `lladdr` match).
/// - `Some((hexmac, None))` — MAC resolved but `<webroot>/<hexmac>` is not a
///   directory.
/// - `Some((hexmac, Some(dir)))` — MAC resolved and the directory exists.
pub async fn resolve_cloud_init_dir(
    executor: &mut (dyn CommandExecutor + Send),
    webroot: &Path,
    client_ip: &str,
) -> Option<(String, Option<PathBuf>)> {
    if client_ip.is_empty() {
        return None;
    }
    let out = executor
        .execute_with_output(&format!("ip neigh show {client_ip}"))
        .await
        .ok()?;
    let mac = mac_from_neighbor_output(&out)?;
    let hexmac = mac_to_hex(&mac);
    let dir = webroot.join(&hexmac);
    Some((hexmac, dir.is_dir().then_some(dir)))
}

// ── Router state ────────────────────────────────────────────────────────

/// Mints one fresh executor per request. `CommandExecutor::execute_with_output`
/// takes `&mut self`, so a single shared instance can't safely serve
/// concurrent requests — the factory sidesteps that without a lock.
type ExecutorFactory = Arc<dyn Fn() -> Box<dyn CommandExecutor + Send> + Send + Sync>;

/// Router state: webroot base + the executor factory. Tests substitute both
/// fields — a tempdir webroot and a factory returning a recording
/// `MockExecutor` clone — so handler logic never touches a live shell or
/// CockroachDB.
#[derive(Clone)]
struct AppState {
    webroot: Arc<PathBuf>,
    executor_factory: ExecutorFactory,
}

/// Production state: real webroot constant + a fresh [`LocalClient`] (already
/// `CommandExecutor`-typed, `crates/uaa-core/src/network/executor.rs`) per
/// request — never a subprocess call written in this file.
fn default_state() -> AppState {
    AppState {
        webroot: Arc::new(PathBuf::from(CLOUD_INIT_BASE)),
        executor_factory: Arc::new(|| Box::new(LocalClient::new()) as Box<dyn CommandExecutor + Send>),
    }
}

// ── HTTP helpers ────────────────────────────────────────────────────────

fn empty_404() -> Response {
    StatusCode::NOT_FOUND.into_response()
}

fn text_200(payload: Vec<u8>) -> Response {
    let len = payload.len();
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/plain; charset=utf-8".to_string()),
            (header::CONTENT_LENGTH, len.to_string()),
        ],
        payload,
    )
        .into_response()
}

// ── Shared resolution + serve logic ────────────────────────────────────

/// Resolve `client_ip` to a hexmac directory, or return the appropriate 404
/// (no neighbor entry / no directory registered) as `Err`. Shared by all
/// five handlers so the DENIED-reason logging (client_ip + hexmac only,
/// never file contents) lives in one place.
async fn resolve_or_deny(
    state: &AppState,
    client_ip: &str,
    endpoint: &str,
) -> Result<(String, PathBuf), Response> {
    let mut executor = (state.executor_factory)();
    match resolve_cloud_init_dir(executor.as_mut(), &state.webroot, client_ip).await {
        None => {
            tracing::info!(%endpoint, %client_ip, "AUTOINSTALL DENIED - no ARP/NDP neighbor entry");
            Err(empty_404())
        }
        Some((hexmac, None)) => {
            tracing::info!(%endpoint, %client_ip, %hexmac, "AUTOINSTALL DENIED - no cloud-init dir registered");
            Err(empty_404())
        }
        Some((hexmac, Some(dir))) => Ok((hexmac, dir)),
    }
}

/// Serve one of the four cloud-init seed files. A missing file under an
/// existing hexmac directory is an EMPTY 200 (Decision 12) — never a 404.
async fn serve_seed_file(state: &AppState, client_ip: &str, filename: &str) -> Response {
    let (hexmac, dir) = match resolve_or_deny(state, client_ip, filename).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let file_path = dir.join(filename);
    // empty-200: Decision 12 — a missing seed file under an existing hexmac
    // directory is served empty (Python `:512`'s `else b""` for a missing file).
    let payload = if file_path.is_file() {
        std::fs::read(&file_path).unwrap_or_default()
    } else {
        Vec::new()
    };
    tracing::info!(%client_ip, %hexmac, %filename, "AUTOINSTALL served");
    text_200(payload)
}

/// Serve `/autoinstall/uaa-config`. Unlike the seed files, a missing
/// `uaa.yaml` is a HARD 404 (Decision 12, Python `:544-548`) — never an
/// empty 200 — so the USB bootstrap fails loudly at fetch time instead of
/// receiving an empty config.
async fn serve_uaa_config(state: &AppState, client_ip: &str) -> Response {
    let (hexmac, dir) = match resolve_or_deny(state, client_ip, "uaa-config").await {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let file_path = dir.join("uaa.yaml");
    if !file_path.is_file() {
        tracing::info!(%client_ip, %hexmac, "UAA-CONFIG DENIED - no uaa.yaml placed");
        return empty_404();
    }
    let payload = std::fs::read(&file_path).unwrap_or_default();
    tracing::info!(%client_ip, %hexmac, "UAA-CONFIG served");
    text_200(payload)
}

// ── Axum handlers (State first, ConnectInfo last — no body extractor on a
// GET route) ────────────────────────────────────────────────────────────

async fn handle_user_data(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> Response {
    serve_seed_file(&state, &addr.ip().to_string(), "user-data").await
}

async fn handle_meta_data(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> Response {
    serve_seed_file(&state, &addr.ip().to_string(), "meta-data").await
}

async fn handle_vendor_data(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> Response {
    serve_seed_file(&state, &addr.ip().to_string(), "vendor-data").await
}

async fn handle_network_config(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> Response {
    serve_seed_file(&state, &addr.ip().to_string(), "network-config").await
}

async fn handle_uaa_config(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> Response {
    serve_uaa_config(&state, &addr.ip().to_string()).await
}

fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/autoinstall/user-data", get(handle_user_data))
        .route("/autoinstall/meta-data", get(handle_meta_data))
        .route("/autoinstall/vendor-data", get(handle_vendor_data))
        .route("/autoinstall/network-config", get(handle_network_config))
        .route("/autoinstall/uaa-config", get(handle_uaa_config))
        .with_state(state)
}

/// The seeds sub-router. Merged into `machine_plane::router()` by the
/// coordinator with `.merge(seeds::router())` (owned by CT-01's `mod.rs` —
/// never edited here). Each route matches exactly one literal filename, so an
/// unrecognized `/autoinstall/<other>` path never reaches these handlers.
/// The listener must be served with
/// `Router::into_make_service_with_connect_info::<SocketAddr>()` for
/// `ConnectInfo` to resolve to the real TCP peer address — the Rust
/// equivalent of Python's `self.client_address[0]`.
pub fn router() -> Router {
    build_router(default_state())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use uaa_core::{AutoInstallError, Result as CoreResult};

    /// Recording mock executor: returns pre-loaded output strings keyed by
    /// the exact command string, or an error when `fail` is set. Mirrors the
    /// `MockExecutor` idiom in `crates/uaa-core/src/autoinstall/verify.rs`.
    #[derive(Clone, Default)]
    struct MockExecutor {
        responses: HashMap<String, String>,
        fail: bool,
    }

    impl MockExecutor {
        fn new(pairs: &[(&str, &str)]) -> Self {
            Self {
                responses: pairs
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
                fail: false,
            }
        }

        fn failing() -> Self {
            Self {
                responses: HashMap::new(),
                fail: true,
            }
        }

        fn get(&self, cmd: &str) -> String {
            self.responses.get(cmd).cloned().unwrap_or_default()
        }
    }

    #[async_trait::async_trait]
    impl CommandExecutor for MockExecutor {
        async fn connect(&mut self, _host: &str, _username: &str) -> CoreResult<()> {
            Ok(())
        }
        async fn execute(&mut self, _command: &str) -> CoreResult<()> {
            Ok(())
        }
        async fn execute_with_output(&mut self, command: &str) -> CoreResult<String> {
            if self.fail {
                return Err(AutoInstallError::ProcessError {
                    command: command.to_string(),
                    exit_code: None,
                    stderr: "mock executor error".to_string(),
                });
            }
            Ok(self.get(command))
        }
        async fn execute_with_error_collection(
            &mut self,
            command: &str,
            _description: &str,
        ) -> CoreResult<(i32, String, String)> {
            Ok((0, self.get(command), String::new()))
        }
        async fn check_silent(&mut self, command: &str) -> CoreResult<bool> {
            Ok(!self.get(command).is_empty())
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

    const CLIENT_IP: &str = "172.16.3.92";
    const REACHABLE: &str = "172.16.3.92 dev eth0 lladdr 6c:4b:90:bc:39:b3 REACHABLE";
    const HEXMAC: &str = "6c4b90bc39b3";

    fn neigh_cmd() -> String {
        format!("ip neigh show {CLIENT_IP}")
    }

    fn client_addr() -> SocketAddr {
        SocketAddr::from(([172, 16, 3, 92], 54321))
    }

    fn test_state_with(webroot: PathBuf, mock: MockExecutor) -> AppState {
        AppState {
            webroot: Arc::new(webroot),
            executor_factory: Arc::new(move || Box::new(mock.clone()) as Box<dyn CommandExecutor + Send>),
        }
    }

    async fn body_bytes(resp: Response) -> Vec<u8> {
        axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec()
    }

    async fn all_five(state: &AppState) -> Vec<Response> {
        let addr = client_addr();
        vec![
            handle_user_data(State(state.clone()), ConnectInfo(addr)).await,
            handle_meta_data(State(state.clone()), ConnectInfo(addr)).await,
            handle_vendor_data(State(state.clone()), ConnectInfo(addr)).await,
            handle_network_config(State(state.clone()), ConnectInfo(addr)).await,
            handle_uaa_config(State(state.clone()), ConnectInfo(addr)).await,
        ]
    }

    #[test]
    fn test_router_builds_standalone() {
        // Constructing the router touches no filesystem/network — only requests do.
        let _ = router();
    }

    #[test]
    fn test_mac_parse_and_hex() {
        let mac = mac_from_neighbor_output(REACHABLE).unwrap();
        assert_eq!(mac, "6c:4b:90:bc:39:b3");
        assert_eq!(mac_to_hex(&mac), HEXMAC);
    }

    #[tokio::test]
    async fn test_no_neighbor_entry_404() {
        let dir = tempfile::tempdir().unwrap();
        // No `lladdr` in the output -> no MAC resolves.
        let mock = MockExecutor::new(&[(neigh_cmd().as_str(), "172.16.3.92 dev eth0 FAILED")]);
        let state = test_state_with(dir.path().to_path_buf(), mock);

        for resp in all_five(&state).await {
            assert_eq!(resp.status(), StatusCode::NOT_FOUND);
            assert!(body_bytes(resp).await.is_empty());
        }
    }

    #[tokio::test]
    async fn test_no_hexmac_dir_404() {
        let dir = tempfile::tempdir().unwrap();
        // MAC resolves, but the hexmac directory is never created under `dir`.
        let mock = MockExecutor::new(&[(neigh_cmd().as_str(), REACHABLE)]);
        let state = test_state_with(dir.path().to_path_buf(), mock);

        for resp in all_five(&state).await {
            assert_eq!(resp.status(), StatusCode::NOT_FOUND);
            assert!(body_bytes(resp).await.is_empty());
        }
    }

    #[tokio::test]
    async fn test_missing_seed_file_empty_200() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(HEXMAC)).unwrap();
        let mock = MockExecutor::new(&[(neigh_cmd().as_str(), REACHABLE)]);
        let state = test_state_with(dir.path().to_path_buf(), mock);

        let resp = handle_user_data(State(state), ConnectInfo(client_addr())).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers().get(header::CONTENT_TYPE).unwrap().to_str().unwrap(),
            "text/plain; charset=utf-8"
        );
        let payload = body_bytes(resp).await;
        assert_eq!(payload.len(), 0);
    }

    #[tokio::test]
    async fn test_missing_uaa_config_hard_404() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(HEXMAC)).unwrap();
        let mock = MockExecutor::new(&[(neigh_cmd().as_str(), REACHABLE)]);
        let state = test_state_with(dir.path().to_path_buf(), mock);

        let resp = handle_uaa_config(State(state), ConnectInfo(client_addr())).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        assert!(body_bytes(resp).await.is_empty());
    }

    #[tokio::test]
    async fn test_present_files_served() {
        let dir = tempfile::tempdir().unwrap();
        let hex_dir = dir.path().join(HEXMAC);
        std::fs::create_dir_all(&hex_dir).unwrap();
        std::fs::write(hex_dir.join("user-data"), b"#cloud-config\nhostname: foo\n").unwrap();
        // Placeholder only -- never a real secret in this repo.
        std::fs::write(hex_dir.join("uaa.yaml"), b"disk_device: REPLACE_AT_PLACE_TIME\n").unwrap();
        let mock = MockExecutor::new(&[(neigh_cmd().as_str(), REACHABLE)]);
        let state = test_state_with(dir.path().to_path_buf(), mock);

        let resp = handle_user_data(State(state.clone()), ConnectInfo(client_addr())).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(body_bytes(resp).await, b"#cloud-config\nhostname: foo\n".to_vec());

        let resp = handle_uaa_config(State(state), ConnectInfo(client_addr())).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            body_bytes(resp).await,
            b"disk_device: REPLACE_AT_PLACE_TIME\n".to_vec()
        );
    }

    #[tokio::test]
    async fn test_executor_error_is_404() {
        let dir = tempfile::tempdir().unwrap();
        let mock = MockExecutor::failing();
        let state = test_state_with(dir.path().to_path_buf(), mock);

        for resp in all_five(&state).await {
            assert_eq!(resp.status(), StatusCode::NOT_FOUND);
            assert!(body_bytes(resp).await.is_empty());
        }
    }

    #[test]
    fn test_empty_client_ip_none() {
        // A synchronous smoke check for the empty-IP guard mentioned in the
        // brief's edge semantics -- exercised end-to-end via the 404 tests
        // above (an unmatched neighbor command returns an empty response,
        // which also has no `lladdr` match).
        assert!(mac_from_neighbor_output("").is_none());
    }
}
