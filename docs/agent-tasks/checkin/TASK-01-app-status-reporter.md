<!-- file: docs/agent-tasks/checkin/TASK-01-app-status-reporter.md -->
<!-- version: 1.0.0 -->
<!-- guid: 27f407ec-f1bf-4348-b577-731b302e4733 -->
<!-- last-edited: 2026-07-16 -->

# TASK-01 — `app_status.rs` client reporter (mirror `luks_sync`) (DS-CHK-01)

**Priority:** P2 · **Effort:** S · **Recommended subagent:** Haiku-class · rust-client subagent · **Why:** mechanical mirror of an existing module — read local state, serialize, POST, gate on `ok:bool`. · **Depends on:** none (wave 1)

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/checkin-app-status-reporter" -b agent/checkin-app-status-reporter origin/main
cd "$REPO/.worktrees/checkin-app-status-reporter"
git rebase origin/main
```

(Protocol also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

## Goal

Create `crates/uaa-core/src/app_status.rs`: a small client module that collects the status of the applications installed on this host (via `systemctl is-active <unit>`) and POSTs it to uaa-control, so the operator can see whether a machine's workloads are actually healthy — not merely that it registered.

**This is a near-exact structural copy of `crates/uaa-core/src/luks_sync.rs`.** Read that file first; mirror its shape, its error handling, and its test strategy. Only the payload type and the endpoint differ.

REUSE — do not invent parallels:

- **Mirror `luks_sync`'s three-function split** — verify: `grep -n "pub struct LuksSyncPayload\|pub async fn post_sync\|pub async fn sync_credentials" crates/uaa-core/src/luks_sync.rs` (3 hits). It splits into: a payload struct, a pure `build_payload`/state-read half, and a thin `post_sync` HTTP seam kept separate **so tests need no live server**. Copy that split exactly.
- **The HTTP idiom** is `reqwest` + `serde_json::Value` body inspection with an `ok` bool: success requires **2xx AND `body.ok == true`**. `reqwest` is already a dependency — do NOT add `ureq`/`hyper`/anything.
- **`crate::error::AutoInstallError::{ConfigError, SystemError}`** for all error paths. Do NOT define a new error enum.
- **`CommandExecutor`** for every `systemctl` call — verify: `grep -rn "pub trait CommandExecutor" crates/uaa-core/src/network/`. Never shell out directly; tests inject a mock.
- Declare the module in `crates/uaa-core/src/lib.rs` mirroring the existing `pub mod luks_sync;` line — verify: `grep -n "pub mod luks_sync" crates/uaa-core/src/lib.rs`.

## Background (verify before editing)

- The control-side ingest endpoint is **DS-CHK-02's** job; this task only builds and posts the payload. The URL is caller-supplied (`control_url` parameter) — this module never hardcodes `172.16.2.30`.
- Payload shape (mirrors `LuksSyncPayload { mac, records }`):
  ```rust
  pub struct AppStatusPayload { pub mac: String, pub reports: Vec<AppStatusReport> }
  pub struct AppStatusReport { pub kind: String, pub unit: String, pub active: bool, pub detail: String }
  ```
  `kind` is the `ApplicationSpec` tag (e.g. `"cockroach"`); `unit` the systemd unit (e.g. `"cockroach.service"`); `detail` the raw `systemctl is-active` output, trimmed.
- Edge semantics (spelled out here AND in acceptance):
  - **No applications on this host** → send an **empty `reports` list**, NOT an error and NOT a skipped POST. Control learning "this host runs zero applications" is correct, useful data. Distinguish it in the return value (`reports_sent: 0`).
  - **`systemctl is-active` exits non-zero** → that is the NORMAL "inactive/failed" answer, **not** a transport error. Record `active: false` with the output in `detail` and keep going. Treating it as an error would mean a dead service reports nothing at all — the exact blindness this module exists to remove.
  - **Non-2xx or `ok:false` response** → `SystemError` including status + body message. This module is read-only with respect to the local host: no writes, no tmp files, nothing.
  - **Empty/invalid MAC** (not 6 colon-separated hex pairs) → `ConfigError` **before** any HTTP call or `systemctl` invocation.
- Testing: unit tests cover payload construction and status collection against a **mock executor** only; the POST is a thin separate seam needing no server. No network in any test.

**HARD RULES (non-negotiable):**
- NO hardware actions. All `systemctl` calls go through `CommandExecutor` mocks.
- NEVER wipe/write/deploy on 172.16.2.30 or len-serv-003. NEVER power on unimatrixone.
- No real secret in any file; `REPLACE_AT_PLACE_TIME` stays a placeholder.
- Stay inside your worktree; never `git push`, `gh pr`, or merge — report done and stop.

- **Re-verify these anchors before editing** — line numbers drift; zero hits = STOP and report:
  ```bash
  grep -n "pub struct LuksSyncPayload" crates/uaa-core/src/luks_sync.rs
  # expect: 1 hit (~line 29) — the payload shape to mirror
  grep -n "pub async fn post_sync" crates/uaa-core/src/luks_sync.rs
  # expect: 1 hit (~line 111) — the HTTP seam to mirror (2xx AND ok:true)
  grep -n "pub async fn sync_credentials" crates/uaa-core/src/luks_sync.rs
  # expect: 1 hit — the top-level entry point shape to mirror
  grep -n "pub mod luks_sync" crates/uaa-core/src/lib.rs
  # expect: 1 hit — add `pub mod app_status;` alongside it
  ```

## Step-by-step

1. Read `crates/uaa-core/src/luks_sync.rs` end to end. You are mirroring it.
2. Create `crates/uaa-core/src/app_status.rs` with a fresh 4-line header (new uuid4 via `uuidgen | tr '[:upper:]' '[:lower:]'`, version `1.0.0`, `last-edited: 2026-07-16`).
3. Implement, mirroring `luks_sync`'s split:
   - `AppStatusPayload` / `AppStatusReport` (above).
   - `pub async fn collect_status(runner: &mut dyn CommandExecutor, units: &[(String, String)]) -> Vec<AppStatusReport>` — one `systemctl is-active <unit>` per `(kind, unit)`; non-zero exit ⇒ `active: false`, never an `Err`.
   - `pub async fn post_status(control_url: &str, payload: &AppStatusPayload) -> Result<()>` — the thin HTTP seam (2xx AND `ok:true`).
   - `pub async fn report_status(...) -> Result<AppStatusOutcome>` — validate MAC, collect, post; returns `reports_sent`.
4. Declare `pub mod app_status;` in `crates/uaa-core/src/lib.rs`.
5. Keep purely additive — do not modify `luks_sync.rs` or any existing module.
6. Add tests in `app_status.rs`'s `mod tests` (mirror `luks_sync`'s test module):
   - `test_no_applications_sends_empty_reports` — zero units ⇒ payload with empty `reports`, `reports_sent: 0`, no error.
   - `test_inactive_unit_reports_false_not_error` — mock `systemctl is-active` exiting non-zero ⇒ `active: false` with the output in `detail`, and the report **is still produced**.
   - `test_active_unit_reports_true` — the happy path still produces `active: true`.
   - `test_invalid_mac_rejected_before_any_command` — bad MAC ⇒ `ConfigError`, and the mock recorded **zero** commands.
7. Bump the header on every file you touch; keep existing guids.

**Anti-over-suppression:** `test_active_unit_reports_true` is the happy-path guard against `test_inactive_unit_reports_false_not_error`'s non-zero-exit handling over-suppressing — i.e. proving the "non-zero is not an error" path does not swallow *every* result and report everything as inactive.

## How to test

```bash
cargo test --lib --offline
# Expected: 634+ passed, 0 failed (634 baseline + your 4).
cargo build --offline
# Expected: exit 0.
cargo clippy --offline -- -D warnings
# Expected: no warnings.
```

## Acceptance criteria

- [ ] `cargo test --lib --offline` exits 0 — verify: `cargo test --lib --offline 2>&1 | grep -E "^test result"`
- [ ] `cargo build --offline` exits 0 — verify: `cargo build --offline && echo BUILD_OK`
- [ ] Module declared — verify: `grep -c "pub mod app_status" crates/uaa-core/src/lib.rs` returns 1
- [ ] No new dependency — verify: `git diff origin/main --name-only | grep -c "Cargo.toml"` returns **0**
- [ ] No direct process spawn — verify: `grep -c "std::process::Command" crates/uaa-core/src/app_status.rs` returns **0**
- [ ] `luks_sync.rs` untouched — verify: `git diff origin/main --name-only | grep -c "luks_sync.rs"` returns **0**
- [ ] A dead service still reports — verify: `cargo test --lib --offline test_inactive_unit_reports_false_not_error`
- [ ] Anti-over-suppression: `cargo test --lib --offline test_active_unit_reports_true` passes
- [ ] `cargo clippy --offline -- -D warnings` clean
- [ ] File headers bumped — verify: `git diff origin/main --name-only | xargs -I{} grep -l "last-edited: 2026-07" {}`

## Commit message

```
feat(core): add app_status client reporter for application health (DS-CHK-01)

Mirrors luks_sync's shape — payload struct, pure collect half, thin POST seam
gated on 2xx AND ok:true — to report whether this host's installed
applications are actually running, which MachineStatus (a registration
lifecycle) does not cover.

A non-zero `systemctl is-active` is the normal inactive/failed answer, not a
transport error: the report is still produced with active:false, because
treating it as an error would mean a dead service reports nothing at all.

Nothing consumes the payload yet; control-side ingest is DS-CHK-02.

Co-Authored-By: Claude <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

**Polarity: additive.** If `grep -n "pub struct AppStatusPayload" crates/uaa-core/src/app_status.rs` hits, this task is already applied — run the acceptance checks instead of re-applying. Rollback = revert the single commit; the module is standalone, nothing calls it yet, and no data, schema, or existing module is touched. No sibling shares a file with this task.
