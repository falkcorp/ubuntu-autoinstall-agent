<!-- file: docs/agent-tasks/checkin/TASK-03-read-time-staleness.md -->
<!-- version: 1.0.0 -->
<!-- guid: a3910fe6-1758-42da-af87-3c2dca5fec64 -->
<!-- last-edited: 2026-07-16 -->

# TASK-03 — Read-time staleness: "no news" must not read as healthy (DS-CHK-03)

**Priority:** P2 · **Effort:** M · **Recommended subagent:** Sonnet-class · rust-control subagent · **Why:** the failure it fixes is silent and actively misleading — a dead box renders green forever. · **Depends on:** DS-CHK-02 (needs `app_reports` + `last_app_status_at`)

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/checkin-read-time-staleness" -b agent/checkin-read-time-staleness origin/main
cd "$REPO/.worktrees/checkin-read-time-staleness"
git rebase origin/main
```

(Protocol also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

**Wave gate:** DS-CHK-02 must be merged. If `grep -n "pub last_app_status_at" crates/uaa-control/src/db/mod.rs` returns 0 hits, the gate is not met: STOP and report.

## Goal

Add a **read-time** staleness computation so the operator can tell *reported healthy* from *hasn't reported since T*.

> ## ⚠ The failure this fixes
>
> The machine plane writes application health **only on check-in**, and **nothing flips a status on absence** — there is no reaper and no TTL. So a machine whose Cockroach died, or whose NIC died, or which never booted after a reinstall, keeps its **last-known-good** health in the snapshot **forever**, and the dashboard renders it **green**.
>
> That is strictly worse than showing nothing: the dashboard actively asserts health for a dead box.
>
> The fix is deliberately **not** a background job: compute `Stale`/`Unknown` from `last_app_status_at` **at read time**. That keeps the ingest path fail-open and job-free, and needs no new persistence.

REUSE — do not invent parallels:

- **`MachineRow.last_app_status_at`** (DS-CHK-02) — the only input. Do not add a new timestamp.
- The existing time handling in the crate: timestamps are `Option<String>` on rows, per `db/mod.rs`'s convention — verify: `grep -n "pub last_seen" crates/uaa-control/src/db/mod.rs`. Parse, don't restructure.
- **`crate::db::store::read_snapshot`** — fail-open is correct on this read path (it is the dashboard/telemetry side, not allocation).

## Background (verify before editing)

- **Read-time, not write-time.** No `tokio::spawn`, no `interval`, no reaper, no sweep. A pure function decides freshness from a timestamp and "now"; callers pass `now` in so it is testable without a clock.
- **Three states, and the distinction is the whole point:**
  - `Fresh` — reported within the threshold.
  - `Stale` — reported, but longer ago than the threshold. **Not the same as unhealthy** — it means *we don't know*.
  - `NeverReported` — `last_app_status_at` is `None`. Also **not** unhealthy, and **not** healthy.
  A machine that reported `active: true` an hour ago is **`Stale`, not healthy** — that is exactly the bug.
- **Threshold** is a named constant with a doc comment justifying its value (a small multiple of the expected report interval). Do not scatter a magic number.
- Edge semantics (spelled out here AND in acceptance):
  - **`last_app_status_at: None`** → `NeverReported`. Never `Fresh`, never `Stale` — a machine that has never reported is a different fact from one that stopped.
  - **Unparseable timestamp** → `NeverReported` **plus a warn log naming the MAC**. Never `Fresh` — a corrupt timestamp must not read as healthy. Fail toward "we don't know".
  - **Timestamp in the future** (clock skew) → `Fresh`. Do not error; a skewed clock is not a health signal, and erroring here would take a dashboard read down.
  - **Empty `app_reports` + a recent timestamp** → `Fresh`, zero applications. A host running nothing, reporting in, is healthy — do not conflate "no applications" with "no news".

**HARD RULES (non-negotiable):**
- **NO SQL, NO migration** — no DB connection exists in production (spec D4).
- NO hardware actions. NEVER wipe/write/deploy on 172.16.2.30 or len-serv-003. NEVER power on unimatrixone.
- **NO background job, reaper, or TTL sweep** — this is read-time only. If you write `tokio::spawn` or `interval(`, you have the wrong design.
- Do NOT write a status onto the row. Nothing about this task mutates state.
- Do NOT extend `MachineStatus`.
- Stay inside your worktree; never `git push`, `gh pr`, or merge — report done and stop.

- **Re-verify these anchors before editing** — line numbers drift; zero hits = STOP and report:
  ```bash
  grep -n "pub last_app_status_at" crates/uaa-control/src/db/mod.rs
  # expect: 1 hit — DS-CHK-02 merged (0 hits = wave gate not met, STOP)
  grep -n "pub app_reports" crates/uaa-control/src/db/mod.rs
  # expect: 1 hit — the reports this task classifies
  grep -n "pub last_seen" crates/uaa-control/src/db/mod.rs
  # expect: 1 hit — the Option<String> timestamp convention to follow
  grep -n "pub enum MachineStatus" crates/uaa-control/src/db/mod.rs
  # expect: 1 hit — do NOT touch this
  ```

## Step-by-step

1. Create `crates/uaa-control/src/machine_plane/staleness.rs` with a fresh 4-line header (new uuid4 via `uuidgen | tr '[:upper:]' '[:lower:]'`), declared from `machine_plane/mod.rs` mirroring its existing `pub mod` lines.
2. Implement, pure and clock-injected:
   ```rust
   /// A machine that reported `active: true` an hour ago is Stale, NOT healthy.
   /// Stale means "we don't know", which is different from unhealthy AND
   /// different from healthy.
   #[derive(Debug, Clone, Copy, PartialEq, Eq)]
   pub enum Freshness { Fresh, Stale, NeverReported }

   /// A small multiple of the expected report interval: long enough that a
   /// slow host is not flagged, short enough that a dead one stops reading green.
   pub const APP_STATUS_STALE_AFTER_SECS: i64 = 900;

   /// Read-time only. `now` is injected so this is testable without a clock,
   /// and so no background job is needed.
   pub fn freshness(last_app_status_at: Option<&str>, now_unix: i64, mac: &str) -> Freshness;
   ```
3. Keep purely additive — no mutation, no new field, no job.
4. Add tests in `staleness.rs`'s `mod tests`:
   - `test_recent_report_is_fresh`.
   - **`test_old_healthy_report_is_stale_not_fresh`** — a report of `active: true` from 2 hours ago ⇒ `Stale`. *This is the bug: it must not read as healthy.*
   - `test_never_reported_is_never_reported` — `None` ⇒ `NeverReported`, **not** `Stale` and **not** `Fresh`.
   - `test_unparseable_timestamp_is_never_reported` — garbage ⇒ `NeverReported`, never `Fresh`.
   - `test_future_timestamp_is_fresh` — clock skew ⇒ `Fresh`, no panic, no error.
   - **`test_empty_reports_with_recent_timestamp_is_fresh`** — a host running zero applications that reported 1 minute ago ⇒ `Fresh`. "No applications" ≠ "no news".
   - `test_boundary_exactly_at_threshold` — pin the boundary explicitly so a future refactor cannot flip `<` to `<=` unnoticed.
5. Bump headers on every file you touch; keep existing guids.

**Anti-over-suppression:** `Stale` is a filter over what the dashboard trusts, so it can over-block. `test_recent_report_is_fresh` and `test_empty_reports_with_recent_timestamp_is_fresh` are the happy-path guards — a threshold that flagged healthy, recently-reporting machines as `Stale` would make the whole signal noise and train the operator to ignore it.

## How to test

```bash
cargo test --lib --offline
# Expected: 634+ passed, 0 failed (baseline + DS-CHK-01/02's tests + your 7).
cargo build --offline
# Expected: exit 0.
cargo clippy --offline -- -D warnings
# Expected: no warnings.
```

## Acceptance criteria

- [ ] `cargo test --lib --offline` exits 0 — verify: `cargo test --lib --offline 2>&1 | grep -E "^test result"`
- [ ] `cargo build --offline` exits 0 — verify: `cargo build --offline && echo BUILD_OK`
- [ ] **An old healthy report is Stale, not Fresh** — verify: `cargo test --lib --offline test_old_healthy_report_is_stale_not_fresh`
- [ ] Never-reported ≠ stale ≠ fresh — verify: `cargo test --lib --offline test_never_reported_is_never_reported test_unparseable_timestamp_is_never_reported`
- [ ] **No background job** — verify: `grep -c "tokio::spawn\|interval(\|sleep(" crates/uaa-control/src/machine_plane/staleness.rs` returns **0**
- [ ] Read-time only, no mutation — verify: `grep -c "write_snapshot\|&mut " crates/uaa-control/src/machine_plane/staleness.rs` returns **0**
- [ ] Anti-over-suppression: healthy machines stay Fresh — verify: `cargo test --lib --offline test_recent_report_is_fresh test_empty_reports_with_recent_timestamp_is_fresh`
- [ ] Threshold is a named constant — verify: `grep -c "APP_STATUS_STALE_AFTER_SECS" crates/uaa-control/src/machine_plane/staleness.rs` returns ≥2 (declaration + use)
- [ ] `MachineStatus` untouched — verify: `git diff origin/main -- crates/uaa-control/src/db/mod.rs | grep -c "enum MachineStatus"` returns **0**
- [ ] `cargo clippy --offline -- -D warnings` clean
- [ ] File headers bumped — verify: `git diff origin/main --name-only | xargs -I{} grep -l "last-edited: 2026-07" {}`

## Commit message

```
feat(control): read-time staleness so "no news" stops reading as healthy (DS-CHK-03)

The machine plane writes application health only on check-in and nothing flips
a status on absence — no reaper, no TTL. So a machine whose Cockroach died, or
whose NIC died, or which never booted after a reinstall, kept its
last-known-good health forever and rendered green. That is worse than showing
nothing: the dashboard actively asserted health for a dead box.

Adds a pure, clock-injected freshness() computed at READ time — Fresh / Stale /
NeverReported — so the ingest path stays fail-open and no background job is
introduced. Stale means "we don't know", which is distinct from unhealthy and
from healthy; a host that reported active:true an hour ago is Stale.

An unparseable timestamp reads as NeverReported, never Fresh: a corrupt
timestamp must not render green.

Co-Authored-By: Claude <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

**Polarity: additive.** If `grep -n "pub fn freshness" crates/uaa-control/src/machine_plane/staleness.rs` hits, this task is already applied — run the acceptance checks instead of re-applying. Rollback = revert the single commit; the module is pure and standalone, nothing mutates, and the dashboard simply returns to rendering raw `app_reports` (the pre-existing, misleading behavior). No data, schema, or route is touched. No sibling shares this file.
