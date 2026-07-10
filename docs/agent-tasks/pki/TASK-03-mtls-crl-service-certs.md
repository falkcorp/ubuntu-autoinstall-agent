<!-- file: docs/agent-tasks/pki/TASK-03-mtls-crl-service-certs.md -->
<!-- version: 1.0.0 -->
<!-- guid: 015a3c8e-8030-44dc-8849-d548768e024b -->
<!-- last-edited: 2026-07-10 -->

# TASK-03 — tls.rs mTLS helpers + `ca issue-service` bootstrap + CRL publish/enforce (ws4-pki)

**Priority:** P1 · **Effort:** M · **Recommended subagent:** Sonnet-class · rust-services subagent · **Why:** trust-plane wiring; revocation semantics per spec Decision 25. · **Depends on:** TASK-01 (PK-01) — **wave-5 gated: this task is serialized BEHIND the PK-01 merge because both edit `crates/uaa-control/src/ca.rs` (collision row: CT-01 stub → PK-01 → PK-03). Do NOT start until PK-01 is merged to `origin/main` and this worktree is rebased.**

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/pki-mtls-crl-service-certs" -b agent/pki-mtls-crl-service-certs origin/main
cd "$REPO/.worktrees/pki-mtls-crl-service-certs"
git rebase origin/main
```

(Protocol also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

## Goal

Two additive deliverables:

1. **`crates/uaa-core/src/tls.rs`** (CP-01 stub — you are its exclusive filler): mTLS config helpers used by every daemon and enrolled agent — `pub fn server_mtls_config(cert, key, ca, crl_cache) -> Result<rustls::ServerConfig>`, `pub fn client_mtls_config(cert, key, ca) -> Result<rustls::ClientConfig>`, plus the CRL cache type `CrlCache` implementing spec Decision 25 verifier semantics: **fail-closed per listed cert** (a presented cert whose serial is on the cached CRL is rejected), **fail-open + loud log on staleness** (a CRL not refreshed for >24h logs `tracing::error!` but does NOT kill the plane — availability over a 90-day-max exposure that expiry already bounds). Fetch cadence for consumers: every 15 minutes (expose `pub const CRL_REFRESH: Duration` and `pub const CRL_STALE_AFTER: Duration = 24h`; the fetch loop is a helper here, wired by WB-01/PX-01 later).
2. **`crates/uaa-control/src/ca.rs` additions** (REUSING PK-01's `InstallCa` — verify: `grep -n "pub fn sign_agent_csr\|pub struct InstallCa" crates/uaa-control/src/ca.rs`; do NOT write a second CA type): (a) `ca issue-service --for uaa-web|uaa-pxe|uaa-control` per spec Decision 23 — a server-local CLI path that mints a 1-year service cert + key and writes `/var/lib/uaa/certs/<svc>.{key,crt}` (key `0600`; dir parameterized for tests), **no network, no operator approval** (service daemons NEVER use the enrollment flow); (b) CRL publish per Decision 25 — `pub fn regenerate_crl(&self, revoked: &[RevokedEntry]) -> Result<String>` producing a signed CRL PEM, called on EVERY revocation (hook into PK-01's `revoke` path), served to the other daemons via the route CT-01/PK-01 wiring exposes.

**NEVER use the CockroachDB CA** (Decision 6) — the install CA from PK-01 signs everything here.

## Background (verify before editing)

- Design spec: `docs/specs/constellation-design.md` — Decision 23 (service identity minted server-locally at install time; `issue-service` runs as root on the server, writes per-service 0600 files; 1y lifetime, renewed by re-running the command), Decision 25 (revocation enforced, not cosmetic: signed CRL regenerated on every revocation, fetched by uaa-web/uaa-pxe every 15 minutes and cached; reject listed certs fail-closed; stale >24h = loud log, plane stays up), C3 "CA + CRL" bullet.
- Edge semantics (spell in code AND tests): a MISSING CRL cache at verifier startup counts as stale-from-birth — serve (fail-open) but log loudly on the same 24h rule; an UNPARSEABLE fetched CRL is discarded (keep the previous cache, log loudly) — never replace good data with garbage; a cert that is BOTH expired and listed is rejected on expiry first (normal rustls path). `issue-service` with an unknown `--for` value → typed ConfigError listing the three valid names, nothing written. Re-running `issue-service` for the same service OVERWRITES key+crt atomically (tmp+rename) — that is the documented renewal path, not an error.
- rcgen/rustls are workspace deps after CP-02/CT-01 — verify: `grep -rn "rcgen\|rustls" Cargo.toml crates/uaa-core/Cargo.toml crates/uaa-control/Cargo.toml | head`. If `rustls` CRL types are insufficient in the pinned version, keep `CrlCache` as an explicit serial-set check inside a custom `ClientCertVerifier` wrapper — semantics above are what is normative, not the mechanism.
- This task runs on cargo only: **no live server writes** — `/var/lib/uaa/certs` is the production default parameter; every test uses `tempfile::tempdir()`.
- `crates/uaa-core/src/tls.rs` has exactly one filler (you); `crates/uaa-control/src/ca.rs` was last touched by PK-01 — rebase picks up its final shape; conflicts here mean the wave gate was violated → STOP and report.

**Path map:** after CP-01 (wave 1) merges, `src/**` lives at `crates/uaa-core/src/**` and the CLI at `crates/uaa/src/**`. Any grep citing a pre-move path must be run at the old path, then the mapped path. Zero hits at BOTH = STOP and report.

**HARD RULES (non-negotiable):**
- NO hardware actions. Validate ONLY in-repo (`cargo`) and, where a brief says so, the QEMU+swtpm harness (`scripts/vm-validate.sh`). Code that COULD touch hardware is written and unit-tested against mock executors only.
- NEVER wipe, write to, or deploy on 172.16.2.30 ("the server") or len-serv-003.
- `disk_device` is read from the live target at runtime, never guessed or hardcoded.
- ipmitool runs via `ssh 172.16.2.30`, never on macOS.
- NEVER power on unimatrixone (U1).
- No real secret in any file: `REPLACE_AT_PLACE_TIME` placeholders stay placeholders.
- Stay inside your worktree; never `git push`, `gh pr`, or merge — report done and stop.

Plus, PKI-specific: **NEVER use the CockroachDB CA**; no real key/cert in fixtures — ephemeral rcgen material only.

- **Re-verify these anchors before editing** — line numbers drift; zero hits at both old and mapped path = STOP and report:
  ```bash
  grep -n "issue-service" docs/specs/constellation-design.md          # expect: 2+ hits (Decision 23 + C3)
  grep -n "fail-open on stale\|stale >24h\|fetched by uaa-web" docs/specs/constellation-design.md  # expect: 1+ hits (Decision 25)
  grep -n "pub struct InstallCa\|pub fn sign_agent_csr" crates/uaa-control/src/ca.rs
  # expect: 2 hits — PK-01's CA you reuse; zero = PK-01 not merged → STOP (wave gate)
  test -f crates/uaa-core/src/tls.rs && echo STUB-PRESENT              # expect: STUB-PRESENT (CP-01)
  grep -rn "rustls" Cargo.toml crates/uaa-core/Cargo.toml | head       # expect: 1+ hits (rustls available via reqwest/workspace deps)
  grep -rn "pub fn revoke\|pub async fn revoke" crates/uaa-control/src/enroll.rs
  # expect: 1 hit — PK-01's revoke path you hook regenerate_crl into
  ```

## Step-by-step

1. Run the ⛔ START HERE block, rebase onto the merged PK-01, then run the anchor greps above. Any zero-hit/missing-file result → STOP and report.
2. **`tls.rs`: `CrlCache`.** Fields: revoked serial set, `last_refreshed: Option<Instant>` (injectable clock seam for tests), signed-CRL verification against the install CA cert. Methods: `update_from_pem(&mut self, crl_pem, ca_cert)` (signature-check; unparseable/bad-sig → keep old data + `tracing::error!`), `is_revoked(&self, serial) -> bool`, `is_stale(&self, now) -> bool` (>24h or never refreshed).
3. **`tls.rs`: verifier wiring.** `server_mtls_config` requires-and-verifies client certs against the install CA and consults `CrlCache`: listed serial → handshake rejected (fail-closed); stale cache → allow + one loud log per staleness detection (rate-limit the log to once per refresh interval, not per handshake — say so in a comment). `client_mtls_config` presents cert/key, trusts ONLY the install CA root.
4. **`tls.rs`: fetch helper.** `pub async fn crl_refresh_loop(url, cache, transport)` on `CRL_REFRESH` (15m) — transport behind a trait so tests script it; consumers (WB-01/PX-01, later waves) spawn it.
5. **`ca.rs`: `issue_service`.** `pub fn issue_service(ca: &InstallCa, svc: ServiceName, out_dir: &Path) -> Result<()>` — `ServiceName` enum with exactly `UaaWeb|UaaPxe|UaaControl` (unknown unrepresentable at the type level; the CLI arg parse rejects others with the ConfigError from Background); 1y cert, SAN = service DNS name; write `<svc>.key` `0600` + `<svc>.crt` `0644`, tmp+rename. Wire the `ca issue-service --for <svc>` subcommand into the uaa-control CLI surface CT-01 established (verify: `grep -rn "issue-service\|IssueService" crates/uaa-control/src/`— fill the pre-wired point if present, else add the clap subcommand alongside CT-01's existing ones).
6. **`ca.rs`: CRL publish.** `regenerate_crl` signing the revoked-serial list with the CA key; call it from PK-01's `revoke` path so every revocation republishes; expose the current CRL PEM through the existing route wiring.
7. **Tests**: `test_crl_listed_cert_rejected` (handshake/verifier path with a revoked serial → Err), `test_crl_stale_serves_but_logs` (cache >24h old → `is_stale` true, verifier still accepts an unlisted cert), `test_crl_bad_update_keeps_previous`, `test_issue_service_writes_0600_1y` (tempdir; parse cert, assert ≈365d + SAN), `test_issue_service_rerun_overwrites`, `test_issue_service_unknown_name_rejected` (CLI-level parse), `test_regenerate_crl_signed_by_install_ca`, and the anti-over-suppression test in Acceptance.
8. Bump the file header (version + last-edited) on every file you touch; new files get a fresh 4-line header with a new uuid4 (`uuidgen | tr 'A-F' 'a-f'`).

## How to test

```bash
cargo test --lib --offline && cargo build --offline
# Expected: all tests pass (baseline 311 + prior waves + your new tests), build clean
cargo clippy --offline -- -D warnings
# Expected: no warnings
cargo test --offline -p uaa-core tls:: && cargo test --offline -p uaa-control ca
# Expected: the 7+ tests from step 7 all pass, no network, no live CRDB
grep -rn "cockroach" crates/uaa-core/src/tls.rs
# Expected: 0 hits (trust root is the install CA only)
```

## Acceptance criteria

- [ ] Revocation fail-closed: `test_crl_listed_cert_rejected` passes; `grep -n "is_revoked" crates/uaa-core/src/tls.rs` → ≥2 hits (definition + verifier call).
- [ ] Staleness fail-open + loud: `test_crl_stale_serves_but_logs` passes; `grep -n "CRL_STALE_AFTER" crates/uaa-core/src/tls.rs` → ≥1 hit with a 24h value.
- [ ] Service bootstrap: `test_issue_service_writes_0600_1y` and `test_issue_service_rerun_overwrites` pass; `grep -n "enum ServiceName" crates/uaa-control/src/ca.rs` → 1 hit with exactly 3 variants.
- [ ] Enrollment-flow boundary held: `grep -n "submit_csr\|GetCredential" crates/uaa-control/src/ca.rs` → issue-service path contains 0 hits (service certs never traverse enrollment — Decision 23).
- [ ] **Anti-over-suppression:** `test_unlisted_fresh_cert_accepted` — an unrevoked cert against a fresh CRL cache completes verification (the revocation guard does not block legitimate peers).
- [ ] `cargo test --lib --offline && cargo build --offline` exits 0; clippy clean.
- [ ] File headers bumped on every changed file (`git diff origin/main --stat` files each show a version bump; guids unchanged).

## Commit message

```
feat(pki): mTLS helpers + ca issue-service bootstrap + enforced CRL (ws4-pki)

Fills crates/uaa-core/src/tls.rs (server/client mTLS configs pinned to the
install CA; CrlCache: fail-closed on listed serials, fail-open+loud-log on
>24h staleness, 15m refresh helper) and extends crates/uaa-control/src/ca.rs
(issue-service --for uaa-web|uaa-pxe|uaa-control: server-local 1y certs,
0600, no enrollment flow per Decision 23; signed CRL regenerated on every
revocation). Reuses PK-01's InstallCa; never the cockroach CA. Ephemeral
rcgen test material only.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

Additive: if `grep -n "pub fn server_mtls_config" crates/uaa-core/src/tls.rs` and `grep -n "pub fn issue_service" crates/uaa-control/src/ca.rs` both hit, already applied — run the Acceptance criteria checks instead of re-applying. Rollback = revert the single commit; PK-01's `InstallCa`/enrollment code, the tls.rs stub shell, and all other crates stay untouched (no server-side cert state exists — production paths are defaults only, tests used tempdirs).
