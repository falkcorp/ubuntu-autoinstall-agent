<!-- file: docs/agent-tasks/operator-api/TASK-01-api-profiles-routes.md -->
<!-- version: 1.0.0 -->
<!-- guid: 8cc152c3-bec6-4707-b15b-42bff967beb1 -->
<!-- last-edited: 2026-07-16 -->

# TASK-01 — `/api/profiles` route group + DTOs (DS-OPS-01)

**Priority:** P2 · **Effort:** M · **Recommended subagent:** Sonnet-class · rust-control subagent · **Why:** auth wiring is load-bearing — a route added outside the role-grouping convention is silently unauthenticated. · **Depends on:** DS-REG-02 (needs `ProfileStore`)

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/operator-api-api-profiles-routes" -b agent/operator-api-api-profiles-routes origin/main
cd "$REPO/.worktrees/operator-api-api-profiles-routes"
git rebase origin/main
```

(Protocol also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

**Wave gate:** DS-REG-02 must be merged. If `grep -n "pub trait ProfileStore" crates/uaa-control/src/profiles/store.rs` returns 0 hits, the gate is not met: STOP and report.

## Goal

Add a `/api/profiles` + `/api/groups` route group to the operator plane (`:15000`) with CRUD over `HostGroupProfile`/`HostProfile`, plus `POST /api/groups/:name/rebind`, wired into the existing role-grouping and audit conventions.

REUSE — do not invent parallels:

- **`build_router`'s role-grouping convention is mandatory** — verify: `grep -n "fn build_router" crates/uaa-control/src/operator/handlers.rs`. The shape: one `Router` per minimum `Role`, each `.with_state(...)`, each wrapped in `auth::require_role(router, Role::X)`, all `.merge()`d, with auth `Extension`s layered **once** at the top. A route added outside this shape is **unauthenticated** — this is exactly the class of bug PR #92 fixed when `auth.rs` existed but was unmounted.
- **`auth::require_role`** — verify: `grep -n "pub fn require_role" crates/uaa-control/src/auth.rs`. Reads at `Role::Viewer`, mutations at `Role::Operator`.
- **`Extension<auth::Session>`** for the actor — mirror `handle_approve_enrollment` (verify: `grep -n "async fn handle_approve_enrollment" crates/uaa-control/src/operator/handlers.rs`). Pass `&session.login` as the audit actor — **never** a placeholder string.
- **`json_response` + `ApiErrorBody`** — verify: `grep -n "fn json_response" crates/uaa-control/src/operator/handlers.rs`. The uniform response convention.
- **DTOs are hand-written `Serialize`-only structs in `api_types.rs`**, mirroring `web/src/api/types.ts` — verify: `grep -n "pub struct MachineRow" crates/uaa-control/src/operator/api_types.rs`. **Never re-export a `db::` row type.**
- **`profile::validate`** (DS-PRF-03) on every write; **`ProfileStore`** (DS-REG-02) for persistence.

## Background (verify before editing)

- **The SPA already has a drift vocabulary.** `web/src/api/types.ts`'s `MachineRow` carries `consistent: boolean` — *"True when every provisioning layer for this machine agrees; false = drift"*. Align profile DTO naming with it; do **not** invent a second word for the same idea.
- **`AppState`** gains a `profile_store` field. Follow the crate's **isolate-failure-per-request** principle — verify: `grep -n "ca_dir" crates/uaa-control/src/operator/handlers.rs` and read the doc explaining why the CA is loaded lazily per request rather than at router construction, "so a CA problem doesn't take down the whole operator plane". A store problem must likewise fail **the request**, not router construction.
- **⚠ Fail-closed on an unreadable store — do NOT copy the crate's degrade-to-Mem habit.** `default_state()` builds `MemEnrollmentStore`/`MemAuditStore` in production, and `default_auth_state()` falls back to an ephemeral key rather than failing. That convention is **wrong here**: an empty profile store means allocation re-allocates every index from 1 and the fleet renames itself (spec D8). If the store cannot be read, profile routes return **503**. Never construct a `MemProfileStore` in production — DS-REG-02 `#[cfg(test)]`-gated it so this cannot compile, and you must not work around that.
- **`rebind` is `Role::Operator`-gated and audited** (spec D18) — it is the NIC-replacement runbook.
- Edge semantics (spelled out here AND in acceptance):
  - **Validation failure on a write** → `400` with `ApiErrorBody` naming **every** violated rule (DS-PRF-03 collects them all). Never just the first.
  - **`DELETE /api/groups/:name` on `standalone`** → `400`; it is undeletable (spec D3).
  - **A rename attempt** (PUT with a different name for the same id) → `400`; names are immutable (spec D2).
  - **`GET` on a nonexistent group** → `404`, not an empty object.
  - **`rebind` with an unbound `old_identity`** → `400` naming it. Never silently allocate instead.

