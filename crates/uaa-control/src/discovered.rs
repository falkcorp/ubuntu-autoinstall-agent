// file: crates/uaa-control/src/discovered.rs
// version: 1.0.0
// guid: 2f9c7b41-6d8e-4a3f-9b5c-1e7d0a4b8c62
// last-edited: 2026-07-19

//! Discovery inbox — every device that ARPs/DHCPs on the segment, surfaced for
//! operator triage (`GET /api/discovered` → SPA Discovery page).
//!
//! **Why this exists:** the server sits on 172.16.2.0/23, so its kernel
//! neighbor (ARP/NDP) table holds an entry for every device that communicates
//! on the segment. The host-side scanner `scripts/arp-discovery-scan.sh` polls
//! `ip neigh` and POSTs each MAC here. (dnsmasq runs in proxy-DHCP mode and,
//! verified on the live box, does NOT log client MACs for non-PXE clients, so
//! its journal is not a usable source — the neighbor table is.) This is the
//! "track everything that ARPs/DHCPs" capture path — distinct from the
//! *reactive* [`crate::machine_plane::seeds::record_seen_mac`], which only
//! fires when a device fetches an autoinstall seed over HTTP.
//!
//! **Why a separate file, not the machine snapshot:** the follower is bursty
//! (every DHCP packet on the LAN). Writing those into
//! `registry-snapshot.json` would add a second frequent writer racing the
//! autoinstall handlers on the fleet registry. Instead this owns
//! `discovered-macs.json` outright; the only writers are the ingest POST and
//! the (rare) operator dismiss, both serialized by [`FILE_LOCK`] within the
//! single `uaa-control` process. A discovered MAC is NOT a fleet machine — an
//! operator promotes one by approving/registering it, or hides it via dismiss.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::{extract::State, http::StatusCode, response::IntoResponse, response::Response, routing::post, Json, Router};
use serde::Deserialize;
use std::sync::Arc;

use crate::operator::api_types::DiscoveredMacRow;

/// Production location of the discovery inbox. Mirrors
/// [`crate::db::store::StatePaths`]'s `/var/lib/uaa` base (same `StateDirectory`).
pub const DEFAULT_DISCOVERED_PATH: &str = "/var/lib/uaa/discovered-macs.json";

/// Serializes every read-modify-write of the discovery file. A single process
/// mutex is sufficient and correct: `uaa-control` is one process, and both the
/// machine-plane ingest (`:25000`) and the operator list/dismiss (`:15000`)
/// run inside it (see `listeners::serve`'s `try_join!`). Two independently
/// constructed [`DiscoveredStore`]s pointed at the same path still share this
/// lock, so their writes cannot interleave and lose rows.
static FILE_LOCK: Mutex<()> = Mutex::new(());

/// File-backed discovery inbox. Cheap to construct (just a path); all state
/// lives on disk so the two planes stay consistent without shared memory.
#[derive(Clone)]
pub struct DiscoveredStore {
    path: PathBuf,
}

impl Default for DiscoveredStore {
    fn default() -> Self {
        Self::new(PathBuf::from(DEFAULT_DISCOVERED_PATH))
    }
}

