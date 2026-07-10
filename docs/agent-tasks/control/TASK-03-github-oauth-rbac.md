<!-- file: docs/agent-tasks/control/TASK-03-github-oauth-rbac.md -->
<!-- version: 1.0.0 -->
<!-- guid: b6930883-cd4c-4549-9194-8e4d9ffa0c7e -->
<!-- last-edited: 2026-07-10 -->

# TASK-03 — Fill auth.rs: GitHub OAuth web flow + org/team RBAC middleware + signed session cookies (ws2-control)

**Priority:** P1 · **Effort:** M · **Recommended subagent:** Sonnet-class · rust-service subagent · **Why:** auth logic; fail-closed-to-viewer on GitHub API failure per spec C3 — a wrong default here silently grants mutation rights. · **Depends on:** TASK-01 (wave-4 gated: CT-01 merged — the `auth.rs` stub must exist on origin/main)

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/control-github-oauth-rbac" -b agent/control-github-oauth-rbac origin/main
cd "$REPO/.worktrees/control-github-oauth-rbac"
git rebase origin/main
```

(Protocol also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

## Goal

Fill the CT-01 stub `crates/uaa-control/src/auth.rs` (your EXCLUSIVE file) with the spec Decision-8 operator auth: GitHub OAuth web flow (authorize redirect → code→token exchange → user + org/team lookup), RBAC mapping (`uaa-admins` team → Admin, `uaa-operators` team → Operator, org member → Viewer, everyone else → 403), role cache with 5-minute TTL, **fail-closed to Viewer on any GitHub API failure** (mutations denied, reads allowed), and HMAC-signed session cookies. NO WebAuthn, NO local accounts, NO break-glass login (Decision 8 — locked); the sanctioned GitHub-outage emergency path is DOCUMENTED, not built: a module-level doc section describing the direct `cockroach sql` mutation + mandatory `uaa-control audit backfill` (Decision 8 repair — CT-04 ships the backfill command).

Purely additive to `auth.rs`. Reuse — do not invent parallels:
- **`reqwest` with rustls** (already a workspace dep — verify: `grep -n "rustls-tls" Cargo.toml`) for GitHub API calls, behind a trait so tests never touch the network. Do NOT add a new HTTP client crate.
- **`ring::hmac`** (ring is already a dep — verify: `grep -n "^ring" Cargo.toml`) for cookie signing. Do NOT add a cookie/JWT crate.
- **Row/role types**: if CT-01's `db/mod.rs` already defines a `Role` enum, use it; otherwise define `pub enum Role { Admin, Operator, Viewer }` here (check first: `grep -n "enum Role" crates/uaa-control/src/db/mod.rs`).

## Background (verify before editing)

- Spec: Decision 8 (+ its repair) and component C3 "Operator plane". RBAC is org-team based; role cache TTL 5 min. OAuth app client-id/secret arrive via config/env (`UAA_GITHUB_CLIENT_ID` / `UAA_GITHUB_CLIENT_SECRET`) — NEVER committed; the OAuth app itself is Bucket-3 human work.
- Edge-case semantics (spell twice — here and Step 4): missing cookie → 401 JSON for `/api/*`, 302-to-login for browser paths; bad signature or expired session → 401 (treat exactly like missing — no oracle); GitHub API error/timeout during role refresh → role degrades to Viewer for that request (NEVER serve a stale cached Admin past its TTL, NEVER error the read path); user not in the org → 403 at login completion (no session minted).
- Session cookie: `uaa_session=<base64(payload)>.<base64(hmac_sha256(payload))>`; payload = JSON `{login, role, exp}` (exp = unix seconds, 24h); `Secure; HttpOnly; SameSite=Lax`. HMAC key: 32 random bytes generated at first start, persisted 0600 under the state dir (path from config so tests use a tempdir).
- The middleware is consumed by CT-07 (operator plane) — export `pub fn require_role(min: Role) -> …` (axum layer/extractor) with a documented contract: mutating routes wrap with `require_role(Role::Operator)`, read routes `require_role(Role::Viewer)`.
- Everything GitHub-shaped goes through `pub trait GithubApi: Send + Sync { async fn exchange_code(&self, code: &str) -> Result<String>; async fn user_login(&self, token: &str) -> Result<String>; async fn org_role(&self, token: &str, login: &str) -> Result<OrgMembership>; }` — real impl uses reqwest; tests use a mock. `OrgMembership { org_member: bool, teams: Vec<String> }`.

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
  grep -n "uaa-admins" docs/specs/constellation-design.md         # expect: 1+ hits (~line 122; the RBAC mapping, normative)
  grep -n "fail-closed to viewer" docs/specs/constellation-design.md  # expect: 2 hits (~lines 123, 456)
  grep -n "audit backfill" docs/specs/constellation-design.md     # expect: 1 hit (~line 127; the documented-not-built hatch)
  grep -n "rustls-tls" Cargo.toml                                 # expect: 1 hit (reqwest rustls feature)
  grep -n "^ring" Cargo.toml                                      # expect: 1 hit (hmac for cookie signing)
  test -f crates/uaa-control/src/auth.rs && echo OK               # expect: OK (wave gate: CT-01 merged; missing = STOP, too early)
  ```

## Step-by-step

1. Run the ⛔ START HERE block, then every anchor grep above. Any zero-hit / missing-file result → STOP and report.

2. **Types + config.** `Role` (Admin > Operator > Viewer, `PartialOrd` by that order), `AuthConfig { client_id, client_secret, org: String, admin_team: String /* "uaa-admins" */, operator_team: String /* "uaa-operators" */, state_dir: PathBuf }` (defaults from env; team names configurable but defaulting to the spec's).
3. **Session cookies.** `mint_session(key, login, role, now) -> String` and `verify_session(key, cookie, now) -> Option<Session>` — HMAC-SHA256 via `ring::hmac` over the exact payload bytes; `None` on bad b64, bad sig, or `exp <= now` (one indistinguishable failure path). `load_or_create_hmac_key(state_dir) -> Result<[u8; 32]>` — 0600, tempdir-tested.
4. **OAuth flow handlers** (exported for CT-07 to mount): `GET /auth/login` → 302 to `https://github.com/login/oauth/authorize` with client_id + random `state` (state stored in a short-TTL in-memory map, verified on callback — reject mismatches); `GET /auth/callback?code&state` → `GithubApi::exchange_code` → `user_login` → `org_role` → map role: `teams` contains admin_team → Admin, else operator_team → Operator, else `org_member` → Viewer, else → 403 with no cookie; on success mint cookie + 302 to `/`. Repeat the edge semantics: any `GithubApi` error at LOGIN → 502 with no cookie (a login must never default-grant); any error at role REFRESH (existing session past the 5-min cache) → Viewer for that request, session kept.
5. **RBAC middleware.** `require_role(min: Role)` — extracts + verifies the cookie (missing/bad/expired → 401 JSON `{error:"unauthenticated"}` for paths starting `/api/`, 302 `/auth/login` otherwise); refreshes the role via `GithubApi` when the per-login cache entry is older than 5 min (cache = `HashMap<login, (Role, Instant)>` behind a Mutex); compares refreshed role to `min` → 403 JSON `{error:"forbidden"}` when insufficient. Documented-not-built hatch: module doc `//! ## Emergency access during a GitHub outage` describing the `cockroach sql` + `uaa-control audit backfill` procedure and explicitly stating no code path implements login bypass.
6. **Unit tests** (mock `GithubApi`, tempdir key, no network): `test_cookie_round_trip`, `test_cookie_bad_sig_rejected`, `test_cookie_expired_rejected`, `test_role_mapping_admin_team` / `test_role_mapping_operator_team` / `test_role_mapping_org_member_viewer` / `test_role_mapping_non_member_403`, `test_oauth_state_mismatch_rejected`, `test_login_github_error_no_cookie` (502, nothing minted), `test_refresh_failure_degrades_to_viewer` (cached Admin + failing mock past TTL → mutation denied, read allowed), `test_cache_within_ttl_skips_github` (mock call-count stays 1), and the anti-over-suppression test `test_operator_mutation_passes_guards` (valid Operator session + healthy mock → `require_role(Operator)` admits the request; the fail-closed stack does not block a legitimate mutation).
7. Bump the file header (version + last-edited) on every file you touch; new files get a fresh 4-line header with a new uuid4 (`uuidgen | tr 'A-F' 'a-f'`).

