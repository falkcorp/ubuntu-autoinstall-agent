<!-- file: docs/agent-tasks/uaa-web/TASK-01-web-crate-static-serve.md -->
<!-- version: 1.0.0 -->
<!-- guid: 13e451c0-eb29-4182-b872-cf8e3a9b294d -->
<!-- last-edited: 2026-07-10 -->

# TASK-01 — Create the uaa-web crate: :8081 read-only ServeDir behind an explicit path allowlist + health + :7445 mTLS gRPC listener, with headered stubs for placement/iso/publish (ws5-web)

**Priority:** P1 · **Effort:** M · **Recommended subagent:** Sonnet-class · rust-service subagent · **Why:** new service skeleton; allowlist mirrors nginx locations (SPA-catch-all hazard) — well-bounded new crate, no existing code modified. · **Depends on:** none within this workstream (wave-6 gated: ALL of waves 1–5 merged — specifically CP-02 for the `uaa-proto` crate + `[workspace.dependencies]`, and PK-03 for `crates/uaa-core/src/tls.rs`)

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/uaa-web-web-crate-static-serve" -b agent/uaa-web-web-crate-static-serve origin/main
cd "$REPO/.worktrees/uaa-web-web-crate-static-serve"
git rebase origin/main
```

(Protocol also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

## Goal

Create the NEW crate `crates/uaa-web` (spec `docs/specs/constellation-design.md` §C4, topology table): the boot-artifact daemon that (a) serves the webroot **read-only** on `:8081` plain HTTP through an **explicit path-prefix allowlist** (Decision 20 — UEFI HTTP-boot/iPXE can't practically TLS), (b) exposes the `WebService` gRPC plane on `:7445` under mTLS using its Decision-23 service cert, and (c) pre-creates the three stub modules `src/placement.rs`, `src/iso_jobs.rs`, `src/publish.rs` that WB-02/03/04 fill EXCLUSIVELY (stub-pattern collision row: each stub file has exactly one filling task). Purely additive: no file outside `crates/uaa-web/**` changes except the root `Cargo.toml` is NOT touched (CP-01's `members = ["crates/*"]` glob picks the crate up automatically — adding a crate never edits the members list, Decision 17).

REUSE — do not invent parallels for any of these:

- **`uaa-proto`** (`crates/uaa-proto`, from CP-02) for the generated `WebService` tonic server trait (`proto/uaa/web/v1/web.proto`). Do NOT hand-write proto types or a second .proto file.
- **mTLS helpers** in `crates/uaa-core/src/tls.rs` (from PK-03) to build the `:7445` server TLS config from `/var/lib/uaa/certs/uaa-web.{key,crt}` + the install-CA + CRL check. Do NOT write new rustls plumbing.
- **`[workspace.dependencies]`** (populated by CP-02): reference `axum`, `tower-http`, `tonic`, `tokio` etc. with `workspace = true` ONLY. Do NOT pin new versions in the crate manifest.
- The allowlist mirrors today's nginx `location` blocks — hazard documented in `unimatrixone-pxe-boot-status.md` (~line 581): the media site has a generic SPA catch-all at `location /`, so new paths MUST live under explicit prefixes. The allowlist here is the Rust mirror of that rule.

**HARD RULES (non-negotiable):**
- NO hardware actions. Validate ONLY in-repo (`cargo`) and, where a brief says so, the QEMU+swtpm harness (`scripts/vm-validate.sh`). Code that COULD touch hardware is written and unit-tested against mock executors only.
- NEVER wipe, write to, or deploy on 172.16.2.30 ("the server") or len-serv-003.
- `disk_device` is read from the live target at runtime, never guessed or hardcoded.
- ipmitool runs via `ssh 172.16.2.30`, never on macOS.
- NEVER power on unimatrixone (U1).
- No real secret in any file: `REPLACE_AT_PLACE_TIME` placeholders stay placeholders.
- Stay inside your worktree; never `git push`, `gh pr`, or merge — report done and stop.

## Background (verify before editing)

- Topology (spec §"Constellation topology"): uaa-web runs on the server, ports `:8081` HTTP (boot artifacts, read-only) and `:7445` gRPC mTLS; owns `/var/www/html` writes. It "fail-closes loudly if its bind is taken" (Decision 20 — CRDB's admin UI commonly sits on :8080, adjacent to :8081).
- The allowlist prefixes (mirror of today's nginx explicit locations): `/ipxe`, `/ubuntu`, `/ubuntu-arm64`, `/isos`, `/cloud-init`, `/uaa`, plus exactly `/healthz`. Anything else (including `/`) → 404. Only `GET`/`HEAD` are served → anything else 405.
- Webroot and cert paths must be constructor parameters (config struct), NOT hardcoded, so tests run against a tempdir with no network and no real certs.
- The three stubs return `tonic::Status::unimplemented(...)` until their filler tasks land; the tonic service impl in the server module DELEGATES each RPC to a free function in its stub module — that is what keeps WB-02/03/04 out of each other's files.
- **Path map:** after CP-01 (wave 1) merges, `src/**` lives at `crates/uaa-core/src/**` and the CLI at `crates/uaa/src/**`. The greps below cite pre-move paths (verifiable on today's main); at execution time run them at the old path, then the mapped path. Zero hits at BOTH = STOP and report.
- **Re-verify these anchors before editing** — line numbers drift; zero hits at both old and mapped path = STOP and report:
  ```bash
  grep -n "location" unimatrixone-pxe-boot-status.md | head -3   # expect: hits (explicit location blocks required; ~lines 399, 581-582)
  grep -n "pub trait CommandExecutor" src/network/executor.rs    # expect: 1 hit (mapped: crates/uaa-core/src/network/executor.rs)
  # Post-merge greps (these files exist ONLY after their waves merged — run at execution time):
  grep -rn "service WebService" proto/uaa/web/v1/web.proto       # expect: 1 hit (CP-02)
  grep -n "pub" crates/uaa-core/src/tls.rs | head -5             # expect: hits — PK-03's mTLS helpers; re-read for exact signatures
  grep -n 'members = \["crates/\*"\]' Cargo.toml                 # expect: 1 hit (CP-01 workspace glob)
  ```

## Step-by-step

1. Run the ⛔ START HERE block, then every anchor grep above. Any zero-hit grep (at old AND mapped path) → STOP and report.

2. **Crate skeleton** — create `crates/uaa-web/Cargo.toml` (package `uaa-web`, bin target `uaa-web`, all deps `workspace = true`: `tokio`, `axum`, `tower`, `tower-http` (ServeDir), `tonic`, `uaa-proto`, `uaa-core`, `tracing`, `serde`, `serde_json`) and `crates/uaa-web/src/main.rs`. Every new `.rs` and the `Cargo.toml` gets a fresh 4-line header (`// file: crates/uaa-web/src/main.rs`, `// version: 1.0.0`, new guid via `uuidgen | tr 'A-F' 'a-f'`, `// last-edited: 2026-07-10`; `#` comments in Cargo.toml).

3. **Config struct** — `crates/uaa-web/src/config.rs`: `pub struct WebConfig { pub webroot: PathBuf /* default /var/www/html */, pub http_addr: SocketAddr /* default 0.0.0.0:8081 */, pub grpc_addr: SocketAddr /* default 0.0.0.0:7445 */, pub cert_dir: PathBuf /* default /var/lib/uaa/certs */ }` with `Default` + env/flag overrides (`UAA_WEB_ROOT`, `UAA_WEB_HTTP_ADDR`, `UAA_WEB_GRPC_ADDR`, `UAA_WEB_CERT_DIR`). Defaults = spec values; tests always inject a tempdir.

4. **Allowlist** — `crates/uaa-web/src/allowlist.rs`:
   ```rust
   pub const ALLOWED_PREFIXES: [&str; 6] =
       ["/ipxe", "/ubuntu", "/ubuntu-arm64", "/isos", "/cloud-init", "/uaa"];
   /// True iff the request path may be served. Exact "/healthz" is allowed;
   /// otherwise the path must start with "<prefix>/" or equal the prefix.
   /// "/" and anything not under a prefix → false (no SPA-style catch-all —
   /// mirror of the nginx location-block hazard). A path containing "..",
   /// "//" or a NUL is ALWAYS false (reject before ServeDir ever sees it).
   pub fn path_allowed(path: &str) -> bool;
   ```
   Prefix matching is segment-aware: `/isosx/evil` is DENIED (must be `/isos` or `/isos/...`). Note `/ubuntu-arm64` vs `/ubuntu`: check longer prefixes work independently (segment-aware match handles it).

5. **HTTP server** — `crates/uaa-web/src/http.rs`: an axum router whose fallback handler (a) rejects any method other than `GET`/`HEAD` with 405, (b) runs `path_allowed`, 404 on false, (c) delegates allowed paths to `tower_http::services::ServeDir::new(&config.webroot)` (read-only by construction — ServeDir cannot write), (d) serves exact `/healthz` → `200 text/plain "ok"` without touching the webroot. Bind failure (`AddrInUse`) = log an ERROR naming the port and the Decision-20 port-audit note, then exit nonzero — fail-closed loud, never silent retry. Expose `pub fn router(config: &WebConfig) -> axum::Router` so tests drive it with `tower::ServiceExt::oneshot` — no sockets in tests.

6. **Stub modules** — create, each with its own fresh header:
   - `crates/uaa-web/src/placement.rs` — `pub async fn place_seed(..) / place_ipxe(..) / flip_boot_target(..) / remove_host(..)`, each `Err(tonic::Status::unimplemented("uaa-web placement: filled by uaa-web/TASK-02"))`.
   - `crates/uaa-web/src/iso_jobs.rs` — `pub async fn build_iso(..) / get_build_job(..) / list_isos(..)`, unimplemented naming TASK-03.
   - `crates/uaa-web/src/publish.rs` — `pub async fn publish_agent_binary(..)`, unimplemented naming TASK-04.
   Signatures take the uaa-proto request types + `&WebConfig` and return the proto response types, so fillers never edit the service impl.

7. **gRPC server** — `crates/uaa-web/src/grpc.rs`: implement the generated `WebService` trait; every RPC body is a one-line delegation to its stub-module function. Build the `:7445` listener with the PK-03 helpers in `crates/uaa-core/src/tls.rs` (server cert `uaa-web.crt`/`uaa-web.key` from `config.cert_dir`, client-cert verification against the install CA, CRL check — re-read tls.rs for the exact constructor; do NOT hand-roll rustls config). Missing cert files at startup = ERROR + exit nonzero (fail-closed; Decision 23 mints them at install time).

8. **main.rs** — parse config, spawn HTTP + gRPC servers, `tokio::select!` on both; either exiting = log + nonzero exit.

9. **Unit tests** (`#[cfg(test)]` in `allowlist.rs` and `http.rs`; tempdir webroot via `tempfile` if it is already a workspace dep, else `std::env::temp_dir()` + manual cleanup):

   | Test | Asserts |
   |---|---|
   | `test_allowlist_prefixes` | each of the 6 prefixes: bare (`/isos`) and child (`/isos/x.iso`) → true |
   | `test_allowlist_denies_root_and_unknown` | `/`, `/index.html`, `/media/x`, `/isosx/evil` → false |
   | `test_allowlist_denies_traversal` | `/ipxe/../secret`, `/ipxe//x`, `/uaa/a/../../y` → false |
   | `test_http_404_outside_allowlist` | oneshot `GET /` and `GET /etc/passwd` → 404, body does not leak file contents |
   | `test_http_405_non_get` | oneshot `POST /ipxe/x.ipxe`, `PUT /isos/a.iso` → 405 |
   | `test_healthz` | `GET /healthz` → 200 body `ok` with an EMPTY webroot |
   | `test_http_serves_allowlisted_file` | **anti-over-suppression / happy path:** write `ipxe/mac-aabb.ipxe` into a tempdir webroot → `GET /ipxe/mac-aabb.ipxe` → 200 with exact file bytes (the guard stack does not block legitimate boot fetches) |
   | `test_grpc_stubs_unimplemented` | calling each stub function returns `Code::Unimplemented` and the message names its filler TASK |

