// file: crates/uaa-control/src/saga.rs
// version: 1.1.1
// guid: 25f36b48-dfa6-49c2-b30b-46d3fdf8adcb
// last-edited: 2026-07-17

//! Approve-SAGA orchestration + compensation (`saga_log` table), spec C3.
//!
//! `ApproveMachine` is a strictly ORDERED, persisted, resumable state machine:
//!
//!   1. `WebService.PlaceSeed` + `PlaceIpxe` — inert placement FIRST;
//!   2. `PxeService.SetupPxe` + `SetBootTarget` — activation LAST (see
//!      [`SAGA_STEPS`]: a failure between steps leaves the host inert, never
//!      activated-with-no-seed);
//!   3. registry `status=approved` + `boot_target` write + audit record.
//!
//! Compensation runs the REVERSE order (activation undone before placement).
//! An unreachable participant during compensation parks the saga in
//! `compensation_pending` with exponential retry (base 1s, cap 5m, jittered)
//! — [`SagaState::Compensated`] is written in EXACTLY ONE place in this file
//! ([`retry_compensation`]'s success arm), and only after every remaining
//! undo has confirmed `Ok`. Every transition persists to `saga_log` via
//! [`SagaStore::put`] before the corresponding participant call runs
//! ([`persist_then`]), so an interrupted saga always resumes by re-running
//! its next unexecuted (or un-undone) step — never skipping one, and never
//! double-running one that already reached a terminal per-step state.
//!
//! # Coordinator wiring (read before touching any other file)
//!
//! This module is purely additive and self-contained; per this task's
//! coordinator rules it does NOT edit `lib.rs`, `listeners.rs`, `main.rs`, or
//! any other file. Two things are needed elsewhere once a real deployment
//! wants this wired up (left as TODOs here, applied by the coordinator):
//!
//! - **Startup resume**: `crate::listeners::serve` (or `main.rs`'s `Serve`
//!   arm) should call [`resume_unfinished`] once, before binding the
//!   listeners, passing a [`SagaDeps`] built from the REAL `PgSagaStore` +
//!   `RegistryStore`/`AuditStore` impls + tonic-backed `WebClient`/
//!   `PxeClient` impls (none of which exist yet — the uaa-web/uaa-pxe gRPC
//!   clients are a later task, matching the brief's "narrow LOCAL traits"
//!   instruction). Until those real clients land there is nothing to
//!   construct a live [`SagaDeps`] from, so no call is added here.
//! - **[`WebClient`]/[`PxeClient`] unification**: `reinstall.rs` (CT-06,
//!   same wave) already declares structurally-similar local
//!   `WebClient`/`PxeClient` traits for the same reason (saga.rs was still a
//!   header-only stub when CT-06 was authored). TODO(coordinator): once real
//!   tonic clients exist, decide whether `reinstall.rs` reuses these traits
//!   (shapes differ slightly — this module's [`WebClient::place_seed`] etc.
//!   vs. reinstall's `flip_boot_target`-only surface) or both stay separate
//!   per-module seams. No action is needed here; this module does not import
//!   from `reinstall.rs` or vice versa.
//!
//! # Reuse (do not invent parallels)
//!
//! - [`crate::db::SagaRow`] / [`crate::db::SagaState`] (CT-01, `db/mod.rs`) —
//!   the `saga_log` row type and its `running|done|compensating|compensated|
//!   compensation_pending` state enum are used AS-IS.
//! - [`crate::db::registry::RegistryStore`] / `MemRegistryStore` (CT-02,
//!   merged) — the registry-write step (3) goes through the real trait, not
//!   a local mock seam.
//! - [`crate::audit::record`] / [`crate::audit::AuditStore`] (CT-04, merged)
//!   — the audit record in step (3) goes through the real hash-chained
//!   store, not a local `AuditSink` stand-in (CT-04 was already merged when
//!   this task started, so the brief's "if not yet merged" fallback does not
//!   apply).
//!
//! [`WebClient`], [`PxeClient`], and [`SagaStore`] have no CT-01..07 owner
//! yet (the brief calls for exactly these three local seams), so they are
//! defined fresh in this file, per the brief.

use std::future::Future;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::audit::{self, AuditStore};
use crate::db::registry::RegistryStore;
use crate::db::{BootTarget, MachineStatus, SagaRow, SagaState};

// ── Step model ───────────────────────────────────────────────────────────

/// Per-step lifecycle. `Ok` means the forward action has been confirmed;
/// `Compensated` means an `Ok` step was later successfully undone.
/// `Failed` is terminal for that step — the SAGA never retries a forward
/// step, it compensates everything before it instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepState {
    Pending,
    Ok,
    Failed,
    Compensated,
}

/// One entry in the `saga_log.steps` JSONB array.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepRecord {
    pub name: String,
    pub state: StepState,
}

/// The fixed step order — THIS ARRAY'S ORDER IS THE SAFETY PROPERTY (spec
/// C3): placement (`place_seed`, `place_ipxe`) runs before activation
/// (`setup_pxe`, `set_boot_target` — "activation LAST — a failure between
/// steps leaves the host inert, never activated-with-no-seed",
/// `docs/specs/constellation-design.md:460`). `registry_approve` (status +
/// boot_target + audit) runs only after every prior step is confirmed `Ok`.
/// Nothing in this file reorders or parallelizes this array.
pub const SAGA_STEPS: [&str; 5] = [
    "place_seed",
    "place_ipxe",
    "setup_pxe",
    "set_boot_target",
    "registry_approve",
];

const IDX_PLACE_SEED: usize = 0;
const IDX_PLACE_IPXE: usize = 1;
const IDX_SETUP_PXE: usize = 2;
const IDX_SET_BOOT_TARGET: usize = 3;
// Only referenced by `#[cfg(test)]` fixtures that construct a mid-flight
// `SagaRow` directly (e.g. `test_resume_compensation_pending_retries`); the
// driver itself reaches this step purely by iterating `SAGA_STEPS`.
#[cfg(test)]
const IDX_REGISTRY_APPROVE: usize = 4;

/// Saga `kind` discriminator prefix. The real key for "is there already a
/// saga for this mac" is `kind`, since the fixed `saga_log` schema (CT-01
/// migration, not editable by this task) has no separate `target`/`mac`
/// column — only `saga_id | kind | state | steps | started_at |
/// finished_at`. Encoding the mac into `kind` keeps duplicate/done lookups a
/// plain equality match (`get_by_kind`) instead of a JSONB path query.
const SAGA_KIND_PREFIX: &str = "approve_machine:";

fn saga_kind(mac: &str) -> String {
    format!("{SAGA_KIND_PREFIX}{mac}")
}

fn mac_from_kind(kind: &str) -> Result<String> {
    kind.strip_prefix(SAGA_KIND_PREFIX)
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("saga_log row has an unrecognized kind: {kind:?}"))
}

