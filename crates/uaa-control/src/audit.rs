// file: crates/uaa-control/src/audit.rs
// version: 1.1.0
// guid: 8f807be1-d330-43bd-b5a5-c38d17dfcb66
// last-edited: 2026-07-10

//! Hash-chained audit log (spec Decision 21) + daily ed25519 checkpoints.
//!
//! Every mutating action in the constellation is recorded as an
//! [`AuditEventRow`] that is cryptographically linked to its predecessor:
//! `hash = SHA-256(prev_hash || canonical_bytes(event))`, where
//! `canonical_bytes` is the JSON serialization of
//! `(at, actor, role, action, target, outcome, detail)` with sorted keys
//! (built via a [`std::collections::BTreeMap`] so ordering never depends on
//! `serde_json`'s `preserve_order` feature). The very first event's
//! `prev_hash` is exactly [`GENESIS_PREV_HASH`] — 32 zero bytes.
//!
//! # Serialization (Decision 21, repeated here and in [`AuditStore`])
//!
//! The append happens in the SAME database transaction as the mutation it
//! records, and `prev_hash` is read from the chain tip under
//! `SELECT hash FROM audit_events ORDER BY seq DESC LIMIT 1 FOR UPDATE`. Two
//! concurrent mutations therefore serialize on that row lock — the tip read
//! and the subsequent insert are one critical section, so the chain can
//! NEVER fork. [`MemAuditStore`] emulates this with a `std::sync::Mutex` held
//! across read-tip -> (caller mutation) -> insert; [`PgAuditStore`] pins the
//! shape in SQL text (`SQL_SELECT_TIP` literally contains `FOR UPDATE`,
//! asserted by `test_pg_tip_sql_has_for_update`) so it is unavoidable for any
//! real implementation.
//!
//! # Threat model (STATED, not extended — Decision 21b)
//!
//! The chain plus the on-server ed25519 audit key defends against a **rogue
//! operator without server root**: they cannot rewrite history through the
//! normal API surface without [`verify_chain`] detecting the break. A
//! **server-root adversary defeats it** — they can rewrite the CockroachDB
//! rows and the on-disk signing key together, and nothing in this module
//! claims otherwise. Out-of-band checkpoint witnessing (publishing the daily
//! signed tip somewhere the root adversary cannot also rewrite) is recorded
//! P2 hardening and is explicitly NOT built here.
//!
//! # Cargo tests need no live CockroachDB
//!
//! Everything DB-shaped sits behind the [`AuditStore`] trait. Unit tests use
//! [`MemAuditStore`]; [`PgAuditStore`] is the runtime implementation and is
//! never constructed or exercised by `cargo test` (its SQL is asserted as
//! text only).

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::sync::Mutex;

use ed25519_dalek::{Signature, Signer, SigningKey};
use rand::rngs::OsRng;
use sha2::{Digest, Sha256};

use crate::db::{AuditCheckpointRow, AuditEventRow};

/// Genesis `prev_hash`: exactly 32 zero bytes. The first event in the chain
/// (and only the first) has this as its `prev_hash`.
pub const GENESIS_PREV_HASH: [u8; 32] = [0u8; 32];

/// Owner-only file mode for the on-server audit signing key.
const OWNER_ONLY: u32 = 0o600;

/// Filename of the on-server ed25519 audit signing key under `state_dir`.
const AUDIT_KEY_FILENAME: &str = "audit-signing-key";

// ── Canonical hashing ───────────────────────────────────────────────────────

/// The not-yet-persisted shape of an audit event: exactly the fields that
/// feed the hash (`at, actor, role, action, target, outcome, detail`). `at`
/// is minted by the caller (never the database's `DEFAULT now()`) because it
/// must be fixed and known BEFORE the hash is computed.
#[derive(Debug, Clone)]
pub struct NewAuditEvent {
    pub at: String,
    pub actor: String,
    pub role: String,
    pub action: String,
    pub target: Option<String>,
    pub outcome: String,
    pub detail: Option<serde_json::Value>,
}

