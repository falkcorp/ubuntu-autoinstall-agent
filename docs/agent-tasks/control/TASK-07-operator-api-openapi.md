<!-- file: docs/agent-tasks/control/TASK-07-operator-api-openapi.md -->
<!-- version: 1.0.0 -->
<!-- guid: 78735539-72b1-47f9-80b0-feb1222d7bcb -->
<!-- last-edited: 2026-07-10 -->

# TASK-07 — Fill operator/*: JSON API (axum+utoipa, /api/openapi.json) + rust-embed SPA hosting (ws2-control)

**Priority:** P2 · **Effort:** M · **Recommended subagent:** Sonnet-class · rust-service subagent · **Why:** REST surface + RBAC wiring across ~15 operations — mechanical breadth, but every mutating route must carry the right role guard and audit call. · **Depends on:** TASK-01 + TASK-03 (wave-5 gated: CT-01, CT-03 AND the rest of global wave 4 — CT-02/04/05/06, IP-01..03, PK-01 — merged, so every handler you wire exists)

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/control-operator-api-openapi" -b agent/control-operator-api-openapi origin/main
cd "$REPO/.worktrees/control-operator-api-openapi"
git rebase origin/main
```

(Protocol also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

## Goal

Fill the CT-01 stubs `crates/uaa-control/src/operator/{mod,handlers,api_types}.rs` (your EXCLUSIVE files) with the spec C3 operator plane (:8443): an axum JSON API documented via utoipa at `GET /api/openapi.json`, RBAC-wrapped with CT-03's `require_role` (reads = Viewer, mutations = Operator, destructive/credential ops = Admin), every mutating handler calling `audit::record`, and the SPA served from `web/dist` via **rust-embed** (Decision 19) — building green even when `web/dist` contains only the CT-08 `.gitkeep` placeholder.

The ~15 operations (route table is normative for this brief):

| Method+path | Calls (already merged) | Min role |
|---|---|---|
| GET /api/machines · GET /api/machines/{mac} | CT-02 store (incl. per-layer boot target + `consistent` flag via CT-06 helpers) | Viewer |
| POST /api/machines/{mac}/approve | CT-05 `approve_machine` SAGA | Operator |
| POST /api/machines/{mac}/reinstall | CT-06 `handle_reinstall` | Operator |
| POST /api/machines/{mac}/deregister | CT-02 store | Admin |
| GET /api/enrollments · POST /api/enrollments/{fp}/approve · POST /api/enrollments/{fp}/reject | PK-01 enroll/ca | Viewer / Operator / Operator |
| GET /api/discovered · POST /api/discovered/{mac}/dismiss | CT-02 store (discovered_macs) | Viewer / Operator |
| GET /api/audit · GET /api/audit/verify | CT-04 list + `verify_chain` | Viewer |
| GET /api/luks-credentials/{mac} | CT-02 store | Viewer |
| GET /api/yubikeys · GET /api/tang-servers | CT-02 store | Viewer |
| GET /api/openapi.json · GET /healthz | utoipa doc / static | public (no auth) |
| GET /* (SPA fallback) | rust-embed asset or index.html | public |

Purely additive to the three operator files (+ mounting the router on the :8443 scaffold in `listeners.rs` — one `merge` line). Reuse — do not invent parallels: CT-03 `require_role` (do NOT write a second auth check), CT-04 `audit::record` (every mutation), CT-02 `RegistryStore`/`MemRegistryStore`, CT-05/CT-06 drivers, `axum`/`utoipa`/`rust-embed` from `[workspace.dependencies]` only.

## Background (verify before editing)

- Spec: C3 "Operator plane" (axum + utoipa, OAuth cookie, RBAC middleware, SPA via rust-embed), Decision 3 (JSON+OpenAPI locked), Decision 19 (`web/dist` CI-built, never hand-edited).
- Edge semantics (spell twice — here and Step 3): unknown mac/fingerprint on a single-resource GET → 404 JSON `{error:"not found"}`; list endpoints with nothing → `200 []` (empty collection, never 404); malformed body → 422 with the serde message; a refused reinstall (`RefusalReason`) → 409 carrying the typed reason; RBAC failures are CT-03's 401/403 — do not duplicate or re-map them.
- rust-embed: `#[derive(RustEmbed)] #[folder = "$CARGO_MANIFEST_DIR/../../web/dist"]` — the folder MUST exist at compile time; CT-08 (merged in global wave 2) committed `web/dist/.gitkeep`. SPA fallback: serve the exact asset when the path matches; otherwise serve `index.html` when embedded; when `index.html` is ABSENT (placeholder-only dist) → 503 JSON `{error:"SPA not built — run cd web && npm run build"}`; `/api/*` paths NEVER fall through to the SPA handler.
- Handlers take the trait objects (stores/drivers) via axum `State` — tests construct the router with `MemRegistryStore` + the sibling mocks and drive it with `tower::ServiceExt::oneshot`; **no live CockroachDB, no network, no real GitHub** (use CT-03's mock `GithubApi` + a minted test cookie).
- utoipa: annotate every route + schema (`#[derive(ToSchema)]` on api_types), aggregate in an `#[derive(OpenApi)]` doc; `/api/openapi.json` returns it.

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
  grep -n "Operator plane (:8443)" docs/specs/constellation-design.md   # expect: 1 hit (~line 455 — the component contract)
  grep -n "rust-embed" docs/specs/constellation-design.md               # expect: 2 hits (~lines 210, 457)
  grep -n "utoipa" Cargo.toml                                           # expect: 1+ hits (workspace dep added by CP-02)
  test -f crates/uaa-control/src/operator/mod.rs && echo OK             # expect: OK (wave gate: CT-01 merged; missing = STOP, too early)
  test -f web/dist/.gitkeep && echo OK                                  # expect: OK (wave gate: CT-08 merged; missing = STOP and report)
  grep -n "fn require_role" crates/uaa-control/src/auth.rs              # expect: 1 hit at execution time (wave gate: CT-03 merged; missing = STOP)
  ```

## Step-by-step

1. Run the ⛔ START HERE block, then every anchor grep above. Any zero-hit / missing-file result → STOP and report.

2. **`operator/api_types.rs`** — request/response DTOs for every route in the table, all `#[derive(Serialize, Deserialize, ToSchema)]`; error body type `ApiError { error: String }`; map `RefusalReason` → 409 payload here.
3. **`operator/handlers.rs`** — one `async fn` per route, thin: deserialize → call the merged driver/store → map edge cases exactly as pinned in Background (404 single / `200 []` list / 422 malformed / 409 refusal); every mutating handler calls `audit::record(actor-from-session, …)` BEFORE returning success; utoipa `#[utoipa::path(...)]` on each.
4. **`operator/mod.rs`** — `pub fn router(state: OperatorState) -> axum::Router`: mount the table with `require_role(Viewer|Operator|Admin)` per row; `/api/openapi.json` + `/healthz` unauthenticated; rust-embed SPA fallback LAST with the `/api/*` exclusion and the 503-when-unbuilt rule; `#[derive(OpenApi)]` aggregating all paths/schemas. Mount on the :8443 scaffold (one `merge` line in `listeners.rs`).
5. **Unit tests** (oneshot requests, mocks throughout): `test_openapi_served` (200 + parses as JSON + contains `"/api/machines"`), `test_machines_list_empty_200`, `test_machine_get_unknown_404`, `test_malformed_body_422`, `test_reinstall_refusal_409_typed`, `test_viewer_cannot_mutate_403` (Viewer cookie on POST approve), `test_unauthenticated_api_401`, `test_mutation_records_audit` (approve with Operator cookie → mock audit sink recorded exactly 1 event), `test_spa_fallback_503_when_unbuilt` (placeholder dist → 503 with the build hint), `test_api_never_falls_through_to_spa` (`GET /api/nope` → 404 JSON, not index.html), and the anti-over-suppression test `test_operator_approve_end_to_end_200` — a valid Operator session POSTs approve, RBAC admits it, the mock SAGA runs, audit records, response 200 (the guard stack does not block the legitimate mutation).
6. Bump the file header (version + last-edited) on every file you touch; new files get a fresh 4-line header with a new uuid4 (`uuidgen | tr 'A-F' 'a-f'`).

## How to test

```bash
cargo test --lib --offline && cargo build --offline
# Expected: all tests pass (baseline 311 + prior control tests + the ~11 new tests), build clean
cargo clippy --offline -- -D warnings
# Expected: no warnings
grep -c "utoipa::path" crates/uaa-control/src/operator/handlers.rs
# Expected: 15+ (every route documented)
grep -n "require_role" crates/uaa-control/src/operator/mod.rs
# Expected: 1+ hits per guarded route group (RBAC from CT-03, not re-implemented)
```

## Acceptance criteria

- [ ] Only the three operator files (+ one `listeners.rs` merge line) changed: `git diff origin/main --stat` confirms.
- [ ] OpenAPI live: `test_openapi_served` passes; `grep -c "utoipa::path" crates/uaa-control/src/operator/handlers.rs` ≥ 15.
- [ ] RBAC per row: `test_viewer_cannot_mutate_403` + `test_unauthenticated_api_401` pass; `grep -rn "fn verify_session\|hmac" crates/uaa-control/src/operator/` → 0 hits (auth logic lives only in CT-03's auth.rs).
- [ ] Audit on every mutation: `test_mutation_records_audit` passes; `grep -c "audit" crates/uaa-control/src/operator/handlers.rs` ≥ number of mutating routes (5).
- [ ] Edge conventions: `test_machines_list_empty_200`, `test_machine_get_unknown_404`, `test_malformed_body_422`, `test_reinstall_refusal_409_typed` pass.
- [ ] SPA rules: `test_spa_fallback_503_when_unbuilt` + `test_api_never_falls_through_to_spa` pass; build is green with the `.gitkeep`-only dist (`cargo build --offline` on a clean checkout proves it).
- [ ] **Anti-over-suppression:** `test_operator_approve_end_to_end_200` passes — a legitimate Operator mutation clears RBAC + audit and succeeds.
- [ ] `cargo test --lib --offline && cargo build --offline` exits 0; clippy clean; no test opens a network connection.
- [ ] File headers bumped on every changed file (`git diff origin/main --stat` files each show a version bump; guids unchanged).

## Commit message

```
feat(control): operator JSON API (axum+utoipa) + RBAC wiring + rust-embed SPA hosting (ws2-control)

Fills the CT-01 operator/* stubs per spec C3: ~15 documented routes
(/api/openapi.json via utoipa) over the merged CT-02 store, CT-05 SAGA,
CT-06 reinstall, PK-01 enrollment, and CT-04 audit — every mutation wrapped
in CT-03 require_role (Viewer/Operator/Admin per route) and recorded to the
audit chain; pinned edge conventions (404 single, 200-empty list, 422
malformed, 409 typed refusal); web/dist embedded via rust-embed with a 503
SPA-not-built fallback and a hard /api exclusion. Oneshot router tests on
mocks — no DB, no network.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

If `grep -n "openapi.json" crates/uaa-control/src/operator/mod.rs` hits, already applied — run the acceptance checks instead of re-applying. Rollback = revert the single commit; the operator files return to CT-01's header-only stubs and the :8443 scaffold to health-only; nothing outside the crate changes.