**HARD RULES (non-negotiable):**
- **NO SQL, NO migration** — no DB connection exists in production (spec D4).
- NO hardware actions. NEVER wipe/write/deploy on 172.16.2.30 or len-serv-003. NEVER power on unimatrixone.
- Every mutating route MUST be `require_role(.., Role::Operator)`-wrapped and take `Extension<auth::Session>`. An unauthenticated mutation is the worst defect this task can ship.
- Do NOT construct `MemProfileStore` outside tests.
- Stay inside your worktree; never `git push`, `gh pr`, or merge — report done and stop.

- **Re-verify these anchors before editing** — line numbers drift; zero hits = STOP and report:
  ```bash
  grep -n "pub trait ProfileStore" crates/uaa-control/src/profiles/store.rs
  # expect: 1 hit — DS-REG-02 merged (0 hits = wave gate not met, STOP)
  grep -n "fn build_router" crates/uaa-control/src/operator/handlers.rs
  # expect: 1 hit (~line 479) — THE route-grouping shape to mirror
  grep -n "let viewer_routes = auth::require_role\|let operator_routes = auth::require_role" crates/uaa-control/src/operator/handlers.rs
  # expect: 2 hits (~lines 495, 507) — copy this wiring exactly
  grep -n "async fn handle_approve_enrollment" crates/uaa-control/src/operator/handlers.rs
  # expect: 1 hit (~line 629) — the audited-mutation handler shape (Extension<Session> -> &session.login)
  grep -n "struct AppState" crates/uaa-control/src/operator/handlers.rs
  # expect: 1 hit (~line 345) — add profile_store here
  grep -n "pub struct MachineRow" crates/uaa-control/src/operator/api_types.rs
  # expect: 1 hit (~line 17) — the Serialize-only DTO convention
  ```

## Step-by-step

1. In `api_types.rs`, add `Serialize`-only DTOs: `HostGroupView`, `HostProfileView`, `AllocationView`. Mirror `MachineRow`'s style; do **not** re-export `db::` rows.
2. In `handlers.rs`, add `profile_store: Arc<dyn ProfileStore>` to `AppState`, constructed so a store failure fails a request rather than router construction.
3. Add routes inside `build_router`, in the correct role group:
   - `Role::Viewer`: `GET /api/groups`, `GET /api/groups/:name`, `GET /api/groups/:name/profiles`, `GET /api/groups/:name/allocations`
   - `Role::Operator`: `POST /api/groups`, `PUT /api/groups/:name`, `DELETE /api/groups/:name`, `POST /api/groups/:name/profiles`, `POST /api/groups/:name/rebind`
4. Every mutating handler: `Extension<auth::Session>` → validate via `profile::validate` → write via `ProfileStore` → `json_response`. Pass `&session.login` as the actor.
5. Keep purely additive — do not modify existing routes, `MachineRow`, or `auth.rs`.
6. Add tests in `handlers.rs`'s test module (mirror its existing route tests; `MemProfileStore` is available under `#[cfg(test)]`):
   - **`test_profile_mutations_require_operator`** — every mutating route returns 401/403 unauthenticated. *The worst defect this task could ship.*
   - `test_profile_reads_require_viewer`.
   - `test_create_group_validates` — an invalid group ⇒ 400 naming **all** violations.
   - `test_delete_standalone_rejected` ⇒ 400.
   - `test_rename_rejected` ⇒ 400.
   - `test_get_unknown_group_404`.
   - `test_rebind_audited` — `MemAuditStore` recorded the event with the session's login.
   - `test_store_unreadable_returns_503_not_empty` — an unreadable store ⇒ 503. **Never** an empty list, which would read as "no allocations" and is how the fleet renames itself.
   - **`test_valid_group_create_succeeds`** — the happy path still works with auth present.