impl From<&AuditEventRow> for NewAuditEvent {
    /// Reconstructs the hashed fields from a persisted row — used by
    /// [`verify_chain`] to recompute (never trust) each stored `hash`.
    fn from(row: &AuditEventRow) -> Self {
        Self {
            at: row.at.clone().unwrap_or_default(),
            actor: row.actor.clone(),
            role: row.role.clone(),
            action: row.action.clone(),
            target: row.target.clone(),
            outcome: row.outcome.clone(),
            detail: row.detail.clone(),
        }
    }
}

/// Deterministic JSON bytes for `event`: a `BTreeMap` (sorted keys) over
/// `(at, actor, role, action, target, outcome, detail)`. Missing `target`/
/// `detail` serialize as JSON `null`, never an absent key, so the shape is
/// stable across events regardless of which optional fields are set.
fn canonical_bytes(event: &NewAuditEvent) -> Vec<u8> {
    let mut map: BTreeMap<&'static str, serde_json::Value> = BTreeMap::new();
    map.insert("at", serde_json::Value::String(event.at.clone()));
    map.insert("actor", serde_json::Value::String(event.actor.clone()));
    map.insert("role", serde_json::Value::String(event.role.clone()));
    map.insert("action", serde_json::Value::String(event.action.clone()));
    map.insert(
        "target",
        event
            .target
            .clone()
            .map(serde_json::Value::String)
            .unwrap_or(serde_json::Value::Null),
    );
    map.insert("outcome", serde_json::Value::String(event.outcome.clone()));
    map.insert(
        "detail",
        event.detail.clone().unwrap_or(serde_json::Value::Null),
    );
    serde_json::to_vec(&map).expect("BTreeMap<&str, Value> always serializes")
}

/// `hash = SHA-256(prev_hash || canonical_bytes(event))` (spec Decision 21).
/// Re-computable from a persisted row via `NewAuditEvent::from`, which is
/// exactly what [`verify_chain`] does to detect tampering.
pub fn event_hash(prev_hash: &[u8; 32], event: &NewAuditEvent) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(prev_hash);
    hasher.update(canonical_bytes(event));
    hasher.finalize().into()
}

// ── Store trait (behind which live/mock persistence hides) ─────────────────

/// A caller-supplied unit of work that must commit atomically WITH the audit
/// append (the "mutation this event records" — spec Decision 21). It runs
/// inside the same critical section as the tip lock, after the tip is read
/// and before the audit row is inserted. `record`/`backfill` pass a no-op
/// mutation (`Ok(())`) since they have no accompanying registry change of
/// their own; sibling tasks recording an audited registry mutation pass
/// their real mutation here so it is impossible to append the audit event
/// without also committing (or, on error, rolling back) the paired change.
pub type MutationFn<'a> = Box<dyn FnOnce() -> anyhow::Result<()> + Send + 'a>;

/// Persistence seam for the audit chain. Every method here is the one place
/// concurrency correctness (Decision 21: never fork) is enforced —
/// [`MemAuditStore`] with a held `Mutex`, [`PgAuditStore`] with `SELECT ...
/// FOR UPDATE` inside a CockroachDB transaction.
#[async_trait::async_trait]
pub trait AuditStore: Send + Sync {
    /// Lock+read the tip, run `mutation`, compute the chained hash, insert —
    /// all as ONE critical section / database transaction. Concurrent
    /// callers serialize on the tip lock; the chain can never fork.
    async fn append_in_txn(
        &self,
        mutation: MutationFn<'_>,
        event: NewAuditEvent,
    ) -> anyhow::Result<AuditEventRow>;

    /// Events with `seq >= from_seq`, ordered ascending by `seq`.
    async fn list_events(&self, from_seq: i64) -> anyhow::Result<Vec<AuditEventRow>>;

    /// The current chain tip as `(seq, hash)`, or `None` for an empty chain.
    async fn tip(&self) -> anyhow::Result<Option<(i64, Vec<u8>)>>;

    /// Persist one daily checkpoint row.
    async fn insert_checkpoint(&self, checkpoint: AuditCheckpointRow) -> anyhow::Result<()>;
}

// ── MemAuditStore: the test double (and export for sibling tasks) ──────────

#[derive(Debug, Default)]
struct MemInner {
    events: Vec<AuditEventRow>,
    checkpoints: Vec<AuditCheckpointRow>,
}