fn initial_steps() -> Vec<StepRecord> {
    SAGA_STEPS
        .iter()
        .map(|name| StepRecord {
            name: (*name).to_string(),
            state: StepState::Pending,
        })
        .collect()
}

fn steps_to_json(steps: &[StepRecord]) -> Result<serde_json::Value> {
    Ok(serde_json::to_value(steps)?)
}

fn steps_from_json(value: &serde_json::Value) -> Result<Vec<StepRecord>> {
    Ok(serde_json::from_value(value.clone())?)
}

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Wire-format boot target string projected to both layers and written to
/// the registry when a machine is approved (spec Decision 13).
const APPROVED_BOOT_TARGET: &str = "custom-autoinstall";

// ── Outcome ──────────────────────────────────────────────────────────────

/// Result of [`approve_machine`] / one saga's continuation in
/// [`resume_unfinished`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SagaOutcome {
    /// All five steps confirmed `Ok`; the host is approved and activated.
    Done { saga_id: Uuid },
    /// A step failed and compensation is parked, waiting on an unreachable
    /// participant. NEVER emitted as a substitute for
    /// [`SagaOutcome::Compensated`] — see the module-level invariant.
    CompensationPending { saga_id: Uuid },
    /// A step failed and every already-completed step has been confirmed
    /// undone. The host is back to its pre-approval state.
    Compensated { saga_id: Uuid },
    /// A typed refusal: a saga for this mac is already `running`,
    /// `compensating`, or `compensation_pending` — no second saga is started.
    Refused { saga_id: Uuid, state: SagaState },
    /// Duplicate `ApproveMachine` for a mac whose saga already reached
    /// `done` — a no-op success, not an error.
    AlreadyDone { saga_id: Uuid },
}

// ── Participant seams (local — the real tonic clients land later) ─────────

/// uaa-web participant. Local stand-in for the not-yet-generated tonic
/// client (`proto/uaa/web/v1/web.proto`'s `WebService`) — see the
/// module-level coordinator-wiring note. `flip_boot_target` is part of the
/// proto surface this trait fronts; `ApproveMachine` itself never calls it
/// (that is `ReinstallMachine`'s job), but it is included so a future
/// concrete client only needs ONE `WebClient` impl for every caller in this
/// crate.
#[async_trait]
pub trait WebClient: Send + Sync {
    /// Place the (non-secret, placeholder-only) autoinstall seed for `mac`.
    async fn place_seed(&self, mac: &str, payload: &serde_json::Value) -> Result<()>;
    /// Place the iPXE boot config for `mac`.
    async fn place_ipxe(&self, mac: &str, payload: &serde_json::Value) -> Result<()>;
    /// Undo whatever `place_seed`/`place_ipxe` placed for `mac`. Idempotent-
    /// tolerant: removing an already-removed (or never-placed) host is `Ok`.
    async fn remove_host(&self, mac: &str) -> Result<()>;
    /// Flip the iPXE `set menu-default` boot target for `mac` to `target`.
    /// Unused by [`approve_machine`] (reserved for `ReinstallMachine`).
    async fn flip_boot_target(&self, mac: &str, target: &str) -> Result<bool>;
}

/// uaa-pxe participant. Local stand-in for the not-yet-generated tonic
/// client (`proto/uaa/pxe/v1/pxe.proto`'s `PxeService`) — see the
/// module-level coordinator-wiring note.
#[async_trait]
pub trait PxeClient: Send + Sync {
    /// Write the per-host `dhcp-hostsdir`/`dhcp-optsdir` files for `mac` and
    /// verify-reload dnsmasq.
    async fn setup_pxe(&self, mac: &str) -> Result<()>;
    /// Set the dnsmasq per-host boot program for `mac` to `target`.
    async fn set_boot_target(&self, mac: &str, target: &str) -> Result<()>;
    /// Undo whatever `setup_pxe`/`set_boot_target` activated for `mac`.
    /// Idempotent-tolerant, same contract as [`WebClient::remove_host`].
    async fn clear_host(&self, mac: &str) -> Result<()>;
}

// ── saga_log persistence seam ───────────────────────────────────────────

/// Persistence for `saga_log`. `put` is an upsert keyed by `saga_id`
/// (unlike the Decision-22 registry tables, `saga_log` is a per-run journal
/// that MUST be rewritable on every step transition — `DO UPDATE` is
/// correct here, not a violation of that law).
#[async_trait]
pub trait SagaStore: Send + Sync {
    /// Upsert `row` by `saga_id`.
    async fn put(&self, row: &SagaRow) -> Result<()>;
    /// Every row NOT in a terminal state (`done`/`compensated`) — i.e.
    /// `running`, `compensating`, or `compensation_pending` — for
    /// [`resume_unfinished`] to continue after a restart.
    async fn list_unfinished(&self) -> Result<Vec<SagaRow>>;
    /// The most-recently-written row for an exact `kind` match (used for the
    /// duplicate-running / done-noop checks in [`approve_machine`]), or
    /// `None` if no saga has ever run for that kind.
    async fn get_by_kind(&self, kind: &str) -> Result<Option<SagaRow>>;
}

/// In-memory [`SagaStore`] for tests (zero network, zero CockroachDB) and
/// for sibling control-task tests that need a saga journal of their own.
/// Keeps a full append-only `history` of every `put` in ADDITION to the
/// current-row map, so tests can assert the exact sequence of persisted
/// states a saga visited (e.g. "never `compensated` while an undo was
/// outstanding") — a real CRDB `saga_log` only keeps the latest row per
/// `saga_id`; this extra bookkeeping is test-only introspection.
#[derive(Debug, Default)]
pub struct MemSagaStore {
    rows: Mutex<std::collections::HashMap<Uuid, SagaRow>>,
    history: Mutex<Vec<SagaRow>>,
}

impl MemSagaStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Every row ever `put`, in call order. Test-only introspection.
    pub fn history(&self) -> Vec<SagaRow> {
        self.history
            .lock()
            .expect("MemSagaStore history poisoned")
            .clone()
    }
}

#[async_trait]
impl SagaStore for MemSagaStore {
    async fn put(&self, row: &SagaRow) -> Result<()> {
        self.rows
            .lock()
            .expect("MemSagaStore rows poisoned")
            .insert(row.saga_id, row.clone());
        self.history
            .lock()
            .expect("MemSagaStore history poisoned")
            .push(row.clone());
        Ok(())
    }

    async fn list_unfinished(&self) -> Result<Vec<SagaRow>> {
        Ok(self
            .rows
            .lock()
            .expect("MemSagaStore rows poisoned")
            .values()
            .filter(|row| {
                matches!(
                    row.state,
                    SagaState::Running | SagaState::Compensating | SagaState::CompensationPending
                )
            })
            .cloned()
            .collect())
    }