7. Bump headers on every file you touch; keep existing guids.

**Anti-over-suppression:** the auth guard and the validator both reject, so both can over-block. `test_valid_group_create_succeeds` is the happy-path proof that an authorized operator submitting a valid group still succeeds — without it, an over-strict validator or a mis-wired role gate would make the API reject everything while all the negative tests pass.

## How to test

```bash
cargo test --lib --offline
# Expected: 634+ passed, 0 failed (baseline + DS-REG-01/02's tests + your 9).
cargo build --offline
# Expected: exit 0.
cargo clippy --offline -- -D warnings
# Expected: no warnings.
```

## Acceptance criteria

- [ ] `cargo test --lib --offline` exits 0 — verify: `cargo test --lib --offline 2>&1 | grep -E "^test result"`
- [ ] `cargo build --offline` exits 0 — verify: `cargo build --offline && echo BUILD_OK`
- [ ] **No unauthenticated mutation** — verify: `cargo test --lib --offline test_profile_mutations_require_operator`
- [ ] Routes use the role-grouping convention — verify: `grep -c "require_role" crates/uaa-control/src/operator/handlers.rs` returns ≥3 (unchanged count + your groups)
- [ ] Actor is real, not a placeholder — verify: `cargo test --lib --offline test_rebind_audited`
- [ ] **Fail-closed on an unreadable store** — verify: `cargo test --lib --offline test_store_unreadable_returns_503_not_empty`
- [ ] `MemProfileStore` not reachable from production — verify: `grep -c "MemProfileStore" crates/uaa-control/src/operator/handlers.rs` returns **0** outside `#[cfg(test)]` blocks (inspect the hits; all must be inside a test module)
- [ ] DTOs are not re-exported db rows — verify: `grep -c "pub use crate::db::" crates/uaa-control/src/operator/api_types.rs` returns **0**
- [ ] Anti-over-suppression: the happy path works — verify: `cargo test --lib --offline test_valid_group_create_succeeds`
- [ ] No SQL — verify: `git diff origin/main | grep -c "CREATE TABLE\|SQL_"` returns **0**
- [ ] `cargo clippy --offline -- -D warnings` clean
- [ ] File headers bumped — verify: `git diff origin/main --name-only | xargs -I{} grep -l "last-edited: 2026-07" {}`

## Commit message

```
feat(control): add /api/profiles + /api/groups operator routes (DS-OPS-01)

CRUD over host groups and profiles plus rebind, wired through build_router's
role-grouping convention: reads at Viewer, mutations at Operator, actor taken
from Extension<auth::Session> and passed to the audit as &session.login. A
route added outside that shape is silently unauthenticated — the same class of
bug PR #92 fixed when auth.rs existed but was unmounted.

Profile routes fail CLOSED (503) when the store cannot be read, deliberately
breaking from the crate's degrade-to-Mem habit: an empty profile store means
allocation re-allocates every index from 1 and the fleet renames itself.

DTOs are hand-written Serialize-only views in api_types.rs mirroring
web/src/api/types.ts, never re-exported db rows.

Co-Authored-By: Claude <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

**Polarity: additive.** If `grep -n '"/api/groups"' crates/uaa-control/src/operator/handlers.rs` hits, this task is already applied — run the acceptance checks instead of re-applying. Rollback = revert the single commit; existing routes, `auth.rs`, and the SPA are untouched, and any profile data written through these routes stays in the snapshot (additive collections, harmless when unread). DS-OPS-02 also edits `handlers.rs` and `api_types.rs` — see the collision table in `../BREAKDOWN-2026-07-16.md`.