/// In-memory [`AuditStore`] for `cargo test` (zero network, zero CockroachDB)
/// and for sibling control tasks that need a chain in their own unit tests.
/// A single `std::sync::Mutex` is held across tip-read -> mutation -> insert,
/// exactly emulating CockroachDB's `SELECT ... FOR UPDATE` tip lock: no
/// `.await` ever happens while the guard is held, so this is safe under the
/// multi-threaded tokio runtime and proves the serialization property
/// (`test_concurrent_appends_never_fork`) without a real database.
#[derive(Debug, Default)]
pub struct MemAuditStore {
    inner: Mutex<MemInner>,
}

impl MemAuditStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait::async_trait]
impl AuditStore for MemAuditStore {
    async fn append_in_txn(
        &self,
        mutation: MutationFn<'_>,
        event: NewAuditEvent,
    ) -> anyhow::Result<AuditEventRow> {
        let mut guard = self.inner.lock().expect("MemAuditStore mutex poisoned");

        let prev_hash: [u8; 32] = match guard.events.last() {
            Some(tip) => to_arr32(&tip.hash).unwrap_or(GENESIS_PREV_HASH),
            None => GENESIS_PREV_HASH,
        };

        // The paired mutation runs INSIDE the lock: it commits (or fails,
        // aborting the whole append) atomically with the audit row.
        mutation()?;

        let hash = event_hash(&prev_hash, &event);
        let seq = guard.events.len() as i64 + 1;
        let row = AuditEventRow {
            seq,
            at: Some(event.at),
            actor: event.actor,
            role: event.role,
            action: event.action,
            target: event.target,
            outcome: event.outcome,
            detail: event.detail,
            prev_hash: prev_hash.to_vec(),
            hash: hash.to_vec(),
        };
        guard.events.push(row.clone());
        Ok(row)
    }

    async fn list_events(&self, from_seq: i64) -> anyhow::Result<Vec<AuditEventRow>> {
        let guard = self.inner.lock().expect("MemAuditStore mutex poisoned");
        Ok(guard
            .events
            .iter()
            .filter(|e| e.seq >= from_seq)
            .cloned()
            .collect())
    }

    async fn tip(&self) -> anyhow::Result<Option<(i64, Vec<u8>)>> {
        let guard = self.inner.lock().expect("MemAuditStore mutex poisoned");
        Ok(guard.events.last().map(|e| (e.seq, e.hash.clone())))
    }

    async fn insert_checkpoint(&self, checkpoint: AuditCheckpointRow) -> anyhow::Result<()> {
        let mut guard = self.inner.lock().expect("MemAuditStore mutex poisoned");
        guard.checkpoints.push(checkpoint);
        Ok(())
    }
}

fn to_arr32(bytes: &[u8]) -> Option<[u8; 32]> {
    bytes.try_into().ok()
}

// ── PgAuditStore: the runtime implementation (never live-tested here) ──────

/// Tip-lock SQL (Decision 21). This literal `FOR UPDATE` is what makes
/// concurrent appends serialize on the tip row instead of forking the chain;
/// `test_pg_tip_sql_has_for_update` asserts it textually. Never executed by
/// `cargo test` — [`PgAuditStore`] requires a live CockroachDB connection.
const SQL_SELECT_TIP: &str = "SELECT seq, hash FROM audit_events ORDER BY seq DESC LIMIT 1 FOR UPDATE";
const SQL_SELECT_ONE_TIP: &str = "SELECT seq, hash FROM audit_events ORDER BY seq DESC LIMIT 1";
const SQL_INSERT_EVENT: &str = "INSERT INTO audit_events \
    (at, actor, role, action, target, outcome, detail, prev_hash, hash) \
    VALUES ($1::timestamptz, $2, $3, $4, $5, $6, $7::jsonb, $8, $9) \
    RETURNING seq, at::text, actor, role, action, target, outcome, detail::text, prev_hash, hash";
const SQL_SELECT_EVENTS_FROM: &str = "SELECT seq, at::text, actor, role, action, target, outcome, \
    detail::text, prev_hash, hash FROM audit_events WHERE seq >= $1 ORDER BY seq ASC";
const SQL_INSERT_CHECKPOINT: &str =
    "INSERT INTO audit_checkpoints (day, tip_seq, tip_hash, signature) VALUES ($1::date, $2, $3, $4)";

