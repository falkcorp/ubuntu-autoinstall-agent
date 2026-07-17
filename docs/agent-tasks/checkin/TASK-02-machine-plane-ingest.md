<!-- file: docs/agent-tasks/checkin/TASK-02-machine-plane-ingest.md -->
<!-- version: 1.0.0 -->
<!-- guid: ffdfacfe-624a-4ebc-9122-53879e8e33ed -->
<!-- last-edited: 2026-07-16 -->

# TASK-02 — Machine-plane application-status ingest (DS-CHK-02)

**Priority:** P2 · **Effort:** M · **Recommended subagent:** Sonnet-class · rust-control subagent · **Why:** touches the fail-open telemetry path and the shared MachineRow snapshot; must not extend MachineStatus. · **Depends on:** DS-CHK-01 (needs `AppStatusPayload`)

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/checkin-machine-plane-ingest" -b agent/checkin-machine-plane-ingest origin/main
cd "$REPO/.worktrees/checkin-machine-plane-ingest"
git rebase origin/main
```

(Protocol also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

**Wave gate:** DS-CHK-01 must be merged. If `grep -n "pub struct AppStatusPayload" crates/uaa-core/src/app_status.rs` returns 0 hits, the gate is not met: STOP and report.

## Goal

Add a `POST /api/app-status` route to the machine plane that ingests DS-CHK-01's payload and stores it on the machine's snapshot row, so the operator can see whether a machine's workloads are running.

REUSE — do not invent parallels:

- **Mirror the existing ingest handlers** in `crates/uaa-control/src/machine_plane/lifecycle.rs` — verify: `grep -n '.route("/api/checkin"' crates/uaa-control/src/machine_plane/lifecycle.rs`. Same router, same `parse_json` helper, same `Registry` trait seam, same JSON error convention.
- **`normalize_mac`** — verify: `grep -n "pub fn normalize_mac" crates/uaa-control/src/machine_plane/lifecycle.rs`. Every handler taking a MAC calls it first.
- **`crate::db::store::{read_snapshot, write_snapshot}`** — the machine plane's store. **Here `read_snapshot`'s fail-open behavior is CORRECT** (this is telemetry ingest, not allocation) — use it, not `read_snapshot_strict`.
- Row types from `db/mod.rs` — import, never redefine.

## Background (verify before editing)

- **⚠ The machine plane never touches CockroachDB.** It writes the on-disk snapshot + WAL via `crate::db::store`, deliberately (fail-open telemetry ingest). Do not reach for `RegistryStore` or `ProfileStore` here.
- **⚠ Do NOT extend `MachineStatus`.** It is the *registration* lifecycle (`Seen|Pending|Approved|Revoked|Unknown`) and its `Unknown(String)` variant exists to round-trip dirty legacy Python parity data. Folding runtime health into it would conflate two different questions and break that parity. Application health is a **separate, additive field** on `MachineRow`.
- **This path is fail-OPEN by design** — verify: `grep -n "fail-OPEN" crates/uaa-control/src/machine_plane/lifecycle.rs`. A telemetry append must never 503. Keep that: a malformed body is a 400, but an ingest that cannot write must not take the plane down.
- **Staleness is NOT computed here.** This task only records what was reported plus a `last_app_status_at` timestamp. Deciding that silence means `Stale` is DS-CHK-03, at **read** time. Do not add a reaper, a TTL sweep, or a background job — the ingest path stays fail-open and job-free.
- Edge semantics (spelled out here AND in acceptance):
  - **Empty `reports` list** → **valid**; record "this host runs zero applications" and stamp `last_app_status_at`. NOT an error, NOT a skipped write. A host with no applications reporting in is meaningful data.
  - **Unknown MAC** (no `MachineRow`) → mirror what `/api/checkin` does for an unknown MAC; do not invent a different behavior. Re-verify its handler and match it.
  - **Malformed JSON** → `400 {"error": "invalid json"}` via the shared `parse_json` helper — the file's existing convention.
  - **A report for an application not in the machine's profile** → record it anyway. This handler is a telemetry sink, not a validator; rejecting it would hide a real running service.

**HARD RULES (non-negotiable):**
- **NO SQL, NO migration** — no DB connection exists in production (spec D4).
- NO hardware actions. NEVER wipe/write/deploy on 172.16.2.30 or len-serv-003. NEVER power on unimatrixone.
- Do NOT extend `MachineStatus`; do NOT add a background job/reaper.
- Do NOT use `read_snapshot_strict` here — fail-open is correct for telemetry.
- Stay inside your worktree; never `git push`, `gh pr`, or merge — report done and stop.

- **Re-verify these anchors before editing** — line numbers drift; zero hits = STOP and report:
  ```bash
  grep -n "pub struct AppStatusPayload" crates/uaa-core/src/app_status.rs
  # expect: 1 hit — DS-CHK-01 merged (0 hits = wave gate not met, STOP)
  grep -n '.route("/api/checkin"' crates/uaa-control/src/machine_plane/lifecycle.rs
  # expect: 1 hit (~line 410) — the ingest handler + route style to mirror
  grep -n "pub fn normalize_mac" crates/uaa-control/src/machine_plane/lifecycle.rs
  # expect: 1 hit — call it on every MAC
  grep -n "pub struct MachineRow" crates/uaa-control/src/db/mod.rs
  # expect: 1 hit (~line 193) — add the additive field here
  grep -n "pub enum MachineStatus" crates/uaa-control/src/db/mod.rs
  # expect: 1 hit (~line 79) — do NOT touch this
  grep -n "fail-OPEN" crates/uaa-control/src/machine_plane/lifecycle.rs
  # expect: 1 hit — the ingest contract you must preserve
  ```

## Step-by-step

1. In `crates/uaa-control/src/db/mod.rs`, add two **additive, defaulted** fields to `MachineRow`:
   ```rust
   /// Application health as last reported by the host. Separate from
   /// MachineStatus, which is the REGISTRATION lifecycle — conflating them
   /// would break the Python parity its Unknown(String) variant preserves.
   #[serde(default)]
   pub app_reports: Vec<AppStatusReportRow>,
   /// When app_reports was last written. DS-CHK-03 computes staleness from
   /// this at READ time; nothing writes a status on absence.
   #[serde(default)]
   pub last_app_status_at: Option<String>,
   ```
   plus the `AppStatusReportRow` row type (`kind`, `unit`, `active`, `detail`), mirroring DS-CHK-01's `AppStatusReport`.
   **`#[serde(default)]` is mandatory** — every existing snapshot on disk lacks these fields and must keep parsing, and every `MachineRow` struct literal in the crate must still compile.
