<!-- file: docs/agent-tasks/operator-api/TASK-02-api-drift-routes.md -->
<!-- version: 1.0.0 -->
<!-- guid: f81c11d1-911a-492b-9323-62dce872c025 -->
<!-- last-edited: 2026-07-16 -->

# TASK-02 — `/api/drift` review routes (accept / revert) (DS-OPS-02)

**Priority:** P2 · **Effort:** M · **Recommended subagent:** Sonnet-class · rust-control subagent · **Why:** mutating routes that must use `append_in_txn` and the role-grouping convention; wrong wiring is silently unauthenticated or unaudited. · **Depends on:** DS-OPS-01 (route group) **and** DS-REG-05 (accept/revert)

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/operator-api-api-drift-routes" -b agent/operator-api-api-drift-routes origin/main
cd "$REPO/.worktrees/operator-api-api-drift-routes"
git rebase origin/main
```

(Protocol also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

**Wave gate — BOTH must be merged:**
- `grep -n '"/api/groups"' crates/uaa-control/src/operator/handlers.rs` → ≥1 hit (DS-OPS-01)
- `grep -n "pub async fn revert_drift" crates/uaa-control/src/profiles/drift.rs` → 1 hit (DS-REG-05)

Zero hits on either = gate not met: STOP and report.

## Goal

Expose drift review over HTTP: `GET /api/drift` (list), `POST /api/drift/:object_id/accept`, `POST /api/drift/:object_id/revert`.

REUSE — do not invent parallels:

- **`scan_drift` / `accept_drift` / `revert_drift`** (DS-REG-05) — this task is a thin HTTP layer over them. Do **not** re-implement any drift logic here, and do **not** re-derive "last good version" — `revert_drift` owns that.
- **DS-OPS-01's route group + `AppState.profile_store`** — add to the existing `Role::Viewer` / `Role::Operator` groups; do not create a new router.
- **`Extension<auth::Session>`** → `&session.login` as the actor. Mirror `handle_approve_enrollment` — verify: `grep -n "async fn handle_approve_enrollment" crates/uaa-control/src/operator/handlers.rs`.
- **`json_response` + `ApiErrorBody`**; DTOs in `api_types.rs`, `Serialize`-only.
- **`audit_store`** already on `AppState` — pass it through to `accept_drift`/`revert_drift`.

## Background (verify before editing)

- **⚠ The UI copy is normative, not decoration.** Revert **restores intent, not the machine** (spec D11): v1 has no re-render, so a revert changes a stored row and **leaves the deployed host exactly as drifted as it was**. The API response must say so — a `note` field on the revert response stating that re-deploying is a separate action. An operator who reads "reverted" as "fleet fixed" is the failure mode this wording prevents.
- **Align with the SPA's existing drift vocabulary.** `web/src/api/types.ts`'s `MachineRow` already carries `consistent: boolean` — *"True when every provisioning layer for this machine agrees; false = drift"*. Use the same word; do not invent a second one for the same idea.
- **`accept_drift` / `revert_drift` already audit via `append_in_txn`.** This layer must **not** additionally call `record()` — that would double-log and, worse, `record()` passes a no-op mutation and must never be used for something that also changes state.
- Edge semantics (spelled out here AND in acceptance):
  - **Review on a non-drifted object** → `400` naming it ("no drift to review"). DS-REG-05 errors; surface it, never a silent 200.
  - **No good version to revert to** → `400` naming the object. Never invent a body, never fall back to the drifted one.
  - **Unknown `object_id`** → `404`.
  - **`GET /api/drift` with no drift** → `200 []`. An empty list is the healthy answer, not a 404.

**HARD RULES (non-negotiable):**
- **NO SQL, NO migration** — no DB connection exists in production (spec D4).
- NO hardware actions. NEVER wipe/write/deploy on 172.16.2.30 or len-serv-003. NEVER power on unimatrixone.
- Every mutating route MUST be `require_role(.., Role::Operator)`-wrapped and take `Extension<auth::Session>`.
- Do NOT re-implement drift logic; do NOT call `record()`.
- Do NOT claim resistance to a root-level adversary in any response, doc, or log string (spec D9).
- Stay inside your worktree; never `git push`, `gh pr`, or merge — report done and stop.

- **Re-verify these anchors before editing** — line numbers drift; zero hits = STOP and report:
  ```bash
  grep -n "pub async fn revert_drift\|pub async fn accept_drift\|pub async fn scan_drift" crates/uaa-control/src/profiles/drift.rs
  # expect: 3 hits — DS-REG-05 merged (0 = wave gate not met, STOP)
  grep -n '"/api/groups"' crates/uaa-control/src/operator/handlers.rs
  # expect: >=1 hit — DS-OPS-01 merged (0 = wave gate not met, STOP)
  grep -n "let operator_routes = auth::require_role" crates/uaa-control/src/operator/handlers.rs
  # expect: 1 hit — add your mutating routes to THIS group
  grep -n "async fn handle_approve_enrollment" crates/uaa-control/src/operator/handlers.rs
  # expect: 1 hit — the audited-mutation handler shape to mirror
  grep -n "consistent" web/src/api/types.ts
  # expect: 1 hit — the existing drift vocabulary to align with
  ```

## Step-by-step

1. In `api_types.rs`, add `Serialize`-only `DriftView` (object kind/id, stored vs actual hash as hex, `seen_count`) and `ReviewResultView` (new version, plus the normative `note` on revert).
2. In `handlers.rs`, add to DS-OPS-01's groups:
   - `Role::Viewer`: `GET /api/drift` → `scan_drift`
   - `Role::Operator`: `POST /api/drift/:object_id/accept` → `accept_drift`; `POST /api/drift/:object_id/revert` → `revert_drift`
3. Both mutating handlers: `Extension<auth::Session>` → call DS-REG-05 with `&session.login` → `json_response`. The revert response carries the "restores intent, not the machine; re-deploy separately" note.
4. Keep purely additive — do not modify DS-OPS-01's routes, `drift.rs`, or `auth.rs`.
5. Add tests in `handlers.rs`'s test module:
   - **`test_drift_review_requires_operator`** — accept/revert unauthenticated ⇒ 401/403.
   - `test_drift_list_requires_viewer`.
   - **`test_drift_list_empty_is_200_not_404`** — no drift ⇒ `200 []`.
   - `test_review_non_drifted_object_400` — named error, not a silent 200.
   - `test_revert_without_good_version_400`.
   - `test_unknown_object_404`.
   - **`test_revert_response_states_intent_not_machine`** — the response body contains the note. (The wording is normative.)
   - `test_review_uses_session_actor` — the audit event carries the session's login, not a placeholder.
   - **`test_accept_on_drifted_object_succeeds`** — the happy path works with auth present.
6. Bump headers on every file you touch; keep existing guids.

**Anti-over-suppression:** the auth gate and the "no drift to review" check both reject. `test_accept_on_drifted_object_succeeds` and `test_drift_list_empty_is_200_not_404` are the happy-path guards — without them, an over-strict gate would reject every legitimate review (and a 404-on-empty would make "no drift" look like a broken endpoint) while all the negative tests passed.

## How to test

```bash
cargo test --lib --offline
# Expected: 634+ passed, 0 failed (baseline + upstream waves' tests + your 9).
cargo build --offline
# Expected: exit 0.
cargo clippy --offline -- -D warnings
# Expected: no warnings.
```

## Acceptance criteria

- [ ] `cargo test --lib --offline` exits 0 — verify: `cargo test --lib --offline 2>&1 | grep -E "^test result"`
- [ ] `cargo build --offline` exits 0 — verify: `cargo build --offline && echo BUILD_OK`
- [ ] No unauthenticated review — verify: `cargo test --lib --offline test_drift_review_requires_operator`
- [ ] **Revert's response states intent-not-machine** — verify: `cargo test --lib --offline test_revert_response_states_intent_not_machine`
- [ ] No drift logic re-implemented — verify: `grep -c "last_good_version\|content_hash(" crates/uaa-control/src/operator/handlers.rs` returns **0**
- [ ] No `record()` double-log — verify: `grep -c "audit::record(" crates/uaa-control/src/operator/handlers.rs` returns **0**
- [ ] Anti-over-suppression: the happy paths work — verify: `cargo test --lib --offline test_accept_on_drifted_object_succeeds test_drift_list_empty_is_200_not_404`
- [ ] No overclaim — verify: `grep -ci "tamper-proof\|cannot be tampered" crates/uaa-control/src/operator/handlers.rs crates/uaa-control/src/operator/api_types.rs` returns **0**
- [ ] No SQL — verify: `git diff origin/main | grep -c "CREATE TABLE\|SQL_"` returns **0**
- [ ] `cargo clippy --offline -- -D warnings` clean
- [ ] File headers bumped — verify: `git diff origin/main --name-only | xargs -I{} grep -l "last-edited: 2026-07" {}`

## Commit message

```
feat(control): add /api/drift review routes (DS-OPS-02)

A thin HTTP layer over DS-REG-05's scan_drift/accept_drift/revert_drift, wired
into DS-OPS-01's role groups: list at Viewer, accept/revert at Operator, actor
from Extension<auth::Session>. No drift logic is re-implemented here, and no
record() call is added — accept/revert already audit via append_in_txn.

The revert response carries a normative note: revert restores INTENT, not the
machine. v1 has no re-render, so the deployed host stays as drifted as it was
and re-deploying is a separate action. An operator reading "reverted" as "fleet
fixed" is the failure this wording prevents.

An empty drift list is 200 [] — the healthy answer, not a 404.

Co-Authored-By: Claude <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

**Polarity: additive.** If `grep -n '"/api/drift"' crates/uaa-control/src/operator/handlers.rs` hits, this task is already applied — run the acceptance checks instead of re-applying. Rollback = revert the single commit; DS-OPS-01's routes and DS-REG-05's functions survive, drift is simply no longer reviewable over HTTP, and no data or schema is touched. DS-OPS-01 also owns `handlers.rs`/`api_types.rs` — this task rebases after it merges; see the collision table in `../BREAKDOWN-2026-07-16.md`.