## How to test

```bash
cargo test --lib --offline && cargo build --offline
# Expected: all tests pass (baseline 311 + prior control tests + the ~12 new tests), build clean
cargo clippy --offline -- -D warnings
# Expected: no warnings
grep -rn "webauthn\|WebAuthn\|password" crates/uaa-control/src/auth.rs | grep -vi "client_secret\|comment\|//"
# Expected: 0 hits (GitHub OAuth ONLY — Decision 8)
grep -n "UAA_GITHUB_CLIENT_SECRET" crates/uaa-control/src/auth.rs
# Expected: 1+ hits (env-sourced), and the literal secret value appears nowhere
grep -n "Secure; HttpOnly; SameSite=Lax\|Secure" crates/uaa-control/src/auth.rs | head -2
# Expected: 1+ hits (cookie attributes pinned on the Set-Cookie builder)
cargo test --lib --offline test_refresh_failure_degrades_to_viewer
# Expected: 1 passed (the fail-closed-to-Viewer law specifically)
```

## Acceptance criteria

- [ ] Only `auth.rs` (+ optional `lib.rs` re-export line) changed: `git diff origin/main --stat` shows no other `crates/uaa-control/src/` file.
- [ ] RBAC mapping proven: the four `test_role_mapping_*` tests pass; team names default to `uaa-admins`/`uaa-operators` (`grep -n "uaa-admins" crates/uaa-control/src/auth.rs` → 1+ hits).
- [ ] Fail-closed proven: `test_refresh_failure_degrades_to_viewer` and `test_login_github_error_no_cookie` pass — no code path grants above Viewer on GitHub failure.
- [ ] Cookie integrity proven: `test_cookie_bad_sig_rejected` + `test_cookie_expired_rejected` pass; HMAC key file created 0600 (asserted in `test`).
- [ ] **Anti-over-suppression:** `test_operator_mutation_passes_guards` passes — a legitimate Operator still gets through the guard stack.
- [ ] Emergency hatch documented, not built: `grep -n "Emergency access during a GitHub outage" crates/uaa-control/src/auth.rs` → 1 hit; `grep -rn "break_glass\|break-glass" crates/uaa-control/src/` → 0 code hits.
- [ ] `cargo test --lib --offline && cargo build --offline` exits 0; clippy clean; no test opens a network connection.
- [ ] File headers bumped on every changed file (`git diff origin/main --stat` files each show a version bump; guids unchanged).

## Commit message

```
feat(control): GitHub OAuth web flow + org/team RBAC + signed session cookies (ws2-control)

Fills the CT-01 auth.rs stub per spec Decision 8: authorize/callback flow
behind a mockable GithubApi trait, uaa-admins->Admin / uaa-operators->Operator
/ org-member->Viewer mapping with a 5-min role cache, fail-closed-to-Viewer
on any GitHub API failure (mutations denied, reads allowed), ring-HMAC
session cookies (0600 key, tempdir-tested), require_role middleware for the
operator plane, and the documented-not-built cockroach-sql + audit-backfill
emergency hatch. 12 unit tests, zero network.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

If `grep -n "fn require_role" crates/uaa-control/src/auth.rs` hits, already applied — run the acceptance checks instead of re-applying. Rollback = revert the single commit; `auth.rs` returns to CT-01's header-only stub, no sessions/keys/server state exist to unwind (the HMAC key file is only ever created at daemon runtime).