2. In `lifecycle.rs`, add `POST /api/app-status` mirroring `/api/checkin`'s handler shape: `parse_json` → `normalize_mac` → read snapshot → update the row → `write_snapshot`. Return the file's existing JSON success shape.
3. Keep purely additive — do not modify `MachineStatus`, `/api/checkin`, `/api/webhook`, or any existing route.
4. Add tests in `lifecycle.rs`'s test module (mirror its existing handler tests):
   - `test_app_status_records_reports` — a payload with one active report ⇒ the row carries it and `last_app_status_at` is set.
   - **`test_app_status_empty_reports_is_valid`** — an empty `reports` list ⇒ 200, row updated, timestamp stamped. Not an error, not skipped.
   - `test_app_status_malformed_json_400` — `400 {"error":"invalid json"}`.
   - `test_app_status_does_not_touch_machine_status` — a row that was `Approved` is still `Approved` afterwards.
   - `test_snapshot_without_app_fields_still_parses` — a snapshot JSON lacking both new fields round-trips (the on-disk backward-compat guard).
5. Bump headers on every file you touch; keep existing guids.

**Anti-over-suppression:** `test_app_status_empty_reports_is_valid` is the happy-path guard — a handler that treated "no reports" as nothing-to-do would silently skip the write, and a host that legitimately runs zero applications would look like a host that never checked in. The empty case must be recorded, not filtered out.

## How to test

```bash
cargo test --lib --offline
# Expected: 634+ passed, 0 failed (baseline + DS-CHK-01's 4 + your 5).
cargo build --offline
# Expected: exit 0.
cargo clippy --offline -- -D warnings
# Expected: no warnings.
```

## Acceptance criteria

- [ ] `cargo test --lib --offline` exits 0 — verify: `cargo test --lib --offline 2>&1 | grep -E "^test result"`
- [ ] `cargo build --offline` exits 0 — verify: `cargo build --offline && echo BUILD_OK`
- [ ] **`MachineStatus` untouched** — verify: `git diff origin/main -- crates/uaa-control/src/db/mod.rs | grep -c "enum MachineStatus"` returns **0**
- [ ] Both new fields are defaulted — verify: `grep -B1 "pub app_reports\|pub last_app_status_at" crates/uaa-control/src/db/mod.rs | grep -c "serde(default)"` returns 2
- [ ] On-disk backward compat — verify: `cargo test --lib --offline test_snapshot_without_app_fields_still_parses`
- [ ] Anti-over-suppression: the empty case is recorded — verify: `cargo test --lib --offline test_app_status_empty_reports_is_valid`
- [ ] No background job — verify: `git diff origin/main | grep -c "tokio::spawn\|interval(" ` returns **0**
- [ ] Fail-open preserved — verify: `grep -c "read_snapshot_strict" crates/uaa-control/src/machine_plane/lifecycle.rs` returns **0**
- [ ] No SQL — verify: `git diff origin/main | grep -c "CREATE TABLE\|SQL_"` returns **0**
- [ ] `cargo clippy --offline -- -D warnings` clean
- [ ] File headers bumped — verify: `git diff origin/main --name-only | xargs -I{} grep -l "last-edited: 2026-07" {}`

## Commit message

```
feat(control): ingest application status on the machine plane (DS-CHK-02)

Adds POST /api/app-status mirroring /api/checkin's handler shape, and two
additive #[serde(default)] fields on MachineRow (app_reports,
last_app_status_at) so snapshots already on disk keep parsing.

MachineStatus is deliberately NOT extended: it is the registration lifecycle,
and its Unknown(String) variant exists to round-trip dirty Python parity data.
Application health is a separate field.

An empty reports list is valid and recorded — a host legitimately running zero
applications is meaningful data, and skipping the write would make it
indistinguishable from a host that never checked in.

Staleness is not computed here: nothing writes a status on absence. DS-CHK-03
derives Stale from last_app_status_at at READ time, keeping this ingest path
fail-open and free of any background job.

Co-Authored-By: Claude <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

**Polarity: additive.** If `grep -n '"/api/app-status"' crates/uaa-control/src/machine_plane/lifecycle.rs` hits, this task is already applied — run the acceptance checks instead of re-applying. Rollback = revert the single commit; existing snapshots are unaffected (both fields were additive and defaulted), `MachineStatus` and every existing route are untouched, and no machine's behavior changes. **`db/mod.rs` is shared with DS-REG-01 (wave 1)** — this task is wave 4 and must rebase after DS-REG-01 merges; see the collision table in `../BREAKDOWN-2026-07-16.md`.