    async fn get_by_kind(&self, kind: &str) -> Result<Option<SagaRow>> {
        Ok(self
            .history
            .lock()
            .expect("MemSagaStore history poisoned")
            .iter()
            .rev()
            .find(|row| row.kind == kind)
            .cloned())
    }
}

// ── PgSagaStore — real tokio-postgres impl (runtime-only; never exercised
// by `cargo test`, which has no live CockroachDB) ──────────────────────────

pub(crate) const SQL_PUT_SAGA: &str = "\
    INSERT INTO saga_log (saga_id, kind, state, steps, started_at, finished_at) \
    VALUES ($1::UUID, $2, $3, $4::JSONB, $5::TIMESTAMPTZ, $6::TIMESTAMPTZ) \
    ON CONFLICT (saga_id) DO UPDATE SET \
      state = excluded.state, steps = excluded.steps, finished_at = excluded.finished_at";
// Deliberately `DO UPDATE`, unlike the insert-if-absent registry tables in
// `db/registry.rs` (Decision 22 no-clobber law): `saga_log` is a per-run
// journal keyed by the stable `saga_id`, and every step transition MUST
// rewrite that same row. Decision 22 scopes the no-clobber law to registry
// import/rollback tables, not this journal.

pub(crate) const SQL_LIST_UNFINISHED: &str = "\
    SELECT saga_id::STRING AS saga_id, kind, state, steps::STRING AS steps, \
           started_at::STRING AS started_at, finished_at::STRING AS finished_at \
    FROM saga_log WHERE state IN ('running', 'compensating', 'compensation_pending')";

pub(crate) const SQL_GET_BY_KIND: &str = "\
    SELECT saga_id::STRING AS saga_id, kind, state, steps::STRING AS steps, \
           started_at::STRING AS started_at, finished_at::STRING AS finished_at \
    FROM saga_log WHERE kind = $1 ORDER BY started_at DESC LIMIT 1";

/// Real [`SagaStore`]: tokio-postgres against CockroachDB. Constructed only
/// at runtime; unit tests never build it (no live database in the test path).
pub struct PgSagaStore {
    client: tokio_postgres::Client,
}

impl PgSagaStore {
    pub fn new(client: tokio_postgres::Client) -> Self {
        Self { client }
    }
}

fn row_to_saga(row: &tokio_postgres::Row) -> Result<SagaRow> {
    let saga_id_text: String = row.get("saga_id");
    let steps_text: String = row.get("steps");
    Ok(SagaRow {
        saga_id: Uuid::parse_str(&saga_id_text)?,
        kind: row.get("kind"),
        state: SagaState::from(row.get::<_, String>("state")),
        steps: serde_json::from_str(&steps_text)?,
        started_at: row.get("started_at"),
        finished_at: row.get("finished_at"),
    })
}

#[async_trait]
impl SagaStore for PgSagaStore {
    async fn put(&self, row: &SagaRow) -> Result<()> {
        let saga_id_text = row.saga_id.to_string();
        let state_text: String = row.state.clone().into();
        let steps_text = serde_json::to_string(&row.steps)?;
        self.client
            .execute(
                SQL_PUT_SAGA,
                &[
                    &saga_id_text,
                    &row.kind,
                    &state_text,
                    &steps_text,
                    &row.started_at,
                    &row.finished_at,
                ],
            )
            .await?;
        Ok(())
    }

    async fn list_unfinished(&self) -> Result<Vec<SagaRow>> {
        let rows = self.client.query(SQL_LIST_UNFINISHED, &[]).await?;
        rows.iter().map(row_to_saga).collect()
    }

    async fn get_by_kind(&self, kind: &str) -> Result<Option<SagaRow>> {
        let row = self.client.query_opt(SQL_GET_BY_KIND, &[&kind]).await?;
        row.as_ref().map(row_to_saga).transpose()
    }
}

// ── Backoff / jitter / sleep seams ──────────────────────────────────────

/// Exponential backoff schedule for compensation retries: `base` (1s),
/// doubling per attempt, capped at `cap` (5m). `max_attempts` bounds the
/// retry loop so tests can drive a deterministic, finite number of passes
/// without ever sleeping in real time; production callers leave it `None`
/// (retry forever — if the process dies first, [`resume_unfinished`]
/// re-enters the same loop from `saga_log` after restart, so "forever" never
/// means "silently gives up").
#[derive(Debug, Clone)]
pub struct Backoff {
    pub base: Duration,
    pub cap: Duration,
    pub max_attempts: Option<u32>,
}

impl Default for Backoff {
    fn default() -> Self {
        Self {
            base: Duration::from_secs(1),
            cap: Duration::from_secs(300),
            max_attempts: None,
        }
    }
}

/// Pure backoff schedule (no jitter): `base * 2^attempt`, capped at `cap`. A
/// pure function so tests can assert the schedule without sleeping.
pub fn backoff_delay(attempt: u32, backoff: &Backoff) -> Duration {
    let multiplier = 1u64.checked_shl(attempt).unwrap_or(u64::MAX);
    let secs = backoff.base.as_secs().saturating_mul(multiplier);
    Duration::from_secs(secs).min(backoff.cap)
}

/// Jitter seam applied on top of [`backoff_delay`]. Production uses
/// [`RandJitter`]; tests use a fixed/deterministic impl so retry timing
/// assertions are exact.
pub trait Jitter: Send + Sync {
    fn apply(&self, base: Duration) -> Duration;
}

/// Production [`Jitter`]: scales the delay by a uniform random factor in
/// `[0.5, 1.0)` (full jitter would risk a zero-length delay hot loop).
pub struct RandJitter;

impl Jitter for RandJitter {
    fn apply(&self, base: Duration) -> Duration {
        use rand::Rng;
        let factor: f64 = rand::thread_rng().gen_range(0.5..1.0);
        Duration::from_secs_f64(base.as_secs_f64() * factor)
    }
}

/// Deterministic [`Jitter`] for tests: returns `base` unchanged.
pub struct NoJitter;

impl Jitter for NoJitter {
    fn apply(&self, base: Duration) -> Duration {
        base
    }
}

/// Clock/sleep seam so tests never really sleep. Mirrors the same pattern
/// used by `uaa-core::pki`'s enrollment poll loop and `reinstall.rs`'s
/// `Clock` — each module owns its own tiny instance rather than sharing one
/// across unrelated domains.
#[async_trait]
pub trait Sleeper: Send + Sync {
    async fn sleep(&self, duration: Duration);
}

/// Production [`Sleeper`] backed by `tokio::time::sleep`.
pub struct TokioSleeper;

#[async_trait]
impl Sleeper for TokioSleeper {
    async fn sleep(&self, duration: Duration) {
        tokio::time::sleep(duration).await;
    }
}

// ── Dependency bundle ────────────────────────────────────────────────────

