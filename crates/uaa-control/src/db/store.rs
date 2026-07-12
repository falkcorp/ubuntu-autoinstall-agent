// file: crates/uaa-control/src/db/store.rs
// version: 1.0.1
// guid: a471e102-2da9-4bf8-8531-1de7595fd24d
// last-edited: 2026-07-12

//! Degraded-mode layer (spec Decision 4).
//!
//! CockroachDB is the system-of-record; when it is unreachable uaa-control degrades
//! deterministically:
//!   * reads are served from a local JSON snapshot (`registry-snapshot.json`);
//!   * mutations fail CLOSED with [`StoreError::Degraded`] → HTTP 503 — EXCEPT
//!   * telemetry ingestion (webhook/checkin/install events) which fails OPEN by
//!     appending to a write-ahead log (`wal.jsonl`).
//!
//! Every WAL entry carries an `event_id` UUID minted at ingest. On reconnect the WAL
//! is replayed with `INSERT ... ON CONFLICT (event_id) DO NOTHING`; an entry is marked
//! consumed ONLY after its CRDB txn commits, so a crash between commit and mark is
//! safe (dedup makes re-replay a no-op). WAL-wins over the snapshot (it is strictly
//! newer). Total quorum loss is explicitly out of scope (spec Non-goals).
//!
//! Everything DB-shaped sits behind a trait ([`DbHealth`], [`WalApply`]) so the unit
//! tests run with zero network and zero CockroachDB — they use in-memory mocks and a
//! tempdir for the snapshot/WAL files.

use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::{
    DiscoveredMacRow, EnrollmentRow, LuksCredentialRow, MachineRow, TangServerRow, YubikeyRow,
};

/// Owner-only file mode for every snapshot/WAL/quarantine artifact (secrets-adjacent).
const OWNER_ONLY: u32 = 0o600;

/// Errors surfaced by the degraded-mode store. [`StoreError::Degraded`] is the
/// fail-closed signal the HTTP layers map to `503 Service Unavailable`.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// The registry is degraded (CRDB unreachable); a fail-closed mutation was refused.
    #[error("registry degraded: mutation refused (fail-closed → 503)")]
    Degraded,
    /// Filesystem error while reading/writing snapshot or WAL.
    #[error("store io error: {0}")]
    Io(#[from] std::io::Error),
    /// (De)serialization error for a snapshot or WAL payload.
    #[error("store serde error: {0}")]
    Serde(#[from] serde_json::Error),
}

/// Filesystem locations for the degraded-mode artifacts. ALWAYS constructed from
/// config (never hard-coded at a call site) so tests can point them at a tempdir.
#[derive(Debug, Clone)]
pub struct StatePaths {
    /// Registry read snapshot, rewritten tmp+rename after every successful mutation.
    pub snapshot: PathBuf,
    /// Append-only write-ahead log of telemetry ingested while degraded.
    pub wal: PathBuf,
    /// Ledger of WAL `event_id`s already committed to CRDB (dedup / crash-safety).
    pub wal_consumed: PathBuf,
    /// Corrupt WAL lines quarantined during replay (kept, never abandoned).
    pub quarantine: PathBuf,
}

impl Default for StatePaths {
    fn default() -> Self {
        let base = Path::new("/var/lib/uaa");
        Self {
            snapshot: base.join("registry-snapshot.json"),
            wal: base.join("wal.jsonl"),
            wal_consumed: base.join("wal.consumed"),
            quarantine: base.join("wal.quarantine.jsonl"),
        }
    }
}

impl StatePaths {
    /// Build a set of paths rooted under `dir` (used by tests + a `--state-dir` flag).
    pub fn under(dir: impl AsRef<Path>) -> Self {
        let dir = dir.as_ref();
        Self {
            snapshot: dir.join("registry-snapshot.json"),
            wal: dir.join("wal.jsonl"),
            wal_consumed: dir.join("wal.consumed"),
            quarantine: dir.join("wal.quarantine.jsonl"),
        }
    }
}

/// The full registry as served from the local snapshot while degraded. Followers
/// extend this doc as they add tables to the snapshot; `default()` is the EMPTY
/// registry returned when the snapshot file is missing (never a panic).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SnapshotDoc {
    #[serde(default)]
    pub machines: Vec<MachineRow>,
    #[serde(default)]
    pub enrollments: Vec<EnrollmentRow>,
    #[serde(default)]
    pub yubikeys: Vec<YubikeyRow>,
    #[serde(default)]
    pub luks_credentials: Vec<LuksCredentialRow>,
    #[serde(default)]
    pub tang_servers: Vec<TangServerRow>,
    #[serde(default)]
    pub discovered_macs: Vec<DiscoveredMacRow>,
}