impl DiscoveredStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// All rows, newest-last-seen first. A missing file is an empty inbox, not
    /// an error (fail-open is correct here: discovery is advisory triage, never
    /// a security boundary). A corrupt file is logged and treated as empty
    /// rather than propagated — a follower's next POST rewrites it cleanly.
    pub fn list(&self) -> Vec<DiscoveredMacRow> {
        let _guard = FILE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let mut rows = read_rows(&self.path);
        rows.sort_by(|a, b| b.last_seen.cmp(&a.last_seen).then(a.mac.cmp(&b.mac)));
        rows
    }

    /// Upsert `mac`: a first sighting gets `first_seen == last_seen == now` and
    /// `dismissed = false`; a returning MAC only advances `last_seen`, so an
    /// operator's earlier dismiss and the original first-seen both survive
    /// re-sightings. Returns `false` (recording nothing) for a malformed MAC —
    /// the follower parses untrusted journal text, so garbage must not create a
    /// row. Idempotent-ish: re-POSTing the same MAC never duplicates it.
    pub fn record(&self, mac: &str) -> bool {
        let Some(mac) = canonical_mac(mac) else {
            return false;
        };
        let now = now_epoch_string();
        let _guard = FILE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let mut rows = read_rows(&self.path);
        match rows.iter_mut().find(|r| r.mac == mac) {
            Some(existing) => existing.last_seen = now,
            None => rows.push(DiscoveredMacRow {
                mac,
                first_seen: now.clone(),
                last_seen: now,
                dismissed: false,
            }),
        }
        write_rows(&self.path, &rows);
        true
    }

    /// Mark `mac` dismissed (hidden from the default triage view). Returns
    /// whether a matching row existed. Never deletes the row — a dismissed MAC
    /// that keeps DHCPing still refreshes `last_seen`, so a device an operator
    /// waved off but that is now behaving unexpectedly is not silently forgotten.
    pub fn dismiss(&self, mac: &str) -> bool {
        let Some(mac) = canonical_mac(mac) else {
            return false;
        };
        let _guard = FILE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let mut rows = read_rows(&self.path);
        let Some(row) = rows.iter_mut().find(|r| r.mac == mac) else {
            return false;
        };
        row.dismissed = true;
        write_rows(&self.path, &rows);
        true
    }
}

/// Read rows from `path`; `[]` for a missing OR unparseable file (see [`DiscoveredStore::list`]).
fn read_rows(path: &Path) -> Vec<DiscoveredMacRow> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|err| {
            tracing::warn!(path = %path.display(), %err, "discovered-macs.json unparseable — treating as empty");
            Vec::new()
        }),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(err) => {
            tracing::warn!(path = %path.display(), %err, "discovered-macs.json unreadable — treating as empty");
            Vec::new()
        }
    }
}

/// Atomically replace `path` with `rows` (tmp-in-same-dir + rename, mirroring
/// the registry snapshot's durable-write discipline). A write failure is logged,
/// not propagated: losing one discovery-inbox update must never fail a DHCP
/// ingest or an operator dismiss.
fn write_rows(path: &Path, rows: &[DiscoveredMacRow]) {
    let json = match serde_json::to_vec_pretty(rows) {
        Ok(j) => j,
        Err(err) => {
            tracing::error!(%err, "serializing discovered rows failed — inbox update dropped");
            return;
        }
    };
    let tmp = path.with_extension("json.tmp");
    if let Err(err) = std::fs::write(&tmp, &json) {
        tracing::error!(path = %tmp.display(), %err, "writing discovered tmp failed — inbox update dropped");
        return;
    }
    if let Err(err) = std::fs::rename(&tmp, path) {
        tracing::error!(path = %path.display(), %err, "renaming discovered tmp failed — inbox update dropped");
        let _ = std::fs::remove_file(&tmp);
    }
}

/// Canonicalize a MAC to 12 lowercase hex chars joined by `:`, or `None` if the
/// input is not six hex octets. Accepts `:`/`-`/`.`-free hex too. This is the
/// gate that keeps malformed journal text out of the inbox.
fn canonical_mac(raw: &str) -> Option<String> {
    let hex: String = raw
        .chars()
        .filter(|c| !matches!(c, ':' | '-' | '.'))
        .collect::<String>()
        .to_ascii_lowercase();
    if hex.len() != 12 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let pairs: Vec<String> = (0..12).step_by(2).map(|i| hex[i..i + 2].to_string()).collect();
    Some(pairs.join(":"))
}

fn now_epoch_string() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_string())
}

// ── Machine-plane ingest (:25000) ───────────────────────────────────────────