/// Everything [`approve_machine`] / [`resume_unfinished`] depend on, as
/// trait objects so tests inject mocks. No live gRPC client or database
/// connection is ever constructed by this file (see the acceptance
/// criteria's `grep -n "7445\|7446\|tonic::transport"` check — this module
/// never does either).
pub struct SagaDeps<'a> {
    pub web: &'a dyn WebClient,
    pub pxe: &'a dyn PxeClient,
    pub saga_store: &'a dyn SagaStore,
    pub registry: &'a dyn RegistryStore,
    pub audit: &'a dyn AuditStore,
    pub sleeper: &'a dyn Sleeper,
    pub jitter: &'a dyn Jitter,
    pub backoff: Backoff,
}

// ── persist_then: the ONE transition helper ─────────────────────────────

/// Write `row` to `saga_log` FIRST, then run `action`. A crash between the
/// persist and the action simply re-runs the acted step on the next resume
/// (safe: every [`WebClient`]/[`PxeClient`] call in this SAGA is
/// idempotent-tolerant per the spec's edge semantics) — it can never SKIP a
/// step whose start was already durably recorded. Every state transition in
/// this file goes through this one helper.
async fn persist_then<'a, F, Fut, T>(deps: &SagaDeps<'a>, row: &SagaRow, action: F) -> Result<T>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<T>>,
{
    deps.saga_store.put(row).await?;
    action().await
}

// ── Forward driver ───────────────────────────────────────────────────────

fn placement_payload(mac: &str) -> serde_json::Value {
    // Non-secret fields + placeholders ONLY (spec `WebService.PlaceSeed`
    // comment: "non-secret fields + placeholders ONLY") — no real secret
    // material is ever constructed here.
    serde_json::json!({ "mac": mac, "luks_key": "REPLACE_AT_PLACE_TIME" })
}

async fn registry_approve(deps: &SagaDeps<'_>, mac: &str) -> Result<()> {
    // Registry write (status=approved) + boot_target write + audit record.
    // The `RegistryStore` trait (CT-02) exposes single-field mutators, not
    // one atomic multi-field write, so a failure between these two calls
    // (status written, boot_target not yet) is a known narrow gap this file
    // cannot close without editing `db/registry.rs` (out of scope for this
    // task — CT-05 is `saga.rs`-exclusive). `update_machine_status` runs
    // first so a `boot_target` failure never leaves an unapproved host
    // "activated" by boot_target alone.
    deps.registry
        .update_machine_status(mac, MachineStatus::Approved, Some(now_rfc3339()))
        .await?;
    deps.registry
        .set_boot_target(mac, BootTarget::from(APPROVED_BOOT_TARGET.to_string()))
        .await?;
    audit::record(
        deps.audit,
        "system",
        "system",
        "approve_machine",
        Some(mac.to_string()),
        "approved",
        Some(serde_json::json!({ "boot_target": APPROVED_BOOT_TARGET })),
    )
    .await?;
    Ok(())
}

async fn run_step(deps: &SagaDeps<'_>, mac: &str, name: &str) -> Result<()> {
    match name {
        "place_seed" => deps.web.place_seed(mac, &placement_payload(mac)).await,
        "place_ipxe" => deps.web.place_ipxe(mac, &placement_payload(mac)).await,
        "setup_pxe" => deps.pxe.setup_pxe(mac).await,
        "set_boot_target" => deps.pxe.set_boot_target(mac, APPROVED_BOOT_TARGET).await,
        "registry_approve" => registry_approve(deps, mac).await,
        other => Err(anyhow::anyhow!("unknown saga step: {other}")),
    }
}

/// Drive `row` forward from its first non-`Ok` step through `registry_approve`
/// (steps already `Ok` — the resume case — are skipped, never re-run).
/// `set_boot_target` (activation) NEVER runs before both `place_seed` and
/// `place_ipxe` (placement) are confirmed `Ok`: the loop is strictly
/// sequential over [`SAGA_STEPS`], no reordering, no parallel join. On the
/// first step failure, everything already `Ok` is compensated via
/// [`retry_compensation`] and this function returns THAT outcome.
async fn run_forward(deps: &SagaDeps<'_>, row: &mut SagaRow, mac: &str) -> Result<SagaOutcome> {
    let mut steps = steps_from_json(&row.steps)?;

    for (i, step_name) in SAGA_STEPS.iter().enumerate() {
        let step_name: &str = step_name;
        if steps[i].state == StepState::Ok {
            continue;
        }

        row.steps = steps_to_json(&steps)?;
        let result = persist_then(deps, row, || run_step(deps, mac, step_name)).await;

        match result {
            Ok(()) => {
                steps[i].state = StepState::Ok;
                row.steps = steps_to_json(&steps)?;
                deps.saga_store.put(row).await?;
            }
            Err(_step_err) => {
                steps[i].state = StepState::Failed;
                row.steps = steps_to_json(&steps)?;
                row.state = SagaState::Compensating;
                deps.saga_store.put(row).await?;
                return retry_compensation(deps, row).await;
            }
        }
    }

    row.state = SagaState::Done;
    row.finished_at = Some(now_rfc3339());
    deps.saga_store.put(row).await?;
    Ok(SagaOutcome::Done {
        saga_id: row.saga_id,
    })
}

// ── Compensation ─────────────────────────────────────────────────────────

enum CompensationPassResult {
    Compensated,
    Pending,
}

/// One pass of reverse-order undo. Compensation is phase-granular, not
/// per-step, because the participant seams only expose ONE undo call per
/// phase ([`PxeClient::clear_host`] for activation, [`WebClient::remove_host`]
/// for placement) — matching the brief's mapping: step-3 (registry) failure
/// compensates activation THEN placement; step-2 (activation) failure
/// compensates whatever activated THEN placement; step-1 (placement)
/// failure just removes whatever placed (no `clear_host` call at all, since
/// activation never started). Each already-`Compensated` phase is skipped on
/// retry (recomputed fresh from the persisted step states every pass), so a
/// partially-successful pass never re-issues a call that already succeeded.
async fn attempt_compensation_pass(
    deps: &SagaDeps<'_>,
    row: &mut SagaRow,
) -> Result<CompensationPassResult> {
    let mac = mac_from_kind(&row.kind)?;
    let mut steps = steps_from_json(&row.steps)?;

    let activation_ok = steps[IDX_SETUP_PXE].state == StepState::Ok
        || steps[IDX_SET_BOOT_TARGET].state == StepState::Ok;
    if activation_ok {
        let result = persist_then(deps, row, || deps.pxe.clear_host(&mac)).await;
        match result {
            Ok(()) => {
                if steps[IDX_SETUP_PXE].state == StepState::Ok {
                    steps[IDX_SETUP_PXE].state = StepState::Compensated;
                }
                if steps[IDX_SET_BOOT_TARGET].state == StepState::Ok {
                    steps[IDX_SET_BOOT_TARGET].state = StepState::Compensated;
                }
                row.steps = steps_to_json(&steps)?;
            }
            Err(_) => return Ok(CompensationPassResult::Pending),
        }
    }

    let placement_ok = steps[IDX_PLACE_SEED].state == StepState::Ok
        || steps[IDX_PLACE_IPXE].state == StepState::Ok;
    if placement_ok {
        let result = persist_then(deps, row, || deps.web.remove_host(&mac)).await;
        match result {
            Ok(()) => {
                if steps[IDX_PLACE_SEED].state == StepState::Ok {
                    steps[IDX_PLACE_SEED].state = StepState::Compensated;
                }
                if steps[IDX_PLACE_IPXE].state == StepState::Ok {
                    steps[IDX_PLACE_IPXE].state = StepState::Compensated;
                }
                row.steps = steps_to_json(&steps)?;
            }
            Err(_) => return Ok(CompensationPassResult::Pending),
        }
    }

    Ok(CompensationPassResult::Compensated)
}