/// Real [`AuditStore`]: appends inside a CockroachDB transaction with the
/// tip locked under `FOR UPDATE` (`SQL_SELECT_TIP`). `detail` is round-
/// tripped as a text-cast JSONB column (`::jsonb` / `::text`) rather than
/// via `tokio-postgres`'s typed JSON conversion, so this compiles against
/// the workspace's default `tokio-postgres` feature set (no
/// `with-serde_json-1` needed). Constructed only at runtime — unit tests
/// never build one.
pub struct PgAuditStore {
    /// libpq-style connection string; secrets injected at runtime.
    pub conninfo: String,
}

impl PgAuditStore {
    async fn connect(&self) -> anyhow::Result<(tokio_postgres::Client, tokio::task::JoinHandle<()>)> {
        let (client, connection) =
            tokio_postgres::connect(&self.conninfo, tokio_postgres::NoTls).await?;
        let handle = tokio::spawn(async move {
            let _ = connection.await;
        });
        Ok((client, handle))
    }
}

fn row_from_pg(row: &tokio_postgres::Row) -> anyhow::Result<AuditEventRow> {
    let detail_text: Option<String> = row.get(7);
    let detail = detail_text.map(|t| serde_json::from_str(&t)).transpose()?;
    Ok(AuditEventRow {
        seq: row.get(0),
        at: row.get(1),
        actor: row.get(2),
        role: row.get(3),
        action: row.get(4),
        target: row.get(5),
        outcome: row.get(6),
        detail,
        prev_hash: row.get(8),
        hash: row.get(9),
    })
}

#[async_trait::async_trait]
impl AuditStore for PgAuditStore {
    async fn append_in_txn(
        &self,
        mutation: MutationFn<'_>,
        event: NewAuditEvent,
    ) -> anyhow::Result<AuditEventRow> {
        let (mut client, handle) = self.connect().await?;
        let txn = client.transaction().await?;

        // Tip lock: held for the rest of this transaction (commit/rollback
        // below), so a concurrent appender blocks here until we finish.
        let tip_row = txn.query_opt(SQL_SELECT_TIP, &[]).await?;
        let prev_hash: [u8; 32] = match tip_row {
            Some(row) => {
                let h: Vec<u8> = row.get(1);
                to_arr32(&h).ok_or_else(|| anyhow::anyhow!("corrupt tip hash length"))?
            }
            None => GENESIS_PREV_HASH,
        };

        mutation()?;

        let hash = event_hash(&prev_hash, &event);
        let detail_text = event.detail.as_ref().map(|v| v.to_string());
        let row = txn
            .query_one(
                SQL_INSERT_EVENT,
                &[
                    &event.at,
                    &event.actor,
                    &event.role,
                    &event.action,
                    &event.target,
                    &event.outcome,
                    &detail_text,
                    &prev_hash.to_vec(),
                    &hash.to_vec(),
                ],
            )
            .await?;
        txn.commit().await?;
        handle.abort();
        row_from_pg(&row)
    }

    async fn list_events(&self, from_seq: i64) -> anyhow::Result<Vec<AuditEventRow>> {
        let (client, handle) = self.connect().await?;
        let rows = client.query(SQL_SELECT_EVENTS_FROM, &[&from_seq]).await?;
        handle.abort();
        rows.iter().map(row_from_pg).collect()
    }

    async fn tip(&self) -> anyhow::Result<Option<(i64, Vec<u8>)>> {
        let (client, handle) = self.connect().await?;
        let row = client.query_opt(SQL_SELECT_ONE_TIP, &[]).await?;
        handle.abort();
        Ok(row.map(|r| (r.get(0), r.get(1))))
    }

    async fn insert_checkpoint(&self, checkpoint: AuditCheckpointRow) -> anyhow::Result<()> {
        let (client, handle) = self.connect().await?;
        client
            .execute(
                SQL_INSERT_CHECKPOINT,
                &[
                    &checkpoint.day,
                    &checkpoint.tip_seq,
                    &checkpoint.tip_hash,
                    &checkpoint.signature,
                ],
            )
            .await?;
        handle.abort();
        Ok(())
    }
}

// ── record / backfill ───────────────────────────────────────────────────────