/// A single write-ahead-log entry. `event_id` is minted at ingest and is the sole
/// dedup key for replay (`INSERT ... ON CONFLICT (event_id) DO NOTHING`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WalEntry {
    pub event_id: uuid::Uuid,
    pub kind: String,
    pub payload: serde_json::Value,
    pub at: String,
}

/// Outcome of a [`wal_replay`] pass.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ReplayReport {
    /// Entries whose `apply` committed on this pass (newly marked consumed).
    pub applied: usize,
    /// Entries skipped because their `event_id` was already consumed (dedup).
    pub skipped: usize,
    /// Corrupt lines copied to quarantine and skipped.
    pub quarantined: usize,
    /// Entries whose `apply` returned Err — NOT marked consumed, re-delivered next pass.
    pub failed: usize,
}

/// Health probe for the CRDB connection. The real impl ([`PgHealth`]) enforces the
/// spec's 2s-connect / 5s-query timeouts; tests use `MockHealth`.
#[async_trait::async_trait]
pub trait DbHealth: Send + Sync {
    async fn healthy(&self) -> bool;
}

/// Applies one replayed WAL entry to CRDB. The real impl runs
/// `INSERT ... ON CONFLICT (event_id) DO NOTHING` inside a txn; the mock records it.
#[async_trait::async_trait]
pub trait WalApply {
    async fn apply(&mut self, entry: &WalEntry) -> anyhow::Result<()>;
}

/// Real [`DbHealth`]: a bounded connect + probe query against CRDB.
///
/// Uses `NoTls` as a scaffold — rustls is the runtime transport per Decision 5, wired
/// when the TLS material lands (PK-03/CT-07). This struct is constructed only at
/// runtime; unit tests never build it (no live database in the test path).
pub struct PgHealth {
    /// libpq-style connection string (host/port/user/db); secrets injected at runtime.
    pub conninfo: String,
}

#[async_trait::async_trait]
impl DbHealth for PgHealth {
    async fn healthy(&self) -> bool {
        let connect = tokio_postgres::connect(&self.conninfo, tokio_postgres::NoTls);
        let (client, connection) = match tokio::time::timeout(Duration::from_secs(2), connect).await
        {
            Ok(Ok(pair)) => pair,
            _ => return false,
        };
        // Drive the connection future in the background for the life of the probe.
        let handle = tokio::spawn(async move {
            let _ = connection.await;
        });
        let probe = client.simple_query("SELECT 1");
        let ok = matches!(
            tokio::time::timeout(Duration::from_secs(5), probe).await,
            Ok(Ok(_))
        );
        drop(client);
        handle.abort();
        ok
    }
}

/// Atomically write the registry snapshot: serialize to `<snapshot>.tmp`, chmod 0600,
/// then `rename` over the live file (rename is atomic on the same filesystem, so a
/// reader never observes a half-written snapshot). Mirrors the Python ground-truth
/// `save_registry` tmp+`os.replace` idiom (`scripts/autoinstall-agent.py`).
///
/// CONTRACT: every follower that commits a mutation to CRDB MUST call this afterward
/// so the degraded read path stays current.
pub fn write_snapshot(paths: &StatePaths, doc: &SnapshotDoc) -> Result<(), StoreError> {
    let mut tmp = paths.snapshot.clone().into_os_string();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);

    if let Some(parent) = paths.snapshot.parent() {
        fs::create_dir_all(parent)?;
    }

    let bytes = serde_json::to_vec_pretty(doc)?;
    {
        let mut f = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(OWNER_ONLY)
            .open(&tmp)?;
        f.write_all(&bytes)?;
        f.flush()?;
    }
    // Ensure the mode is 0600 even if a prior umask-affected tmp file existed.
    fs::set_permissions(&tmp, fs::Permissions::from_mode(OWNER_ONLY))?;
    fs::rename(&tmp, &paths.snapshot)?;
    Ok(())
}