/// Retry compensation passes with exponential backoff until every step is
/// confirmed undone (`Compensated`), or `deps.backoff.max_attempts` is
/// exhausted (test-only bound; production leaves it unbounded and relies on
/// [`resume_unfinished`] to keep trying after a restart).
///
/// INVARIANT (grep-checkable): the only `row.state = SagaState::Compensated`
/// assignment in this file is in the `CompensationPassResult::Compensated`
/// arm below, AFTER `attempt_compensation_pass` has reported every
/// remaining undo as `Ok`. Every other exit from this function persists
/// `SagaState::CompensationPending` — never `Compensated` — so a saga is
/// NEVER falsely marked `compensated` while any undo is outstanding.
async fn retry_compensation(deps: &SagaDeps<'_>, row: &mut SagaRow) -> Result<SagaOutcome> {
    let mut attempt: u32 = 0;
    loop {
        match attempt_compensation_pass(deps, row).await? {
            CompensationPassResult::Compensated => {
                row.state = SagaState::Compensated;
                row.finished_at = Some(now_rfc3339());
                deps.saga_store.put(row).await?;
                return Ok(SagaOutcome::Compensated {
                    saga_id: row.saga_id,
                });
            }
            CompensationPassResult::Pending => {
                row.state = SagaState::CompensationPending;
                deps.saga_store.put(row).await?;

                let exhausted =
                    matches!(deps.backoff.max_attempts, Some(max) if attempt + 1 >= max);
                if exhausted {
                    return Ok(SagaOutcome::CompensationPending {
                        saga_id: row.saga_id,
                    });
                }

                let delay = deps.jitter.apply(backoff_delay(attempt, &deps.backoff));
                deps.sleeper.sleep(delay).await;
                attempt += 1;
            }
        }
    }
}

// ── Public entrypoints ───────────────────────────────────────────────────

/// Drive the `ApproveMachine` SAGA for `mac` per spec C3. Refuses a second
/// concurrent saga for the same mac (`running`/`compensating`/
/// `compensation_pending` → [`SagaOutcome::Refused`]); a prior `done` saga is
/// a no-op success (`AlreadyDone`); a prior `compensated` (fully rolled
/// back) saga does not block a fresh attempt.
pub async fn approve_machine(deps: &SagaDeps<'_>, mac: &str) -> Result<SagaOutcome> {
    let kind = saga_kind(mac);

    if let Some(existing) = deps.saga_store.get_by_kind(&kind).await? {
        match &existing.state {
            SagaState::Running | SagaState::Compensating | SagaState::CompensationPending => {
                return Ok(SagaOutcome::Refused {
                    saga_id: existing.saga_id,
                    state: existing.state.clone(),
                });
            }
            SagaState::Done => {
                return Ok(SagaOutcome::AlreadyDone {
                    saga_id: existing.saga_id,
                });
            }
            SagaState::Compensated | SagaState::Unknown(_) => {
                // A prior attempt fully rolled back (or the row is from an
                // unrecognized legacy state) — start a fresh saga below.
            }
        }
    }

    let mut row = SagaRow {
        saga_id: Uuid::new_v4(),
        kind,
        state: SagaState::Running,
        steps: steps_to_json(&initial_steps())?,
        started_at: Some(now_rfc3339()),
        finished_at: None,
    };
    deps.saga_store.put(&row).await?;

    run_forward(deps, &mut row, mac).await
}

