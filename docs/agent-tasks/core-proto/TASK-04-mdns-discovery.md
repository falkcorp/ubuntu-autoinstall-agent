<!-- file: docs/agent-tasks/core-proto/TASK-04-mdns-discovery.md -->
<!-- version: 1.0.0 -->
<!-- guid: 4661352d-2a85-4a6d-a0bc-5cf61a042242 -->
<!-- last-edited: 2026-07-10 -->

# TASK-04 — discovery.rs: mDNS advertise (daemons only) + resolve() returning the UNION of mDNS+static candidates (ws1-core)

**Priority:** P1 · **Effort:** M · **Recommended subagent:** Sonnet-class · rust-network-library subagent · **Why:** network library logic with fail-closed empty-union semantics (spec Decision 11) · **Depends on:** TASK-02 (wave-3 gated: CP-02 MERGED — `mdns-sd` and `semver` must exist in `[workspace.dependencies]` before this compiles)

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/core-proto-mdns-discovery" -b agent/core-proto-mdns-discovery origin/main
cd "$REPO/.worktrees/core-proto-mdns-discovery"
git rebase origin/main
```

(Protocol also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

## Goal

Fill the `crates/uaa-core/src/discovery.rs` stub per spec Decision 11 and the C1 sketch (`docs/specs/constellation-design.md`): service discovery over mDNS (`_uaa._tcp.local`, TXT records `service`/`version`/`port`) with static fallback from `/etc/uaa/endpoints.yaml`. The LOCKED semantics (Decision 11 repairs — restated because they are the whole task): (a) `resolve()` returns the **UNION** of mDNS and static candidates, mDNS-sourced first — callers iterate candidates under mTLS and accept the first that authenticates; fallback is **per-endpoint-failure, never only-on-empty-browse** (a stale advertisement must not mask a valid static entry, so the static list is ALWAYS in the returned vec, not just when browse comes back empty); (b) `advertise()` exists in this library but is called ONLY by the three daemons — the client-only `uaa` CLI ships browse-only (document this on the fn, you cannot enforce callers here); (c) empty union = **hard error, never a guess** (fail-closed). Crate: `mdns-sd` (pure Rust, already in workspace deps from CP-02) — do NOT add any other mDNS/zeroconf crate. Reuse `AutoInstallError::{ConfigError, NetworkError}` from `crates/uaa-core/src/error.rs` — no new error enum. Purely additive: only discovery.rs (and lib.rs header bump if you touch it — the `pub mod discovery;` line already exists from CP-01).

## Background (verify before editing)

- API shape (from spec C1, normative):
  ```rust
  pub enum ServiceKind { Control, Web, Pxe }
  pub enum Source { Mdns, Static }
  pub struct ServiceInfo { pub service: ServiceKind, pub version: semver::Version,
                           pub host: std::net::IpAddr, pub port: u16, pub source: Source }
  pub async fn advertise(info: &ServiceInfo) -> Result<DiscoveryHandle>;   // daemons only
  pub async fn resolve(kind: ServiceKind, static_fallback: &EndpointsFile,
                       timeout: std::time::Duration) -> Result<Vec<ServiceInfo>>;
  ```
  `DiscoveryHandle` wraps the `mdns_sd::ServiceDaemon` + registered fullname; dropping it unregisters (best-effort).
- `EndpointsFile`: serde struct for `/etc/uaa/endpoints.yaml` — `endpoints: Vec<StaticEndpoint>` with `service` (string `control|web|pxe`), `host` (IP string), `port` (u16), optional `version` (defaults `"0.0.0"` — static entries may predate version knowledge). Parse failure = `ConfigError` (fail-closed); ABSENT file = empty static list (`EndpointsFile::default()`) — absence is legal, the union with mDNS may still be non-empty. Spell both in code comments and tests.
- Edge semantics, again (they appear in Step 3 AND acceptance): union ordering = all mDNS candidates first (browse order), then all static candidates NOT already present (dedupe key = `(host, port)`; when both sources yield the same host:port the mDNS entry wins for `source` labeling); `resolve()` returning an EMPTY vec is forbidden — return `Err(NetworkError)` naming the kind, the timeout, and the static file path checked.
- mDNS browse cannot run deterministically in unit tests (no multicast in CI) — the union/parse logic MUST live in pure sync functions that tests call directly; `resolve()` is a thin async composition of `browse_mdns(kind, timeout)` + `union_candidates(...)`.
- TXT record mapping: `service` = `control|web|pxe` (match on it to filter foreign `_uaa._tcp` instances), `version` = semver string (unparseable → skip that candidate with a `tracing::warn!`, never abort the browse), `port` from the SRV record.

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

**Path map:** after CP-01 (wave 1) merges, `src/**` lives at `crates/uaa-core/src/**` and the CLI at `crates/uaa/src/**`. The greps below cite pre-move paths (verifiable on today's main); at execution time run them at the old path, then the mapped path. Zero hits at BOTH = STOP and report.

- **Re-verify these anchors before editing** — line numbers drift; zero hits at both old and mapped path = STOP and report:
  ```bash
  grep -rin "mdns" src/ Cargo.toml   # expect: 0 hits on today's main. At execution time expect exactly ONE kind of hit: the `mdns-sd` entry CP-02 added to Cargo.toml [workspace.dependencies]; `grep -rin "mdns" crates/uaa-core/src/` must still be 0 (no mDNS CODE exists — that is what you add)
  grep -n "//! .*discovery\|filled exclusively" crates/uaa-core/src/discovery.rs   # expect: 1+ hits (the CP-01 stub you fill)
  grep -n "NetworkError\|ConfigError" src/error.rs   # expect: hits — the two variants you use (mapped: crates/uaa-core/src/error.rs)
  ```

## Step-by-step

1. Run the ⛔ START HERE block, then the anchor greps (mapped paths per the path-map note).
2. Fill `crates/uaa-core/src/discovery.rs` (keep the CP-01 header, bump to 1.1.0) with the types from Background: `ServiceKind` (+ `as_txt()` → `"control"|"web"|"pxe"` and `from_txt()`), `Source`, `ServiceInfo`, `EndpointsFile` + `StaticEndpoint` (serde, `#[serde(deny_unknown_fields)]`, absent-file-→-default loader `EndpointsFile::load_from(path)`).
3. Implement the pure core first (unit-testable, no network):
   - `pub fn union_candidates(mdns: Vec<ServiceInfo>, statics: Vec<ServiceInfo>) -> Vec<ServiceInfo>` — mDNS first in browse order, then statics whose `(host, port)` is not already present; duplicates keep the mDNS entry. NO other filtering — a static candidate is NEVER dropped because browse returned something (per-endpoint-failure fallback is the caller's loop; your job is to hand over the full union).
   - `pub fn static_candidates(kind: ServiceKind, file: &EndpointsFile) -> Vec<ServiceInfo>` — filter by kind, `Source::Static`, default version `0.0.0`, unparseable IP → skip with `tracing::warn!`.
4. Implement the mDNS layer over `mdns-sd`: `advertise(info)` registers `_uaa._tcp.local.` instance `uaa-<service>-<host>` with TXT `service`/`version`/`port`, returns `DiscoveryHandle` (doc comment: "daemons only — the `uaa` CLI is browse-only, spec Decision 11"); `async fn browse_mdns(kind, timeout) -> Vec<ServiceInfo>` collects `ServiceResolved` events until timeout, filters TXT `service == kind.as_txt()`, skips bad-version candidates with a warn. Browse errors (daemon spawn failure etc.) degrade to an EMPTY mdns vec with a warn — the static path must still work on hosts without multicast (that is the fallback's whole point); they do NOT abort resolve().
5. `pub async fn resolve(kind, static_fallback, timeout)` = `union_candidates(browse_mdns(kind, timeout).await, static_candidates(kind, static_fallback))`; if the union is EMPTY return `Err(AutoInstallError::NetworkError(...))` naming kind + timeout + "no mDNS answers and no static endpoints" — never `Ok(vec![])`, never a guessed default. (Fail-closed, spec C1: "resolve() with an empty candidate union is an error, never a guess.")
6. `#[cfg(test)]` tests (pure functions + tempdir yaml; NO live multicast):
   | Test | Asserts |
   |---|---|
   | `test_union_mdns_first_then_static` | 1 mdns + 1 distinct static → vec of 2, mdns at index 0, static labeled `Source::Static` |
   | `test_union_dedupes_same_host_port` | same `(host, port)` in both → 1 entry, `Source::Mdns` |
   | `test_union_static_survives_nonempty_mdns` | **anti-over-suppression / happy path:** browse returned a candidate AND a DIFFERENT static entry exists → the static entry is STILL in the union (never only-on-empty-browse) |
   | `test_static_candidates_filters_kind` | endpoints file with control+web rows, kind=Web → only the web row |
   | `test_endpoints_file_absent_is_empty` | `load_from` on a nonexistent path → `Ok(default)`, zero endpoints |
   | `test_endpoints_file_invalid_fails_closed` | garbage yaml / unknown field → `Err(ConfigError)` |
   | `test_resolve_empty_union_is_error` | empty mdns + empty static → assert the composed empty-union branch errors, message names the service kind (test the sync post-condition via a small `ensure_nonempty()` helper `resolve` uses) |
7. Bump the file header (version + last-edited) on every file you touch; new files get a fresh 4-line header with a new uuid4 (`uuidgen | tr 'A-F' 'a-f'`).

## How to test

```bash
cargo test --lib --offline && cargo build --offline
# Expected: all tests pass (baseline 311 + prior-wave additions + your 7 discovery tests), build clean
cargo clippy --offline -- -D warnings
# Expected: no warnings
cargo test --lib --offline -p uaa-core discovery
# Expected: 7 passed; 0 failed (all discovery tests, no network required)
grep -rn "mdns_sd" crates/uaa-core/src/ --include="*.rs" -l
# Expected: exactly one file — crates/uaa-core/src/discovery.rs
```

## Acceptance criteria

- [ ] API surface present: `grep -n "pub async fn advertise\|pub async fn resolve\|pub fn union_candidates\|pub struct EndpointsFile" crates/uaa-core/src/discovery.rs` → 4 hits.
- [ ] Union semantics proven: `test_union_mdns_first_then_static`, `test_union_dedupes_same_host_port` green; fail-closed empty union proven by `test_resolve_empty_union_is_error` (`grep -n "never a guess\|no mDNS answers" crates/uaa-core/src/discovery.rs` → ≥1 hit in the error text).
- [ ] **Anti-over-suppression:** `test_union_static_survives_nonempty_mdns` passes — the dedupe/filter path never drops a distinct static candidate when browse is non-empty (per-endpoint-failure fallback preserved).
- [ ] Browse-only CLI rule documented: `grep -n "daemons only" crates/uaa-core/src/discovery.rs` → ≥1 hit on `advertise`.
- [ ] Endpoints file semantics: absent → empty (`test_endpoints_file_absent_is_empty`), invalid → `ConfigError` (`test_endpoints_file_invalid_fails_closed`), both green.
- [ ] Only `mdns-sd` used: `grep -rn "zeroconf\|libmdns\|astro-dnssd" crates/ Cargo.toml` → 0 hits.
- [ ] `cargo test --lib --offline && cargo build --offline` exits 0; clippy clean.
- [ ] File headers bumped on every changed file (`git diff origin/main --stat` files each show a version bump; guids unchanged).

## Commit message

```
feat(discovery): mDNS advertise + union-resolve with static fallback (ws1-core)

Fills the CP-01 discovery.rs stub per spec Decision 11: _uaa._tcp.local
advertise (daemons only; CLI browse-only) and resolve() returning the UNION of
mDNS + static /etc/uaa/endpoints.yaml candidates — mDNS first, dedupe by
(host,port), static entries always included (per-endpoint-failure fallback,
never only-on-empty-browse), empty union = hard NetworkError (fail-closed,
never a guess). Browse errors degrade to static-only with a warning. Pure
union/parse core unit-tested without multicast; 7 tests incl. the
static-survives-nonempty-mdns anti-over-suppression case.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

Additive polarity: if `grep -n "pub async fn resolve" crates/uaa-core/src/discovery.rs` hits, the task is already applied — run the Acceptance criteria checks instead of re-applying. Rollback = revert the single commit; discovery.rs returns to the CP-01 stub — nothing else references it yet (its first consumers arrive with the daemon crates in waves 3–7), and no network or file state exists to unwind.