/// Mints `at` (RFC3339, UTC) and appends a chained audit event with no
/// paired registry mutation of its own. This is the entrypoint used by
/// [`backfill`], the `audit checkpoint`/`audit verify` CLI paths, and most
/// tests. Sibling tasks that DO have an accompanying mutation should call
/// [`AuditStore::append_in_txn`] directly with their own [`MutationFn`] so
/// the mutation and the audit row commit atomically together.
#[allow(clippy::too_many_arguments)]
pub async fn record(
    store: &dyn AuditStore,
    actor: impl Into<String>,
    role: impl Into<String>,
    action: impl Into<String>,
    target: Option<String>,
    outcome: impl Into<String>,
    detail: Option<serde_json::Value>,
) -> anyhow::Result<AuditEventRow> {
    let event = NewAuditEvent {
        at: chrono::Utc::now().to_rfc3339(),
        actor: actor.into(),
        role: role.into(),
        action: action.into(),
        target,
        outcome: outcome.into(),
        detail,
    };
    store.append_in_txn(Box::new(|| Ok(())), event).await
}

/// The Decision-8 emergency-hatch companion: records an out-of-band
/// `cockroach sql` mutation AFTER the fact, through the SAME serialized
/// append path as any other event (`role = "system"`, `action` prefixed
/// `backfill:`). No side door — a backfilled event is chained exactly like
/// any other.
pub async fn backfill(
    store: &dyn AuditStore,
    actor: impl Into<String>,
    action: impl Into<String>,
    target: Option<String>,
    outcome: impl Into<String>,
    detail: Option<serde_json::Value>,
) -> anyhow::Result<AuditEventRow> {
    let action = format!("backfill:{}", action.into());
    record(store, actor, "system", action, target, outcome, detail).await
}

/// CLI-ready parameters for `uaa-control audit backfill`.
#[derive(Debug, Clone)]
pub struct BackfillArgs {
    pub actor: String,
    pub action: String,
    pub target: Option<String>,
    pub outcome: String,
    pub detail: Option<serde_json::Value>,
}

/// Runs [`backfill`] from parsed CLI args — the body of the `audit backfill`
/// subcommand, exposed here so `main.rs`'s clap wiring is a one-line call.
pub async fn run_backfill(store: &dyn AuditStore, args: BackfillArgs) -> anyhow::Result<AuditEventRow> {
    backfill(
        store,
        args.actor,
        args.action,
        args.target,
        args.outcome,
        args.detail,
    )
    .await
}

// ── verify_chain ─────────────────────────────────────────────────────────

/// Which check first failed at `seq` when a chain does not verify.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChainDefectKind {
    /// The first event's `prev_hash` is not [`GENESIS_PREV_HASH`].
    BadGenesis,
    /// A non-first event's `prev_hash` does not equal its predecessor's `hash`.
    BadPrevHash,
    /// A stored `hash` does not match the recomputed `event_hash`.
    BadHash,
}

/// A chain-integrity failure, naming the first bad `seq`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("audit chain defect at seq {seq}: {kind:?}")]
pub struct ChainDefect {
    pub seq: i64,
    pub kind: ChainDefectKind,
}

/// Walks `events` in the given order, recomputing every hash and checking
/// genesis + linkage, returning the FIRST defect found (never trusts a
/// stored `hash`/`prev_hash` without recomputing it). Callers pass events in
/// `seq`-ascending order (as returned by [`AuditStore::list_events`]); a
/// reordered or tampered slice is detected here.
pub fn verify_chain(events: &[AuditEventRow]) -> Result<(), ChainDefect> {
    let mut prev_expected = GENESIS_PREV_HASH;
    for (i, row) in events.iter().enumerate() {
        let row_prev = to_arr32(&row.prev_hash).unwrap_or([0xffu8; 32]);

        if i == 0 {
            if row_prev != GENESIS_PREV_HASH {
                return Err(ChainDefect {
                    seq: row.seq,
                    kind: ChainDefectKind::BadGenesis,
                });
            }
        } else if row_prev != prev_expected {
            return Err(ChainDefect {
                seq: row.seq,
                kind: ChainDefectKind::BadPrevHash,
            });
        }

        let recomputed = event_hash(&row_prev, &NewAuditEvent::from(row));
        let row_hash = to_arr32(&row.hash).unwrap_or([0xfeu8; 32]);
        if recomputed != row_hash {
            return Err(ChainDefect {
                seq: row.seq,
                kind: ChainDefectKind::BadHash,
            });
        }
        prev_expected = row_hash;
    }
    Ok(())
}