10. Bump the file header (version + last-edited) on every file you touch; new files get a fresh 4-line header with a new uuid4 (`uuidgen | tr 'A-F' 'a-f'`).

## How to test

```bash
cargo test --lib --offline && cargo build --offline
# Expected: all tests pass (baseline 311 + waves 1-5 additions + the 8 tests above), build clean
cargo clippy --offline -- -D warnings
# Expected: no warnings
cargo build --offline -p uaa-web
# Expected: exit 0 — the new crate builds standalone via the workspace glob
grep -rn "ServeDir" crates/uaa-web/src/
# Expected: ≥1 hit in http.rs (tower-http reuse, no hand-rolled file reader)
grep -rn "unimplemented" crates/uaa-web/src/placement.rs crates/uaa-web/src/iso_jobs.rs crates/uaa-web/src/publish.rs
# Expected: ≥1 hit per file (stubs in place for WB-02/03/04)
git -C . diff origin/main --stat
# Expected: ONLY crates/uaa-web/** paths listed (root Cargo.toml untouched — glob members)
```

## Acceptance criteria

- [ ] Crate exists and is picked up by the glob: `cargo build --offline -p uaa-web` exits 0 and `git diff origin/main --stat` shows NO change to the root `Cargo.toml`.
- [ ] Allowlist is explicit and closed: `grep -n "ALLOWED_PREFIXES" crates/uaa-web/src/allowlist.rs` → 1 definition listing exactly `/ipxe /ubuntu /ubuntu-arm64 /isos /cloud-init /uaa`; `test_allowlist_denies_root_and_unknown` and `test_allowlist_denies_traversal` pass.
- [ ] Read-only plane: `test_http_405_non_get` passes; `grep -rn "fs::write\|File::create\|OpenOptions" crates/uaa-web/src/http.rs crates/uaa-web/src/allowlist.rs` → 0 hits outside `#[cfg(test)]`.
- [ ] Health: `test_healthz` passes (200 `ok` with empty webroot).
- [ ] Stubs present, one per filler: `test_grpc_stubs_unimplemented` passes; `grep -ln "unimplemented" crates/uaa-web/src/{placement,iso_jobs,publish}.rs` → 3 files.
- [ ] mTLS listener uses PK-03 helpers: `grep -rn "tls" crates/uaa-web/src/grpc.rs | grep -i "uaa_core\|uaa-core"` → ≥1 hit, and `grep -rn "rustls::ServerConfig::builder" crates/uaa-web/src/` → 0 hits (no hand-rolled TLS).
- [ ] **Anti-over-suppression:** `test_http_serves_allowlisted_file` passes — a legitimate allowlisted boot artifact is served 200 byte-identical through the allowlist + method guards.
- [ ] `cargo test --lib --offline && cargo build --offline` exits 0; clippy clean (`cargo clippy --offline -- -D warnings`).
- [ ] File headers bumped on every changed file (`git diff origin/main --stat` files each show a version bump; new files have fresh uuid4 headers; guids unchanged elsewhere).