/// Body of `POST /api/discovered`. The scanner sends the MAC it read from the
/// neighbor table; `ip` is accepted for forward-compatibility but not persisted
/// (`DiscoveredMacRow` has no ip field yet — a MAC can hold several over time).
#[derive(Debug, Deserialize)]
struct IngestBody {
    mac: String,
    #[serde(default)]
    #[allow(dead_code)]
    ip: Option<String>,
}

/// The ingest sub-router merged into `machine_plane::router()`. Unauthenticated,
/// exactly like the rest of the `:25000` machine plane — the scanner POSTs from
/// localhost. State is the store so tests inject a tempdir path.
pub fn ingest_router() -> Router {
    build_ingest_router(Arc::new(DiscoveredStore::default()))
}

fn build_ingest_router(store: Arc<DiscoveredStore>) -> Router {
    Router::new()
        .route("/api/discovered", post(handle_ingest))
        .with_state(store)
}

async fn handle_ingest(State(store): State<Arc<DiscoveredStore>>, Json(body): Json<IngestBody>) -> Response {
    if store.record(&body.mac) {
        StatusCode::NO_CONTENT.into_response()
    } else {
        // A malformed MAC is the follower's problem, not a server fault, but 400
        // makes a bad follower loudly visible rather than silently no-op.
        (StatusCode::BAD_REQUEST, "invalid mac").into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store() -> (DiscoveredStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = DiscoveredStore::new(dir.path().join("discovered-macs.json"));
        (store, dir)
    }

    #[test]
    fn missing_file_is_empty_inbox() {
        let (store, _d) = temp_store();
        assert!(store.list().is_empty());
    }

    #[test]
    fn record_creates_then_refreshes_without_duplicating() {
        let (store, _d) = temp_store();
        assert!(store.record("6c:4b:90:bc:39:b3"));
        let rows = store.list();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].mac, "6c:4b:90:bc:39:b3");
        assert_eq!(rows[0].first_seen, rows[0].last_seen);
        assert!(!rows[0].dismissed);

        // Re-record: still one row, first_seen preserved.
        let first_seen = rows[0].first_seen.clone();
        assert!(store.record("6C:4B:90:BC:39:B3")); // different case → same MAC
        let rows = store.list();
        assert_eq!(rows.len(), 1, "a returning MAC must not duplicate");
        assert_eq!(rows[0].first_seen, first_seen, "first_seen must survive re-sighting");
    }

    #[test]
    fn dismiss_sets_flag_and_survives_resighting() {
        let (store, _d) = temp_store();
        store.record("aa:bb:cc:dd:ee:ff");
        assert!(store.dismiss("aa:bb:cc:dd:ee:ff"));
        assert!(store.list()[0].dismissed);

        // A dismissed MAC that DHCPs again stays dismissed, only last_seen moves.
        store.record("aa:bb:cc:dd:ee:ff");
        let rows = store.list();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].dismissed, "dismiss must survive a later re-sighting");
    }

    #[test]
    fn dismiss_unknown_mac_is_false() {
        let (store, _d) = temp_store();
        assert!(!store.dismiss("11:22:33:44:55:66"));
    }

    #[test]
    fn malformed_mac_records_nothing() {
        let (store, _d) = temp_store();
        assert!(!store.record("not-a-mac"));
        assert!(!store.record("6c:4b:90:bc:39")); // 5 octets
        assert!(!store.record("")); // empty
        assert!(store.list().is_empty());
    }

    #[test]
    fn canonical_mac_normalizes_separators_and_case() {
        assert_eq!(canonical_mac("6C4B90BC39B3").as_deref(), Some("6c:4b:90:bc:39:b3"));
        assert_eq!(canonical_mac("6c-4b-90-bc-39-b3").as_deref(), Some("6c:4b:90:bc:39:b3"));
        assert_eq!(canonical_mac("zz:zz:zz:zz:zz:zz"), None);
    }
}