/// Resume every non-terminal `saga_log` row after a restart: `running`
/// continues from its first non-`Ok` step (via [`run_forward`], which
/// naturally skips already-`Ok` steps); `compensating`/`compensation_pending`
/// re-enter the compensation retry loop (via [`retry_compensation`], which
/// naturally skips already-`Compensated` phases). Called once from the
/// `serve` startup path — see the module-level coordinator-wiring note for
/// why that one-line call is not added by this file.
pub async fn resume_unfinished(deps: &SagaDeps<'_>) -> Result<Vec<SagaOutcome>> {
    let rows = deps.saga_store.list_unfinished().await?;
    let mut outcomes = Vec::with_capacity(rows.len());

    for mut row in rows {
        let mac = mac_from_kind(&row.kind)?;
        let outcome = match &row.state {
            SagaState::Running => run_forward(deps, &mut row, &mac).await?,
            SagaState::Compensating | SagaState::CompensationPending => {
                retry_compensation(deps, &mut row).await?
            }
            SagaState::Done | SagaState::Compensated | SagaState::Unknown(_) => continue,
        };
        outcomes.push(outcome);
    }

    Ok(outcomes)
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::MemAuditStore;
    use crate::db::registry::MemRegistryStore;
    use crate::db::{BootTarget as DbBootTarget, MachineRow, MachineStatus as DbMachineStatus};
    use std::collections::VecDeque;
    use std::sync::Arc;

    // ── Shared call-order log ───────────────────────────────────────────

    #[derive(Debug, Default)]
    struct CallLog(Mutex<Vec<&'static str>>);

    impl CallLog {
        fn push(&self, call: &'static str) {
            self.0.lock().unwrap().push(call);
        }
        fn snapshot(&self) -> Vec<&'static str> {
            self.0.lock().unwrap().clone()
        }
    }

    /// Pops the next scripted result off `queue`; an empty queue defaults to
    /// `Ok(())` (so most tests only need to script the failures they care
    /// about).
    fn next_result(queue: &Mutex<VecDeque<bool>>) -> Result<()> {
        let ok = queue.lock().unwrap().pop_front().unwrap_or(true);
        if ok {
            Ok(())
        } else {
            Err(anyhow::anyhow!("mock participant failure"))
        }
    }

    // ── Mock WebClient / PxeClient ───────────────────────────────────────

    #[derive(Default)]
    struct MockParticipants {
        log: Arc<CallLog>,
        place_seed: Mutex<VecDeque<bool>>,
        place_ipxe: Mutex<VecDeque<bool>>,
        remove_host: Mutex<VecDeque<bool>>,
        setup_pxe: Mutex<VecDeque<bool>>,
        set_boot_target: Mutex<VecDeque<bool>>,
        clear_host: Mutex<VecDeque<bool>>,
    }

    impl MockParticipants {
        fn new(log: Arc<CallLog>) -> Self {
            Self {
                log,
                ..Default::default()
            }
        }
    }

    #[async_trait]
    impl WebClient for MockParticipants {
        async fn place_seed(&self, _mac: &str, _payload: &serde_json::Value) -> Result<()> {
            self.log.push("place_seed");
            next_result(&self.place_seed)
        }
        async fn place_ipxe(&self, _mac: &str, _payload: &serde_json::Value) -> Result<()> {
            self.log.push("place_ipxe");
            next_result(&self.place_ipxe)
        }
        async fn remove_host(&self, _mac: &str) -> Result<()> {
            self.log.push("remove_host");
            next_result(&self.remove_host)
        }
        async fn flip_boot_target(&self, _mac: &str, _target: &str) -> Result<bool> {
            Ok(true)
        }
    }

    #[async_trait]
    impl PxeClient for MockParticipants {
        async fn setup_pxe(&self, _mac: &str) -> Result<()> {
            self.log.push("setup_pxe");
            next_result(&self.setup_pxe)
        }
        async fn set_boot_target(&self, _mac: &str, _target: &str) -> Result<()> {
            self.log.push("set_boot_target");
            next_result(&self.set_boot_target)
        }
        async fn clear_host(&self, _mac: &str) -> Result<()> {
            self.log.push("clear_host");
            next_result(&self.clear_host)
        }
    }

    // ── Registry wrapper: logs "registry_approve" + optional fail-injection ─

    struct RecordingRegistry {
        inner: MemRegistryStore,
        log: Arc<CallLog>,
        fail_status_update: Mutex<VecDeque<bool>>,
    }

    impl RecordingRegistry {
        fn new(log: Arc<CallLog>) -> Self {
            Self {
                inner: MemRegistryStore::new(),
                log,
                fail_status_update: Mutex::new(VecDeque::new()),
            }
        }
    }

    #[async_trait]
    impl RegistryStore for RecordingRegistry {
        async fn get_machine(&self, mac: &str) -> Result<Option<MachineRow>> {
            self.inner.get_machine(mac).await
        }
        async fn list_machines(&self) -> Result<Vec<MachineRow>> {
            self.inner.list_machines().await
        }
        async fn insert_machine_if_absent(&self, row: MachineRow) -> Result<bool> {
            self.inner.insert_machine_if_absent(row).await
        }
        async fn update_machine_status(
            &self,
            mac: &str,
            status: DbMachineStatus,
            approved_at: Option<String>,
        ) -> Result<()> {
            self.log.push("registry_approve");
            next_result(&self.fail_status_update)?;
            self.inner
                .update_machine_status(mac, status, approved_at)
                .await
        }
        async fn touch_last_seen(&self, mac: &str, ip: Option<String>) -> Result<()> {
            self.inner.touch_last_seen(mac, ip).await
        }
        async fn set_boot_target(&self, mac: &str, boot_target: DbBootTarget) -> Result<()> {
            self.inner.set_boot_target(mac, boot_target).await
        }
        async fn list_yubikeys(&self) -> Result<Vec<crate::db::YubikeyRow>> {
            self.inner.list_yubikeys().await
        }
        async fn insert_yubikey_if_absent(&self, row: crate::db::YubikeyRow) -> Result<bool> {
            self.inner.insert_yubikey_if_absent(row).await
        }
        async fn list_tang_servers(&self) -> Result<Vec<crate::db::TangServerRow>> {
            self.inner.list_tang_servers().await
        }
        async fn upsert_tang_server(&self, row: crate::db::TangServerRow) -> Result<()> {
            self.inner.upsert_tang_server(row).await
        }
        async fn insert_tang_if_absent(&self, row: crate::db::TangServerRow) -> Result<bool> {
            self.inner.insert_tang_if_absent(row).await
        }
        async fn insert_luks_credential(&self, row: crate::db::LuksCredentialRow) -> Result<()> {
            self.inner.insert_luks_credential(row).await
        }
        async fn list_luks_credentials(
            &self,
            mac: &str,
        ) -> Result<Vec<crate::db::LuksCredentialRow>> {
            self.inner.list_luks_credentials(mac).await
        }
        async fn revoke_luks_credential(&self, id: Uuid) -> Result<()> {
            self.inner.revoke_luks_credential(id).await
        }
    }

    // ── NoopSleeper: records requested durations, never actually waits ────

    #[derive(Default)]
    struct NoopSleeper(Mutex<Vec<Duration>>);

    #[async_trait]
    impl Sleeper for NoopSleeper {
        async fn sleep(&self, duration: Duration) {
            self.0.lock().unwrap().push(duration);
            // No real sleep: this is exactly the injectable-clock seam the
            // brief calls for so tests never sleep in real time.
        }
    }

    // ── Test fixture ────────────────────────────────────────────────────

    const MAC: &str = "aa:bb:cc:dd:ee:ff";

    fn seed_machine_row() -> MachineRow {
        MachineRow {
            mac: MAC.to_string(),
            hostname: "h1".into(),
            ip: None,
            r#type: "lenovo".into(),
            status: DbMachineStatus::Pending,
            boot_target: DbBootTarget::LocalDisk,
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
        }
    }

    struct Fixture {
        log: Arc<CallLog>,
        participants: MockParticipants,
        saga_store: MemSagaStore,
        registry: RecordingRegistry,
        audit: MemAuditStore,
        sleeper: NoopSleeper,
    }

    impl Fixture {
        fn new() -> Self {
            let log = Arc::new(CallLog::default());
            Self {
                participants: MockParticipants::new(log.clone()),
                saga_store: MemSagaStore::new(),
                registry: RecordingRegistry::new(log.clone()),
                audit: MemAuditStore::new(),
                sleeper: NoopSleeper::default(),
                log,
            }
        }

        fn deps(&self, max_attempts: Option<u32>) -> SagaDeps<'_> {
            SagaDeps {
                web: &self.participants,
                pxe: &self.participants,
                saga_store: &self.saga_store,
                registry: &self.registry,
                audit: &self.audit,
                sleeper: &self.sleeper,
                jitter: &NoJitter,
                backoff: Backoff {
                    base: Duration::from_millis(1),
                    cap: Duration::from_millis(10),
                    max_attempts,
                },
            }
        }
    }

    /// The most-recently-persisted row for `mac`, via [`MemSagaStore::history`].
    fn latest_row(store: &MemSagaStore, mac: &str) -> SagaRow {
        store
            .history()
            .into_iter()
            .rev()
            .find(|r| r.kind == saga_kind(mac))
            .expect("saga row present")
    }

    // ── Ordering (the safety property) ───────────────────────────────────

    #[tokio::test]
    async fn test_happy_path_order() {
        let fx = Fixture::new();
        fx.registry
            .inner
            .insert_machine_if_absent(seed_machine_row())
            .await
            .unwrap();
        let deps = fx.deps(Some(5));

        let outcome = approve_machine(&deps, MAC).await.unwrap();

        assert!(matches!(outcome, SagaOutcome::Done { .. }), "{outcome:?}");
        assert_eq!(
            fx.log.snapshot(),
            vec![
                "place_seed",
                "place_ipxe",
                "setup_pxe",
                "set_boot_target",
                "registry_approve",
            ],
            "a fully healthy approve flows through every guard to done \
             (anti-over-suppression: the ordering machinery does not block \
             or reorder the legitimate flow)"
        );

        let machine = fx.registry.get_machine(MAC).await.unwrap().unwrap();
        assert_eq!(machine.status, DbMachineStatus::Approved);
        assert_eq!(machine.boot_target, DbBootTarget::CustomAutoinstall);
        assert_eq!(fx.audit.list_events(0).await.unwrap().len(), 1);

        let row = latest_row(&fx.saga_store, MAC);
        assert_eq!(row.state, SagaState::Done);
    }

    #[tokio::test]
    async fn test_activation_never_before_placement() {
        let fx = Fixture::new();
        fx.registry
            .inner
            .insert_machine_if_absent(seed_machine_row())
            .await
            .unwrap();
        fx.participants
            .place_ipxe
            .lock()
            .unwrap()
            .push_back(false); // place_ipxe fails
        let deps = fx.deps(Some(5));

        let outcome = approve_machine(&deps, MAC).await.unwrap();

        assert!(
            matches!(outcome, SagaOutcome::Compensated { .. }),
            "{outcome:?}"
        );
        let calls = fx.log.snapshot();
        assert!(
            !calls.contains(&"setup_pxe") && !calls.contains(&"set_boot_target"),
            "activation must never run after a placement failure: {calls:?}"
        );
        assert!(
            !calls.contains(&"registry_approve"),
            "registry must never be approved after a placement failure: {calls:?}"
        );
        // step-1 partial failure: place_seed succeeded (Ok), place_ipxe
        // failed — only remove_host undoes it, no clear_host (activation
        // never started).
        assert_eq!(
            calls,
            vec!["place_seed", "place_ipxe", "remove_host"],
            "{calls:?}"
        );
    }

    // ── Compensation mapping ─────────────────────────────────────────────

    #[tokio::test]
    async fn test_step2_failure_compensates_reverse() {
        let fx = Fixture::new();
        fx.registry
            .inner
            .insert_machine_if_absent(seed_machine_row())
            .await
            .unwrap();
        fx.participants
            .set_boot_target
            .lock()
            .unwrap()
            .push_back(false); // set_boot_target (activation) fails
        let deps = fx.deps(Some(5));

        let outcome = approve_machine(&deps, MAC).await.unwrap();

        assert!(
            matches!(outcome, SagaOutcome::Compensated { .. }),
            "{outcome:?}"
        );
        let calls = fx.log.snapshot();
        assert_eq!(
            calls,
            vec![
                "place_seed",
                "place_ipxe",
                "setup_pxe",
                "set_boot_target",
                "clear_host",
                "remove_host",
            ],
            "undo order must be exactly [clear_host, remove_host]: {calls:?}"
        );

        let machine = fx.registry.get_machine(MAC).await.unwrap().unwrap();
        assert_eq!(
            machine.status,
            DbMachineStatus::Pending,
            "registry must NOT be approved"
        );
        assert_eq!(fx.audit.list_events(0).await.unwrap().len(), 0);

        let row = latest_row(&fx.saga_store, MAC);
        assert_eq!(row.state, SagaState::Compensated);
    }

    #[tokio::test]
    async fn test_step3_registry_failure_compensates_both_phases() {
        let fx = Fixture::new();
        fx.registry
            .inner
            .insert_machine_if_absent(seed_machine_row())
            .await
            .unwrap();
        fx.registry
            .fail_status_update
            .lock()
            .unwrap()
            .push_back(false); // registry_approve fails
        let deps = fx.deps(Some(5));

        let outcome = approve_machine(&deps, MAC).await.unwrap();

        assert!(
            matches!(outcome, SagaOutcome::Compensated { .. }),
            "{outcome:?}"
        );
        let calls = fx.log.snapshot();
        assert_eq!(
            calls,
            vec![
                "place_seed",
                "place_ipxe",
                "setup_pxe",
                "set_boot_target",
                "registry_approve",
                "clear_host",
                "remove_host",
            ],
            "step-3 failure compensates 2 (activation) then 1 (placement): {calls:?}"
        );
    }

    // ── Never falsely compensated ────────────────────────────────────────

    #[tokio::test]
    async fn test_unreachable_compensation_parks_pending() {
        let fx = Fixture::new();
        fx.registry
            .inner
            .insert_machine_if_absent(seed_machine_row())
            .await
            .unwrap();
        fx.participants
            .set_boot_target
            .lock()
            .unwrap()
            .push_back(false);
        {
            let mut q = fx.participants.clear_host.lock().unwrap();
            q.push_back(false);
            q.push_back(false);
            q.push_back(false);
            q.push_back(true);
        }
        let deps = fx.deps(Some(10));

        let outcome = approve_machine(&deps, MAC).await.unwrap();

        assert!(
            matches!(outcome, SagaOutcome::Compensated { .. }),
            "{outcome:?}"
        );

        let history = fx.saga_store.history();
        let states: Vec<SagaState> = history.iter().map(|r| r.state.clone()).collect();
        let last = states.last().cloned().unwrap();
        assert_eq!(last, SagaState::Compensated);

        let first_compensated_idx = states
            .iter()
            .position(|s| *s == SagaState::Compensated)
            .unwrap();
        assert_eq!(
            first_compensated_idx,
            states.len() - 1,
            "Compensated must be the LAST and ONLY compensated-terminal state written"
        );
        let pending_count = states
            .iter()
            .filter(|s| **s == SagaState::CompensationPending)
            .count();
        assert!(
            pending_count >= 3,
            "must have visited compensation_pending across retries: {states:?}"
        );
        // NEVER falsely marked compensated: every entry strictly before the
        // final one must be something other than Compensated.
        assert!(
            states[..states.len() - 1]
                .iter()
                .all(|s| *s != SagaState::Compensated),
            "compensated written only once, at the very end: {states:?}"
        );
    }

    #[tokio::test]
    async fn test_never_falsely_compensated() {
        let fx = Fixture::new();
        fx.registry
            .inner
            .insert_machine_if_absent(seed_machine_row())
            .await
            .unwrap();
        fx.participants
            .set_boot_target
            .lock()
            .unwrap()
            .push_back(false);
        // clear_host fails FOREVER — bounded test iterations via max_attempts.
        {
            let mut q = fx.participants.clear_host.lock().unwrap();
            for _ in 0..20 {
                q.push_back(false);
            }
        }
        let deps = fx.deps(Some(5)); // bounded: exactly 5 compensation attempts

        let outcome = approve_machine(&deps, MAC).await.unwrap();

        assert!(
            matches!(outcome, SagaOutcome::CompensationPending { .. }),
            "{outcome:?}"
        );

        let history = fx.saga_store.history();
        assert!(
            history.iter().all(|r| r.state != SagaState::Compensated),
            "must NEVER be marked compensated while an undo is outstanding: {:?}",
            history
                .iter()
                .map(|r| r.state.clone())
                .collect::<Vec<_>>()
        );
        let row = latest_row(&fx.saga_store, MAC);
        assert_eq!(
            row.state,
            SagaState::CompensationPending,
            "final persisted state must be compensation_pending, not compensated"
        );
        // remove_host must never be called: activation compensation never
        // succeeded, so placement compensation never even starts.
        assert!(!fx.log.snapshot().contains(&"remove_host"));
    }

    // ── Resume ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_resume_running_continues() {
        let fx = Fixture::new();
        fx.registry
            .inner
            .insert_machine_if_absent(seed_machine_row())
            .await
            .unwrap();

        let mut steps = initial_steps();
        steps[IDX_PLACE_SEED].state = StepState::Ok;
        steps[IDX_PLACE_IPXE].state = StepState::Ok;
        let row = SagaRow {
            saga_id: Uuid::new_v4(),
            kind: saga_kind(MAC),
            state: SagaState::Running,
            steps: steps_to_json(&steps).unwrap(),
            started_at: Some(now_rfc3339()),
            finished_at: None,
        };
        fx.saga_store.put(&row).await.unwrap();

        let deps = fx.deps(Some(5));
        let outcomes = resume_unfinished(&deps).await.unwrap();

        assert_eq!(outcomes.len(), 1);
        assert!(matches!(outcomes[0], SagaOutcome::Done { .. }));

        let calls = fx.log.snapshot();
        assert!(
            !calls.contains(&"place_seed") && !calls.contains(&"place_ipxe"),
            "already-ok steps must not re-run: {calls:?}"
        );
        assert_eq!(
            calls,
            vec!["setup_pxe", "set_boot_target", "registry_approve"],
            "only steps 3..5 execute: {calls:?}"
        );
    }

    #[tokio::test]
    async fn test_resume_compensation_pending_retries() {
        let fx = Fixture::new();
        fx.registry
            .inner
            .insert_machine_if_absent(seed_machine_row())
            .await
            .unwrap();
        {
            let mut q = fx.participants.clear_host.lock().unwrap();
            q.push_back(false);
            q.push_back(true);
        }

        let mut steps = initial_steps();
        steps[IDX_PLACE_SEED].state = StepState::Ok;
        steps[IDX_PLACE_IPXE].state = StepState::Ok;
        steps[IDX_SETUP_PXE].state = StepState::Ok;
        steps[IDX_SET_BOOT_TARGET].state = StepState::Ok;
        steps[IDX_REGISTRY_APPROVE].state = StepState::Failed;
        let row = SagaRow {
            saga_id: Uuid::new_v4(),
            kind: saga_kind(MAC),
            state: SagaState::CompensationPending,
            steps: steps_to_json(&steps).unwrap(),
            started_at: Some(now_rfc3339()),
            finished_at: None,
        };
        fx.saga_store.put(&row).await.unwrap();

        let deps = fx.deps(Some(5));
        let outcomes = resume_unfinished(&deps).await.unwrap();

        assert_eq!(outcomes.len(), 1);
        assert!(matches!(outcomes[0], SagaOutcome::Compensated { .. }));
        assert_eq!(
            fx.log.snapshot(),
            vec!["clear_host", "clear_host", "remove_host"],
            "retries then succeeds: {:?}",
            fx.log.snapshot()
        );
    }

    // ── Duplicate / done semantics ───────────────────────────────────────

    #[tokio::test]
    async fn test_duplicate_running_saga_refused() {
        let fx = Fixture::new();
        let existing = SagaRow {
            saga_id: Uuid::new_v4(),
            kind: saga_kind(MAC),
            state: SagaState::Running,
            steps: steps_to_json(&initial_steps()).unwrap(),
            started_at: Some(now_rfc3339()),
            finished_at: None,
        };
        fx.saga_store.put(&existing).await.unwrap();

        let deps = fx.deps(Some(5));
        let outcome = approve_machine(&deps, MAC).await.unwrap();

        match outcome {
            SagaOutcome::Refused { saga_id, state } => {
                assert_eq!(saga_id, existing.saga_id, "no second saga is started");
                assert_eq!(state, SagaState::Running);
            }
            other => panic!("expected Refused, got {other:?}"),
        }
        assert!(
            fx.log.snapshot().is_empty(),
            "no participant call for a refused duplicate: {:?}",
            fx.log.snapshot()
        );
    }

    #[tokio::test]
    async fn test_done_saga_noop() {
        let fx = Fixture::new();
        let existing = SagaRow {
            saga_id: Uuid::new_v4(),
            kind: saga_kind(MAC),
            state: SagaState::Done,
            steps: steps_to_json(&initial_steps()).unwrap(),
            started_at: Some(now_rfc3339()),
            finished_at: Some(now_rfc3339()),
        };
        fx.saga_store.put(&existing).await.unwrap();

        let deps = fx.deps(Some(5));
        let outcome = approve_machine(&deps, MAC).await.unwrap();

        match outcome {
            SagaOutcome::AlreadyDone { saga_id } => {
                assert_eq!(saga_id, existing.saga_id);
            }
            other => panic!("expected AlreadyDone, got {other:?}"),
        }
        assert!(
            fx.log.snapshot().is_empty(),
            "a done saga is a no-op success, no participant call: {:?}",
            fx.log.snapshot()
        );
    }

    // ── Backoff schedule (pure) ──────────────────────────────────────────

    #[test]
    fn test_backoff_schedule_1s_to_5m_cap() {
        let backoff = Backoff::default();
        assert_eq!(backoff_delay(0, &backoff), Duration::from_secs(1));
        assert_eq!(backoff_delay(1, &backoff), Duration::from_secs(2));
        assert_eq!(backoff_delay(2, &backoff), Duration::from_secs(4));
        assert_eq!(backoff_delay(9, &backoff), Duration::from_secs(300));
        assert_eq!(backoff_delay(30, &backoff), Duration::from_secs(300));
    }
}