/// Read the registry snapshot for degraded reads. A missing OR corrupt file yields an
/// EMPTY [`SnapshotDoc`] plus a loud `tracing::error!` — degraded reads must never
/// panic (spec edge semantics).
pub fn read_snapshot(paths: &StatePaths) -> SnapshotDoc {
    match fs::read(&paths.snapshot) {
        Ok(bytes) => match serde_json::from_slice::<SnapshotDoc>(&bytes) {
            Ok(doc) => doc,
            Err(err) => {
                tracing::error!(
                    path = %paths.snapshot.display(),
                    %err,
                    "registry snapshot is corrupt; serving EMPTY registry (degraded)"
                );
                SnapshotDoc::default()
            }
        },
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            tracing::error!(
                path = %paths.snapshot.display(),
                "registry snapshot is missing; serving EMPTY registry (degraded)"
            );
            SnapshotDoc::default()
        }
        Err(err) => {
            tracing::error!(
                path = %paths.snapshot.display(),
                %err,
                "registry snapshot unreadable; serving EMPTY registry (degraded)"
            );
            SnapshotDoc::default()
        }
    }
}

/// Append one telemetry event to the WAL, minting its `event_id` at ingest. Returns
/// the minted id so the caller can correlate. The WAL file is created 0600.
pub fn wal_append(
    paths: &StatePaths,
    kind: impl Into<String>,
    payload: serde_json::Value,
) -> Result<uuid::Uuid, StoreError> {
    let event_id = uuid::Uuid::new_v4();
    let entry = WalEntry {
        event_id,
        kind: kind.into(),
        payload,
        at: now_epoch_secs(),
    };
    let line = serde_json::to_string(&entry)?;
    append_line(&paths.wal, &line)?;
    Ok(event_id)
}

/// Replay the WAL against CRDB. For each not-yet-consumed entry, `apply` is invoked
/// (real impl: `INSERT ... ON CONFLICT (event_id) DO NOTHING`); the entry is marked
/// consumed ONLY after `apply` returns Ok. Corrupt lines are copied to quarantine and
/// skipped (the tail is never abandoned). Dedup is by `event_id`, so re-running replay
/// after a crash re-applies nothing.
pub async fn wal_replay(
    paths: &StatePaths,
    apply: &mut dyn WalApply,
) -> Result<ReplayReport, StoreError> {
    let mut report = ReplayReport::default();
    let mut consumed = read_consumed(paths)?;

    let content = match fs::read_to_string(&paths.wal) {
        Ok(c) => c,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(report),
        Err(err) => return Err(err.into()),
    };

    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let entry: WalEntry = match serde_json::from_str(line) {
            Ok(entry) => entry,
            Err(err) => {
                tracing::error!(%err, "corrupt WAL line quarantined; continuing replay");
                append_line(&paths.quarantine, line)?;
                report.quarantined += 1;
                continue;
            }
        };
        if consumed.contains(&entry.event_id) {
            report.skipped += 1;
            continue;
        }
        match apply.apply(&entry).await {
            Ok(()) => {
                // Mark consumed ONLY after the apply (CRDB commit) succeeds.
                append_line(&paths.wal_consumed, &entry.event_id.to_string())?;
                consumed.insert(entry.event_id);
                report.applied += 1;
            }
            Err(err) => {
                tracing::warn!(event_id = %entry.event_id, %err,
                    "WAL apply failed; NOT marking consumed (will re-deliver)");
                report.failed += 1;
            }
        }
    }
    Ok(report)
}

/// Guard a fail-closed mutation behind the health check. When degraded, returns
/// [`StoreError::Degraded`] and does NOT run the mutation or touch the snapshot. When
/// healthy, runs `mutation` and — on success — atomically rewrites the snapshot from
/// `next` (the WAL is untouched: this is the fail-closed path, not telemetry ingest).
pub async fn guarded_mutation<H, F>(
    health: &H,
    paths: &StatePaths,
    next: &SnapshotDoc,
    mutation: F,
) -> Result<(), StoreError>
where
    H: DbHealth + ?Sized,
    F: FnOnce() -> Result<(), StoreError>,
{
    if !health.healthy().await {
        return Err(StoreError::Degraded);
    }
    mutation()?;
    write_snapshot(paths, next)?;
    Ok(())
}

/// Unix epoch seconds as a string for the WAL `at` field (ordering/debug only; not a
/// dedup key — that is `event_id`). Followers may upgrade this to RFC3339 via chrono.
fn now_epoch_secs() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{secs}")
}

/// Append a single line (0600) to `path`, creating it if absent.
fn append_line(path: &Path, line: &str) -> Result<(), StoreError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(OWNER_ONLY)
        .open(path)?;
    // Enforce 0600 even if the file pre-existed with looser perms.
    fs::set_permissions(path, fs::Permissions::from_mode(OWNER_ONLY))?;
    writeln!(f, "{line}")?;
    Ok(())
}

