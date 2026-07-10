<!-- file: docs/agent-tasks/control/TASK-05-approve-saga.md -->
<!-- version: 1.0.0 -->
<!-- guid: 21f57d9e-e0c6-4928-98f3-eedd0eabde4d -->
<!-- last-edited: 2026-07-10 -->

# TASK-05 — Fill saga.rs: ApproveMachine SAGA with ordered placement→activation, compensation_pending retry, restart resume (ws2-control)

**Priority:** P1 · **Effort:** L · **Recommended subagent:** Sonnet-class · rust-service subagent · **Why:** distributed failure-path logic; the ORDERING is the safety property (spec C3 repair) — activation before placement can boot a host into an installer with no seed. · **Depends on:** TASK-01 (wave-4 gated: CT-01 merged — the `saga.rs` stub must exist on origin/main)

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/control-approve-saga" -b agent/control-approve-saga origin/main
cd "$REPO/.worktrees/control-approve-saga"
git rebase origin/main
```

(Protocol also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

## Goal

Fill the CT-01 stub `crates/uaa-control/src/saga.rs` (your EXCLUSIVE file) with the `ApproveMachine` SAGA per spec C3 (Decision-13-adjacent, ops-judge repair): a strictly ORDERED, persisted, resumable state machine —

1. `WebService.PlaceSeed` + `WebService.PlaceIpxe` (**inert placement FIRST**),
2. `PxeService.SetupPxe` + `PxeService.SetBootTarget` (**activation LAST** — a failure between steps leaves the host inert, never activated-with-no-seed),
3. registry `status=approved` + `boot_target` write + audit record.

Compensation runs the REVERSE order; an unreachable participant during compensation parks the saga in `compensation_pending` with exponential retry — it is **NEVER falsely marked `compensated`**. Every transition persists to `saga_log` (via a store trait) and an interrupted saga resumes from it after restart. Tested ENTIRELY against mock gRPC clients — no live uaa-web/uaa-pxe/CockroachDB.

Purely additive to `saga.rs`. Reuse — do not invent parallels:
- **`SagaRow`** from CT-01's `db/mod.rs` (verify: `grep -n "pub struct SagaRow" crates/uaa-control/src/db/mod.rs` at execution time) — states `running|done|compensating|compensated|compensation_pending` exactly as the `saga_log` schema comment pins them.
- **`audit::record`** (CT-04, same wave — if not yet merged when you start, call through a narrow local `trait AuditSink { async fn record(&self, …) -> Result<()>; }` with the real wiring left as a one-line TODO naming CT-07; do NOT block on CT-04).
- **`MemRegistryStore`** (CT-02, same wave) for registry-write tests IF already merged; otherwise a local minimal mock behind your own `trait RegistryWrites` — same rule as above.

## Background (verify before editing)

- Spec: C3 "Approve SAGA" bullet (ordered, compensation, `compensation_pending`, resume from `saga_log`), Decision 13 (boot_target authority), `saga_log` schema (CT-01 migration — do not touch it).
- Participant seams (define here; the real tonic clients are wired by CT-07/later): `pub trait WebClient: Send + Sync { async fn place_seed(&self, mac, payload) -> Result<()>; async fn place_ipxe(&self, mac, payload) -> Result<()>; async fn remove_host(&self, mac) -> Result<()>; async fn flip_boot_target(&self, mac, target) -> Result<bool>; }` and `pub trait PxeClient: Send + Sync { async fn setup_pxe(&self, mac) -> Result<()>; async fn set_boot_target(&self, mac, target) -> Result<()>; async fn clear_host(&self, mac) -> Result<()>; }`, plus `pub trait SagaStore { async fn put(&self, &SagaRow) -> Result<()>; async fn list_unfinished(&self) -> Result<Vec<SagaRow>>; }` (`MemSagaStore` for tests, `PgSagaStore` runtime-only).
- Compensation mapping (spell twice — here and Step 3): step-3 failure → un-write registry is not needed (registry write is last and atomic) → compensate 2 then 1; step-2 failure → compensate anything already activated (`clear_host`), then remove placement (`remove_host`); step-1 partial failure → remove whatever placed. Compensation step Err → state `compensation_pending`, retry with exponential backoff (base 1s, cap 5m, jittered), transitioning to `compensated` ONLY when every remaining compensation step returns Ok. There is NO code path that writes `compensated` while any compensation step is unconfirmed.
- Edge semantics: a saga interrupted mid-`running` resumes by re-running its NEXT unexecuted step (steps are recorded per-step in `steps` JSONB with `{name, state: pending|ok|failed}`); participant calls must therefore be idempotent-tolerant (placing an already-placed seed is Ok — the mocks model this); duplicate `ApproveMachine` for a mac with a saga already `running` → typed refusal (no second saga), `done` → no-op success.
- Every state transition goes through ONE `persist_then(…)` helper: write `saga_log` FIRST, then act — a crash between persist and act re-runs the acted step (safe by idempotent-tolerance), never skips it.

**HARD RULES (non-negotiable):**
- NO hardware actions. Validate ONLY in-repo (`cargo`) and, where a brief says so,
  the QEMU+swtpm harness (`scripts/vm-validate.sh`). Code that COULD touch hardware
  is written and unit-tested against mock executors only.
- NEVER wipe, write to, or deploy on 172.16.2.30 ("the server") or len-serv-003.
- `disk_device` is read from the live target at runtime, never guessed or hardcoded.
- ipmitool runs via `ssh 172.16.2.30`, never on macOS.
- NEVER power on unimatrixone (U1).
- No real secret in any file: `REPLACE_AT_PLACE_TIME` placeholders stay placeholders.
- Stay inside your worktree; never `git push`, `gh pr`, or merge — report done and stop.

- **Re-verify these anchors before editing** — line numbers drift; zero hits at both
  old and mapped path = STOP and report:
  ```bash
  grep -n "activation LAST" docs/specs/constellation-design.md        # expect: 1 hit (~line 460 — the ordering law, normative)
  grep -n "compensation_pending" docs/specs/constellation-design.md   # expect: 3 hits (~lines 345, 463, 596)
  grep -n "NEVER falsely marked" docs/specs/constellation-design.md   # expect: 1 hit (~line 463)
  grep -n "saga_log" docs/specs/constellation-design.md               # expect: 3+ hits (schema + resume rule)
  test -f crates/uaa-control/src/saga.rs && echo OK                   # expect: OK (wave gate: CT-01 merged; missing = STOP, too early)
  ```

## Step-by-step

1. Run the ⛔ START HERE block, then every anchor grep above. Any zero-hit / missing-file result → STOP and report.

2. **Types.** `SagaState` enum matching the schema comment exactly; `StepRecord { name: &'static str, state: StepState }`; the fixed step list as a `const` array `["place_seed", "place_ipxe", "setup_pxe", "set_boot_target", "registry_approve"]` — the array ORDER is the safety property, put the spec quote ("activation LAST") in a comment on it.
3. **Forward driver.** `pub async fn approve_machine(deps: &SagaDeps, mac: &str) -> Result<SagaOutcome>` where `SagaDeps` bundles `&dyn WebClient / PxeClient / SagaStore / RegistryWrites / AuditSink` + a `Backoff` policy (injectable clock/sleeper so tests don't sleep). Behavior: refuse duplicate `running` saga for the mac; persist `running` + all steps `pending`; execute steps IN ARRAY ORDER, persisting each step `ok` before the next; on step Err → persist step `failed`, state `compensating`, run compensation (Step 4). Repeat: `set_boot_target` (activation) runs only after both placements are `ok` — no reordering, no parallel join.
4. **Compensation.** Reverse-order undo of every step marked `ok` (mapping in Background); each compensation call Ok → persist; any Err → state `compensation_pending`, schedule retry (exponential, base 1s, cap 5m, injectable sleeper), loop until all remaining undos succeed → `compensated`. Assert in code (comment + structure): the ONLY write of `SagaState::Compensated` sits after the all-undos-Ok check.
5. **Resume.** `pub async fn resume_unfinished(deps) -> Result<Vec<SagaOutcome>>` — for each `saga_log` row in `running` → continue from first non-`ok` step; `compensating`/`compensation_pending` → re-enter the compensation loop. Called from `serve` startup (one line in `main.rs`/`lib.rs` serve path).
6. **Unit tests** (recording mocks with per-call scripted results + call-order log; injectable no-sleep backoff): `test_happy_path_order` — **anti-over-suppression:** all mocks Ok → outcome `done`, registry approved, and the recorded call order is EXACTLY `[place_seed, place_ipxe, setup_pxe, set_boot_target, registry_approve]` (the guard/ordering machinery does not block or reorder the legitimate flow); `test_activation_never_before_placement` (failing `place_ipxe` → `setup_pxe`/`set_boot_target` never called); `test_step2_failure_compensates_reverse` (`set_boot_target` fails → undo order `[clear_host, remove_host]`, state `compensated`, registry NOT approved); `test_unreachable_compensation_parks_pending` (undo Err ×3 then Ok → state visits `compensation_pending`, retries, ends `compensated`; assert it was NEVER `compensated` while an undo was outstanding — check the persisted state sequence); `test_never_falsely_compensated` (undo Err forever, bounded test iterations → final persisted state is `compensation_pending`, NOT `compensated`); `test_resume_running_continues` (persist a `running` row with first two steps `ok`, resume → only steps 3..5 execute); `test_resume_compensation_pending_retries`; `test_duplicate_running_saga_refused`; `test_done_saga_noop`.
7. Bump the file header (version + last-edited) on every file you touch; new files get a fresh 4-line header with a new uuid4 (`uuidgen | tr 'A-F' 'a-f'`).

## How to test

```bash
cargo test --lib --offline && cargo build --offline
# Expected: all tests pass (baseline 311 + prior control tests + the ~9 new tests), build clean
cargo clippy --offline -- -D warnings
# Expected: no warnings
cargo test --lib --offline test_never_falsely_compensated test_activation_never_before_placement
# Expected: 2 passed (the two safety-property tests specifically)
grep -n "activation LAST" crates/uaa-control/src/saga.rs
# Expected: 1+ hits (the spec law quoted on the step array)
```

## Acceptance criteria

- [ ] Only `saga.rs` (+ one resume call line in the serve path) changed: `git diff origin/main --stat` shows no other `crates/uaa-control/src/` file.
- [ ] Ordering law proven: `test_happy_path_order` and `test_activation_never_before_placement` pass (exact call order asserted, activation never precedes placement).
- [ ] Never-falsely-compensated proven: `test_never_falsely_compensated` and `test_unreachable_compensation_parks_pending` pass (persisted state sequence asserted).
- [ ] Resume proven: `test_resume_running_continues` and `test_resume_compensation_pending_retries` pass; `grep -n "resume_unfinished" crates/uaa-control/src/` → hit in the serve path.
- [ ] Duplicate/done semantics: `test_duplicate_running_saga_refused` + `test_done_saga_noop` pass.
- [ ] **Anti-over-suppression:** `test_happy_path_order` passes — a fully healthy approve flows through every guard to `done`.
- [ ] All participants mocked: `grep -rn "7445\|7446\|tonic::transport" crates/uaa-control/src/saga.rs` → 0 hits outside comments (no live client construction in this file).
- [ ] `cargo test --lib --offline && cargo build --offline` exits 0; clippy clean; no test sleeps in real time or opens a network connection.
- [ ] File headers bumped on every changed file (`git diff origin/main --stat` files each show a version bump; guids unchanged).

## Commit message

```
feat(control): ApproveMachine SAGA — ordered placement-then-activation, compensation_pending retry, saga_log resume (ws2-control)

Fills the CT-01 saga.rs stub per spec C3: fixed step array (place_seed,
place_ipxe, setup_pxe, set_boot_target LAST, registry_approve) driven through
persist-then-act transitions in saga_log; reverse-order compensation where an
unreachable participant parks the saga in compensation_pending with capped
exponential retry and `compensated` is only ever written after every undo
confirms; restart resume for running/compensating/pending rows. WebClient/
PxeClient/SagaStore trait seams with recording mocks — ordering, false-
compensation, and resume all asserted; no live services or DB in tests.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

If `grep -n "pub async fn approve_machine" crates/uaa-control/src/saga.rs` hits, already applied — run the acceptance checks instead of re-applying. Rollback = revert the single commit; `saga.rs` returns to CT-01's header-only stub; no saga rows exist anywhere until the daemon runs against a real DB, so nothing external needs unwinding.