## Commit message

```
feat(web): add uaa-web crate — :8081 allowlisted read-only ServeDir + :7445 mTLS gRPC + filler stubs (ws5-web)

New crates/uaa-web daemon per constellation spec C4/Decisions 17/20/23: axum
fallback enforcing GET/HEAD + explicit path allowlist (/ipxe /ubuntu
/ubuntu-arm64 /isos /cloud-init /uaa, /healthz) in front of tower-http
ServeDir; fail-closed loud on bind/cert failure; tonic WebService on :7445 via
uaa-core tls.rs delegating every RPC to headered stubs placement.rs /
iso_jobs.rs / publish.rs (filled exclusively by TASK-02/03/04). 8 unit tests
incl. traversal denial and allowlisted happy path. Root Cargo.toml untouched
(crates/* glob).

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

Idempotency (additive — check for the NEW thing's presence): if `grep -n "ALLOWED_PREFIXES" crates/uaa-web/src/allowlist.rs` hits AND `test -f crates/uaa-web/src/placement.rs`, the task is already applied — run the acceptance checks instead of re-applying. Rollback = revert the single commit: it removes `crates/uaa-web/**` cleanly; the root `Cargo.toml`, `uaa-core`, `uaa-proto`, and every other crate stay untouched (glob members), and no server-side state exists (the daemon was never deployed by this task).