/// Runs [`verify_chain`] over the full stored chain and prints the outcome —
/// the body of the `audit verify` subcommand.
pub async fn run_verify(store: &dyn AuditStore) -> anyhow::Result<()> {
    let events = store.list_events(0).await?;
    match verify_chain(&events) {
        Ok(()) => {
            println!("audit chain OK: {} event(s) verified", events.len());
            Ok(())
        }
        Err(defect) => {
            println!("audit chain BROKEN: {defect}");
            Err(defect.into())
        }
    }
}

// ── Daily checkpoint ─────────────────────────────────────────────────────

/// Typed refusal reasons for [`daily_checkpoint`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CheckpointError {
    /// The chain has no events yet; refuse rather than sign an empty tip.
    #[error("audit chain is empty; refusing to sign an empty checkpoint")]
    EmptyChain,
}

/// The exact bytes a daily checkpoint signs: `day || tip_seq (8-byte BE) ||
/// tip_hash`. Exposed so out-of-band witnessing tooling (P2 hardening,
/// Decision 21b — not built here) can independently verify a checkpoint
/// signature without re-deriving this layout.
pub fn checkpoint_signing_bytes(day: &str, tip_seq: i64, tip_hash: &[u8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(day.len() + 8 + tip_hash.len());
    buf.extend_from_slice(day.as_bytes());
    buf.extend_from_slice(&tip_seq.to_be_bytes());
    buf.extend_from_slice(tip_hash);
    buf
}

/// Signs `day || tip_seq || tip_hash` with the on-server ed25519 audit key
/// and persists the checkpoint row. An empty chain (no events yet) is
/// refused with [`CheckpointError::EmptyChain`] — never a signed empty tip.
pub async fn daily_checkpoint(
    store: &dyn AuditStore,
    signing_key: &SigningKey,
    day: &str,
) -> anyhow::Result<AuditCheckpointRow> {
    let (tip_seq, tip_hash) = store.tip().await?.ok_or(CheckpointError::EmptyChain)?;
    let signing_bytes = checkpoint_signing_bytes(day, tip_seq, &tip_hash);
    let signature: Signature = signing_key.sign(&signing_bytes);

    let checkpoint = AuditCheckpointRow {
        day: day.to_string(),
        tip_seq,
        tip_hash,
        signature: signature.to_bytes().to_vec(),
    };
    store.insert_checkpoint(checkpoint.clone()).await?;
    Ok(checkpoint)
}

/// Runs [`daily_checkpoint`] and prints the outcome — the body of the
/// `audit checkpoint` subcommand.
pub async fn run_checkpoint(
    store: &dyn AuditStore,
    signing_key: &SigningKey,
    day: &str,
) -> anyhow::Result<AuditCheckpointRow> {
    let checkpoint = daily_checkpoint(store, signing_key, day).await?;
    println!(
        "checkpoint signed for {day}: tip_seq={} tip_hash={}",
        checkpoint.tip_seq,
        hex_encode(&checkpoint.tip_hash)
    );
    Ok(checkpoint)
}

/// Today's UTC calendar date as `YYYY-MM-DD`, for `audit checkpoint`'s "sign
/// today" default.
pub fn today_utc_date() -> String {
    chrono::Utc::now().format("%Y-%m-%d").to_string()
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// ── On-server audit signing key ─────────────────────────────────────────

/// Loads the on-server ed25519 audit key from `<state_dir>/audit-signing-key`,
/// generating and persisting (0600) a fresh one on first start. Tests pass a
/// tempdir; production passes the daemon's real state directory.
pub fn load_or_create_audit_key(state_dir: &Path) -> anyhow::Result<SigningKey> {
    let key_path = state_dir.join(AUDIT_KEY_FILENAME);

    if let Ok(bytes) = fs::read(&key_path) {
        let arr = to_arr32(&bytes).ok_or_else(|| {
            anyhow::anyhow!(
                "corrupt audit signing key at {}: expected 32 bytes, found {}",
                key_path.display(),
                bytes.len()
            )
        })?;
        return Ok(SigningKey::from_bytes(&arr));
    }

    fs::create_dir_all(state_dir)?;
    let key = SigningKey::generate(&mut OsRng);
    let mut f = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(OWNER_ONLY)
        .open(&key_path)?;
    f.write_all(&key.to_bytes())?;
    f.flush()?;
    fs::set_permissions(&key_path, fs::Permissions::from_mode(OWNER_ONLY))?;
    Ok(key)
}

// ── Unit tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::Verifier;
    use std::sync::Arc;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_genesis_prev_hash_is_zero() {
        let store = MemAuditStore::new();
        let row = record(&store, "alice", "operator", "test.action", None, "success", None)
            .await
            .unwrap();
        assert_eq!(row.seq, 1);
        assert_eq!(row.prev_hash, GENESIS_PREV_HASH.to_vec());
    }

    #[tokio::test]
    async fn test_append_links_prev_hash() {
        let store = MemAuditStore::new();
        let e1 = record(&store, "a", "operator", "act1", None, "ok", None).await.unwrap();
        let e2 = record(&store, "a", "operator", "act2", None, "ok", None).await.unwrap();
        let e3 = record(&store, "a", "operator", "act3", None, "ok", None).await.unwrap();

        assert_eq!(e1.prev_hash, GENESIS_PREV_HASH.to_vec());
        assert_eq!(e2.prev_hash, e1.hash, "e2 must link to e1's hash");
        assert_eq!(e3.prev_hash, e2.hash, "e3 must link to e2's hash");

        let all = store.list_events(0).await.unwrap();
        assert_eq!(all.len(), 3);
        assert!(verify_chain(&all).is_ok());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_concurrent_appends_never_fork() {
        // THE Decision-21 test: 16 tasks race to append through the same
        // store. If the tip lock ever let two appends read the same tip,
        // this would produce duplicate seqs, duplicate prev_hash links, or
        // a `verify_chain` failure. None of that may happen.
        let store = Arc::new(MemAuditStore::new());
        let mut handles = Vec::new();
        for i in 0..16 {
            let store = Arc::clone(&store);
            handles.push(tokio::spawn(async move {
                record(
                    &*store,
                    format!("actor-{i}"),
                    "operator",
                    "concurrent.append",
                    None,
                    "ok",
                    None,
                )
                .await
                .unwrap()
            }));
        }

        let mut seqs: Vec<i64> = Vec::new();
        for h in handles {
            seqs.push(h.await.unwrap().seq);
        }
        seqs.sort_unstable();
        assert_eq!(
            seqs,
            (1..=16).collect::<Vec<_>>(),
            "seq must be exactly 1..=16 with no gaps or duplicates"
        );

        let all = store.list_events(0).await.unwrap();
        assert_eq!(all.len(), 16);
        assert!(
            verify_chain(&all).is_ok(),
            "16 concurrent appends must form ONE linear verified chain, never fork"
        );
    }

    #[tokio::test]
    async fn test_verify_detects_tamper() {
        let store = MemAuditStore::new();
        record(&store, "a", "operator", "act1", None, "ok", None).await.unwrap();
        record(
            &store,
            "a",
            "operator",
            "act2",
            None,
            "ok",
            Some(serde_json::json!({"x": 1})),
        )
        .await
        .unwrap();
        record(&store, "a", "operator", "act3", None, "ok", None).await.unwrap();

        let mut events = store.list_events(0).await.unwrap();
        // Tamper the middle event's detail WITHOUT recomputing its hash.
        events[1].detail = Some(serde_json::json!({"x": 999}));

        let err = verify_chain(&events).unwrap_err();
        assert_eq!(err.seq, events[1].seq);
        assert_eq!(err.kind, ChainDefectKind::BadHash);
    }

    #[tokio::test]
    async fn test_verify_detects_reorder() {
        let store = MemAuditStore::new();
        for i in 0..4 {
            record(&store, "a", "operator", format!("act{i}"), None, "ok", None)
                .await
                .unwrap();
        }
        let mut events = store.list_events(0).await.unwrap();
        events.swap(1, 2);

        let err = verify_chain(&events).unwrap_err();
        assert_eq!(err.kind, ChainDefectKind::BadPrevHash);
    }

    #[tokio::test]
    async fn test_checkpoint_signs_tip() {
        let store = MemAuditStore::new();
        record(&store, "a", "operator", "act1", None, "ok", None).await.unwrap();
        let last = record(&store, "a", "operator", "act2", None, "ok", None).await.unwrap();

        let dir = tempdir().unwrap();
        let signing_key = load_or_create_audit_key(dir.path()).unwrap();
        let verifying_key = signing_key.verifying_key();

        let checkpoint = daily_checkpoint(&store, &signing_key, "2026-07-10").await.unwrap();
        assert_eq!(checkpoint.tip_seq, last.seq);
        assert_eq!(checkpoint.tip_hash, last.hash);

        let signing_bytes =
            checkpoint_signing_bytes(&checkpoint.day, checkpoint.tip_seq, &checkpoint.tip_hash);
        let sig_bytes: [u8; 64] = checkpoint.signature.as_slice().try_into().unwrap();
        let sig = Signature::from_bytes(&sig_bytes);
        assert!(verifying_key.verify(&signing_bytes, &sig).is_ok());
    }

    #[tokio::test]
    async fn test_checkpoint_empty_chain_refused() {
        let store = MemAuditStore::new();
        let dir = tempdir().unwrap();
        let signing_key = load_or_create_audit_key(dir.path()).unwrap();

        let err = daily_checkpoint(&store, &signing_key, "2026-07-10").await.unwrap_err();
        assert_eq!(
            err.downcast_ref::<CheckpointError>(),
            Some(&CheckpointError::EmptyChain),
            "must refuse with a typed error, never sign an empty tip"
        );
        assert!(store.tip().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_backfill_goes_through_chain() {
        let store = MemAuditStore::new();
        record(&store, "a", "operator", "normal.action", None, "ok", None)
            .await
            .unwrap();

        let row = run_backfill(
            &store,
            BackfillArgs {
                actor: "github-login".into(),
                action: "manual-fix".into(),
                target: Some("machine:aa:bb:cc:dd:ee:ff".into()),
                outcome: "applied".into(),
                detail: Some(serde_json::json!({"note": "ran cockroach sql by hand"})),
            },
        )
        .await
        .unwrap();

        assert_eq!(row.role, "system");
        assert!(
            row.action.starts_with("backfill:"),
            "backfill action must be prefixed"
        );
        assert_eq!(row.seq, 2, "backfill goes through the SAME serialized append (no side door)");

        let all = store.list_events(0).await.unwrap();
        assert!(verify_chain(&all).is_ok());
    }

    #[test]
    fn test_pg_tip_sql_has_for_update() {
        assert!(SQL_SELECT_TIP.contains("FOR UPDATE"));
    }

    #[tokio::test]
    async fn test_record_happy_path_returns_row() {
        // Anti-over-suppression: the tip lock must not deadlock or block the
        // ordinary single-append path, and every field must round-trip.
        let store = MemAuditStore::new();
        let row = record(
            &store,
            "alice",
            "operator",
            "machine.approve",
            Some("aa:bb:cc:dd:ee:ff".into()),
            "success",
            Some(serde_json::json!({"note": "ok"})),
        )
        .await
        .unwrap();

        assert_eq!(row.seq, 1);
        assert_eq!(row.actor, "alice");
        assert_eq!(row.role, "operator");
        assert_eq!(row.action, "machine.approve");
        assert_eq!(row.target.as_deref(), Some("aa:bb:cc:dd:ee:ff"));
        assert_eq!(row.outcome, "success");
        assert_eq!(row.detail, Some(serde_json::json!({"note": "ok"})));
        assert_eq!(row.prev_hash, GENESIS_PREV_HASH.to_vec());
        assert!(verify_chain(std::slice::from_ref(&row)).is_ok());
    }

    #[test]
    fn test_load_or_create_audit_key_persists_0600() {
        let dir = tempdir().unwrap();
        let key1 = load_or_create_audit_key(dir.path()).unwrap();
        let key2 = load_or_create_audit_key(dir.path()).unwrap();
        assert_eq!(
            key1.to_bytes(),
            key2.to_bytes(),
            "second call must load the persisted key, not mint a new one"
        );

        let mode = fs::metadata(dir.path().join(AUDIT_KEY_FILENAME))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }
}