/// Load the set of already-consumed `event_id`s; a missing ledger is an empty set.
fn read_consumed(paths: &StatePaths) -> Result<HashSet<uuid::Uuid>, StoreError> {
    match fs::read_to_string(&paths.wal_consumed) {
        Ok(content) => Ok(content
            .lines()
            .filter_map(|l| uuid::Uuid::parse_str(l.trim()).ok())
            .collect()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(HashSet::new()),
        Err(err) => Err(err.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    struct MockHealth(bool);
    #[async_trait::async_trait]
    impl DbHealth for MockHealth {
        async fn healthy(&self) -> bool {
            self.0
        }
    }

    /// Records each applied `event_id`; optionally fails to simulate a CRDB error.
    struct MockWalApply {
        applied: Vec<uuid::Uuid>,
        fail: bool,
    }
    #[async_trait::async_trait]
    impl WalApply for MockWalApply {
        async fn apply(&mut self, entry: &WalEntry) -> anyhow::Result<()> {
            if self.fail {
                anyhow::bail!("simulated CRDB apply failure");
            }
            self.applied.push(entry.event_id);
            Ok(())
        }
    }

    fn mode_of(path: &Path) -> u32 {
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn test_snapshot_write_is_atomic_and_0600() {
        let dir = tempdir().unwrap();
        let paths = StatePaths::under(dir.path());
        let doc = SnapshotDoc {
            machines: vec![MachineRow {
                mac: "aa:bb:cc:dd:ee:ff".into(),
                hostname: "h1".into(),
                ip: None,
                r#type: "lenovo".into(),
                status: crate::db::MachineStatus::Pending,
                boot_target: crate::db::BootTarget::LocalDisk,
                tpm_ek: None,
                registered_at: None,
                approved_at: None,
                last_seen: None,
                last_ip: None,
                installed_at: None,
                last_install_status: None,
                updated_at: None,
            }],
            ..Default::default()
        };
        write_snapshot(&paths, &doc).unwrap();

        assert!(paths.snapshot.exists(), "snapshot must exist");
        assert_eq!(mode_of(&paths.snapshot), 0o600, "snapshot must be 0600");

        // No .tmp residue left behind after the atomic rename.
        let mut tmp = paths.snapshot.clone().into_os_string();
        tmp.push(".tmp");
        assert!(!PathBuf::from(tmp).exists(), "no .tmp file may remain");

        let round = read_snapshot(&paths);
        assert_eq!(round.machines.len(), 1);
        assert_eq!(round.machines[0].mac, "aa:bb:cc:dd:ee:ff");
    }

    #[test]
    fn test_snapshot_missing_reads_empty() {
        let dir = tempdir().unwrap();
        let paths = StatePaths::under(dir.path());
        // No file written: must not panic, must return the empty default doc.
        let doc = read_snapshot(&paths);
        assert!(doc.machines.is_empty());
        assert!(doc.enrollments.is_empty());
    }

    #[test]
    fn test_wal_append_mints_event_id() {
        let dir = tempdir().unwrap();
        let paths = StatePaths::under(dir.path());
        let id = wal_append(&paths, "checkin", serde_json::json!({"mac": "aa"})).unwrap();

        assert_eq!(mode_of(&paths.wal), 0o600, "wal must be 0600");
        let content = fs::read_to_string(&paths.wal).unwrap();
        let line = content.lines().next().unwrap();
        let entry: WalEntry = serde_json::from_str(line).unwrap();
        assert_eq!(entry.event_id, id, "line must carry the minted id");
        // The id round-trips as a parseable UUID.
        assert_eq!(uuid::Uuid::parse_str(&id.to_string()).unwrap(), id);
    }

    #[tokio::test]
    async fn test_wal_replay_dedup() {
        let dir = tempdir().unwrap();
        let paths = StatePaths::under(dir.path());
        let id1 = wal_append(&paths, "a", serde_json::json!({"n": 1})).unwrap();
        let id2 = wal_append(&paths, "b", serde_json::json!({"n": 2})).unwrap();

        let mut apply = MockWalApply {
            applied: vec![],
            fail: false,
        };
        let r1 = wal_replay(&paths, &mut apply).await.unwrap();
        assert_eq!(r1.applied, 2);
        assert_eq!(apply.applied, vec![id1, id2], "each event applied once");

        // Second replay: both already consumed → nothing re-applied.
        let r2 = wal_replay(&paths, &mut apply).await.unwrap();
        assert_eq!(r2.applied, 0);
        assert_eq!(r2.skipped, 2);
        assert_eq!(apply.applied.len(), 2, "no event applied a second time");

        // Consumed-mark was written (only after Ok).
        let consumed = read_consumed(&paths).unwrap();
        assert!(consumed.contains(&id1) && consumed.contains(&id2));
    }

    #[tokio::test]
    async fn test_wal_replay_quarantines_corrupt_line() {
        let dir = tempdir().unwrap();
        let paths = StatePaths::under(dir.path());

        // Three lines: good, corrupt, good — written directly to control ordering.
        let e1 = WalEntry {
            event_id: uuid::Uuid::new_v4(),
            kind: "a".into(),
            payload: serde_json::json!({}),
            at: "0".into(),
        };
        let e3 = WalEntry {
            event_id: uuid::Uuid::new_v4(),
            kind: "c".into(),
            payload: serde_json::json!({}),
            at: "0".into(),
        };
        let body = format!(
            "{}\n{{ this is not valid json\n{}\n",
            serde_json::to_string(&e1).unwrap(),
            serde_json::to_string(&e3).unwrap()
        );
        fs::write(&paths.wal, body).unwrap();

        let mut apply = MockWalApply {
            applied: vec![],
            fail: false,
        };
        let report = wal_replay(&paths, &mut apply).await.unwrap();
        assert_eq!(report.applied, 2, "the two good lines apply");
        assert_eq!(report.quarantined, 1, "the corrupt line is quarantined");
        assert_eq!(apply.applied, vec![e1.event_id, e3.event_id]);

        let q = fs::read_to_string(&paths.quarantine).unwrap();
        assert!(
            q.contains("this is not valid json"),
            "corrupt line preserved"
        );
    }

    #[tokio::test]
    async fn test_wal_apply_failure_not_marked_consumed() {
        let dir = tempdir().unwrap();
        let paths = StatePaths::under(dir.path());
        let id = wal_append(&paths, "checkin", serde_json::json!({})).unwrap();

        // First pass: apply fails → entry NOT consumed.
        let mut failing = MockWalApply {
            applied: vec![],
            fail: true,
        };
        let r1 = wal_replay(&paths, &mut failing).await.unwrap();
        assert_eq!(r1.failed, 1);
        assert_eq!(r1.applied, 0);
        assert!(
            !read_consumed(&paths).unwrap().contains(&id),
            "failed entry must not be marked consumed"
        );

        // Retry with a healthy applier: the entry is RE-DELIVERED and now applied.
        let mut ok = MockWalApply {
            applied: vec![],
            fail: false,
        };
        let r2 = wal_replay(&paths, &mut ok).await.unwrap();
        assert_eq!(r2.applied, 1, "entry re-delivered after prior failure");
        assert_eq!(ok.applied, vec![id]);
        assert!(read_consumed(&paths).unwrap().contains(&id));
    }

    #[tokio::test]
    async fn test_mutation_degraded_fails_closed() {
        let dir = tempdir().unwrap();
        let paths = StatePaths::under(dir.path());
        let health = MockHealth(false);

        let mut ran = false;
        let result = guarded_mutation(&health, &paths, &SnapshotDoc::default(), || {
            ran = true;
            Ok(())
        })
        .await;

        assert!(matches!(result, Err(StoreError::Degraded)));
        assert!(!ran, "the mutation body must not run when degraded");
        assert!(
            !paths.snapshot.exists(),
            "degraded mutation must not touch the snapshot"
        );
    }

    #[tokio::test]
    async fn test_mutation_healthy_passes_and_snapshots() {
        let dir = tempdir().unwrap();
        let paths = StatePaths::under(dir.path());
        let health = MockHealth(true);

        let mut ran = false;
        let result = guarded_mutation(&health, &paths, &SnapshotDoc::default(), || {
            ran = true;
            Ok(())
        })
        .await;

        assert!(result.is_ok(), "healthy path must succeed");
        assert!(ran, "the mutation body must run on the happy path");
        assert!(
            paths.snapshot.exists(),
            "healthy mutation must rewrite the snapshot"
        );
        assert!(
            !paths.wal.exists(),
            "fail-closed mutation must NOT append to the WAL"
        );
    }
}
