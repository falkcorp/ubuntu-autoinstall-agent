<!-- file: docs/agent-tasks/pki/TASK-02-agent-enroll-client.md -->
<!-- version: 1.0.0 -->
<!-- guid: 34c1ae42-f4a8-4bbc-a94d-81b71f23b99d -->
<!-- last-edited: 2026-07-10 -->

# TASK-02 — Agent-side pki.rs: keypair+CSR gen, pinned-CA poll loop with persistence/resume, `uaa enroll` command fill (ws4-pki)

**Priority:** P1 · **Effort:** M · **Recommended subagent:** Sonnet-class · rust-client subagent · **Why:** client state machine with restart resume; fills the CP-01 stub. · **Depends on:** none within this workstream (wave-3 gated: `core-proto/TASK-02` (CP-02) must be MERGED first — workspace deps + `uaa-proto` types; CP-01 already created the stubs this task fills)

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/pki-agent-enroll-client" -b agent/pki-agent-enroll-client origin/main
cd "$REPO/.worktrees/pki-agent-enroll-client"
git rebase origin/main
```

(Protocol also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

## Goal

Fill two CP-01-created stubs, exactly and only these files:

1. `crates/uaa-core/src/pki.rs` — the client half of spec C6 / Decision 7: `pub fn generate_keypair_and_csr(identity: &AgentIdentity) -> Result<(KeyPem, CsrPem)>` (P-256 key via rcgen; CSR SAN = hostname + `uaa-mac:<mac>` URI, mirroring what PK-01's server signs) and `pub async fn enroll_poll(endpoint, pinned_ca, state_dir) -> Result<Credential>` — the persistent, restart-resumable poll loop (signatures per spec C1's `pki.rs` sketch; adapt names to the CP-01 stub if it already declares them — the stub is authoritative for signatures, this brief for semantics).
2. `crates/uaa/src/cli/enroll.rs` — the `uaa enroll` handler filling the CLI variant CP-01 pre-wired (variant + dispatch arm already exist; verify with the greps below — you add ONLY the handler body).

REUSE — do not invent parallels:
- **rcgen** (workspace dep after CP-02 — verify: `grep -rn "rcgen" Cargo.toml crates/*/Cargo.toml`) for keypair + CSR. Do NOT add `openssl` or shell out to an `openssl` binary.
- **reqwest with rustls** (root dep today at `Cargo.toml:31`, workspace dep after CP-02) for the `:7444` JSON plane: `ClientBuilder::tls_built_in_root_certs(false)` + `add_root_certificate(pinned_ca)` — the install CA is the ONLY trust root (Decision 7: agent pins it). **NEVER use the CockroachDB CA** (Decision 6).
- Put the HTTP transport behind a small `trait EnrollTransport` (`async fn submit_csr(...)`, `async fn get_credential(fp)`) implemented once with reqwest and once as a `#[cfg(test)]` scripted mock — **tests never open sockets**.

Purely additive: fill the two stubs; touch no other module, variant, or handler beyond header bumps.

## Background (verify before editing)

- Design spec: `docs/specs/constellation-design.md` — Decision 7 (flow + repairs a/b), C6 (state machine), C1 (`pki.rs` sketch). Skeleton `shared_state`: backoff 30s→5m cap; idempotent re-claim by SPKI fingerprint.
- **Client state machine (C6, agent side — NORMATIVE):** on start, load `agent.key`, `agent.csr`, `claim.json` from `state_dir` — create all three if absent (generate keypair+CSR once; `claim.json` records the SPKI fp + submitted-at). Pin `install-ca.crt`: **missing/unreadable CA file = fail-closed** — return a typed error the caller retries on (abort + retry loop); NEVER fall back to system roots or plain HTTP. Then `SubmitCsr` (idempotent — safe on every boot), then poll `GetCredential` keyed by the SPKI fp with exponential backoff 30s→5m cap. `pending` → keep polling. `issued` → persist `agent.crt` (0600, tmp+rename) and return. `rejected`/`revoked` → log loudly and hold at a fixed 1h poll interval (operator can re-approve server-side; the loop must survive that transition back to `issued`).
- Edge semantics (spell in code AND tests): restart resume — if `agent.key`+`agent.csr` exist, re-derive the SAME SPKI fp and re-claim it (never mint a second keypair over a live claim); if `agent.crt` already exists and is unexpired, `enroll_poll` returns it immediately without network. Renewal is server-side auto-issue (PK-01) — the client just re-submits the same-key CSR when the persisted cert is past 2/3 lifetime. Missing `state_dir` → create it `0700`. Unknown-fp 404 from the server while we hold a local claim = server lost state → re-submit CSR, do not error out.
- All key/cert writes: `0600`, atomic tmp+rename, under `state_dir` (production `/var/lib/uaa/`, tests `tempfile::tempdir()`).
- `uaa enroll` handler: flags per the CP-01 pre-wired variant (endpoint URL, `--ca <path>` default `/etc/uaa/install-ca.crt` — the file PK-04 bakes into seeds, `--state-dir` default `/var/lib/uaa`); prints state transitions; exit non-zero on fail-closed CA-missing error.

**Path map:** after CP-01 (wave 1) merges, `src/**` lives at `crates/uaa-core/src/**` and the CLI at `crates/uaa/src/**`. The greps below cite pre-move paths (verifiable on today's main) where they reference existing code; at execution time run them at the old path, then the mapped path. Zero hits at BOTH = STOP and report.

**HARD RULES (non-negotiable):**
- NO hardware actions. Validate ONLY in-repo (`cargo`) and, where a brief says so, the QEMU+swtpm harness (`scripts/vm-validate.sh`). Code that COULD touch hardware is written and unit-tested against mock executors only.
- NEVER wipe, write to, or deploy on 172.16.2.30 ("the server") or len-serv-003.
- `disk_device` is read from the live target at runtime, never guessed or hardcoded.
- ipmitool runs via `ssh 172.16.2.30`, never on macOS.
- NEVER power on unimatrixone (U1).
- No real secret in any file: `REPLACE_AT_PLACE_TIME` placeholders stay placeholders.
- Stay inside your worktree; never `git push`, `gh pr`, or merge — report done and stop.

Plus, PKI-specific: **NEVER use the CockroachDB CA**; no real private key or certificate in any fixture — tests generate ephemeral rcgen material.

- **Re-verify these anchors before editing** — line numbers drift; zero hits at both old and mapped path = STOP and report:
  ```bash
  grep -n "rustls-tls" Cargo.toml                                   # expect: 2 hits (comment line 30 + reqwest feature line 31; post-CP-02 also check crates/uaa-core/Cargo.toml)
  test -f crates/uaa-core/src/pki.rs && test -f crates/uaa/src/cli/enroll.rs && echo STUBS-PRESENT
  # expect: STUBS-PRESENT — missing = CP-01/CP-02 not merged; STOP and report (wave gate)
  grep -rn "Enroll" crates/uaa/src/cli/args.rs crates/uaa/src/main.rs
  # expect: 1+ hit per file (CP-01 pre-wired variant + dispatch arm — you fill the handler only)
  grep -n "GetCredential\|SubmitCsr" docs/specs/constellation-design.md   # expect: 2+ hits (enroll.proto surface)
  ```

## Step-by-step

1. Run the ⛔ START HERE block, then the anchor greps above. Any zero-hit/missing-file result → STOP and report.
2. **`pki.rs`: types + keygen.** `AgentIdentity { hostname, mac }`; `generate_keypair_and_csr` via rcgen P-256, SANs as in Goal; `pub fn spki_fingerprint(csr_pem: &str) -> Result<String>` (sha256 hex of the CSR's SubjectPublicKeyInfo — MUST produce the same value PK-01's server computes; document the byte range hashed).
3. **`pki.rs`: state persistence.** `EnrollState::load_or_init(state_dir)` — create-if-absent per Background; all writes 0600 tmp+rename; re-loading an existing dir yields the identical fp.
4. **`pki.rs`: `EnrollTransport` trait** + reqwest impl (pinned CA only, `tls_built_in_root_certs(false)`) + `#[cfg(test)]` scripted mock returning a programmable sequence of responses.
5. **`pki.rs`: `enroll_poll`.** Implements the loop from Background: short-circuit on valid persisted cert; fail-closed on missing CA; submit → poll with 30s→5m exponential backoff (factor 2, cap 5m — make the schedule a pure function `pub fn backoff_delay(attempt: u32) -> Duration` so tests assert it without sleeping); `rejected`/`revoked` → 1h fixed interval + loud `tracing::error!`; 404-with-local-claim → re-submit; `issued` → persist + return. Inject a clock/sleep seam (`tokio::time` + `#[cfg(test)]` pause or a `Sleeper` trait) — tests must not real-sleep.
6. **`crates/uaa/src/cli/enroll.rs`:** handler reading the pre-wired flags, loading the pinned CA from `--ca`, calling `enroll_poll`, printing each transition; CA-missing error exits non-zero with a message naming `/etc/uaa/install-ca.crt`.
7. **Tests** (mock transport + tempdir): `test_keypair_csr_sans`, `test_spki_fp_stable_across_reload` (init, drop, re-load → same fp, same key bytes), `test_missing_ca_fail_closed` (no CA file → typed Err, transport records ZERO calls), `test_backoff_schedule_30s_to_5m_cap` (attempts 0..10 → 30s,60s,...,300s,300s), `test_pending_then_issued_persists_cert` (scripted pending,pending,issued → cert file 0600 exists, content matches), `test_rejected_holds_1h`, `test_404_with_claim_resubmits`, `test_valid_cert_short_circuits_no_network` (transport records zero calls).
8. Bump the file header (version + last-edited) on every file you touch; new files get a fresh 4-line header with a new uuid4 (`uuidgen | tr 'A-F' 'a-f'`).

## How to test

```bash
cargo test --lib --offline && cargo build --offline
# Expected: all tests pass (baseline 311 + your new tests; earlier waves added more — never fewer), build clean
cargo clippy --offline -- -D warnings
# Expected: no warnings
cargo test --offline -p uaa-core pki::
# Expected: the 8 tests from step 7 all pass, no network, no real sleeps (suite finishes in seconds)
grep -rn "tls_built_in_root_certs(false)" crates/uaa-core/src/pki.rs
# Expected: 1 hit (system roots disabled — pinned CA is the only trust root)
```

## Acceptance criteria

- [ ] Pinning proven: `grep -n "tls_built_in_root_certs(false)" crates/uaa-core/src/pki.rs` → 1 hit; `test_missing_ca_fail_closed` passes with zero transport calls.
- [ ] Resume proven: `test_spki_fp_stable_across_reload` and `test_valid_cert_short_circuits_no_network` pass (idempotent re-claim by SPKI fp; no second keypair over a live claim).
- [ ] Backoff contract: `grep -n "pub fn backoff_delay" crates/uaa-core/src/pki.rs` → 1 hit; `test_backoff_schedule_30s_to_5m_cap` passes.
- [ ] CLI wired: `cargo run --offline -p uaa -- enroll --help` exits 0 and shows `--ca` with default `/etc/uaa/install-ca.crt`.
- [ ] **Anti-over-suppression:** `test_pending_then_issued_persists_cert` passes — the fail-closed CA guard and terminal-state hold do not block the legitimate pending→issued happy path.
- [ ] `cargo test --lib --offline && cargo build --offline` exits 0; clippy clean.
- [ ] File headers bumped on every changed file (`git diff origin/main --stat` files each show a version bump; guids unchanged).

## Commit message

```
feat(pki): agent enroll client — pinned-CA poll loop with persistence/resume (ws4-pki)

Fills CP-01 stubs crates/uaa-core/src/pki.rs + crates/uaa/src/cli/enroll.rs:
P-256 keypair/CSR (rcgen), SPKI-fp re-claim, fail-closed CA pinning
(system roots disabled), 30s->5m backoff, rejected/revoked 1h hold,
restart resume with 0600 tmp+rename persistence. Transport behind a
trait; 8 tests on a scripted mock — no sockets, no sleeps.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

Additive (stub-fill): if `grep -n "pub async fn enroll_poll" crates/uaa-core/src/pki.rs` and `grep -n "fn " crates/uaa/src/cli/enroll.rs` both hit with non-stub bodies, already applied — run the Acceptance criteria checks instead of re-applying. Rollback = revert the single commit; the CP-01 stub shells, the pre-wired CLI variant, and every other module stay untouched.
