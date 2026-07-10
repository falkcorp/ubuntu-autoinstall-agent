<!-- file: docs/agent-tasks/pki/TASK-01-install-ca-enroll-service.md -->
<!-- version: 1.0.0 -->
<!-- guid: 9cc27de9-a09a-40f1-8e29-744821736ba7 -->
<!-- last-edited: 2026-07-10 -->

# TASK-01 — Install CA (rcgen) + EnrollService: CSR upsert by SPKI fp, approve/sign, supersede-on-reinstall, same-key renewal (ws4-pki)

**Priority:** P1 · **Effort:** L · **Recommended subagent:** Sonnet-class · rust-services subagent · **Why:** the enrollment state machine is the security core; spec C6 is the contract. · **Depends on:** none within this workstream (wave-4 gated: `control/TASK-01` (CT-01) must be MERGED first — it creates the `crates/uaa-control` crate and the `ca.rs`/`enroll.rs` stubs this task fills)

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/pki-install-ca-enroll-service" -b agent/pki-install-ca-enroll-service origin/main
cd "$REPO/.worktrees/pki-install-ca-enroll-service"
git rebase origin/main
```

(Protocol also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

## Goal

Fill the two CT-01-created stub files `crates/uaa-control/src/ca.rs` and `crates/uaa-control/src/enroll.rs`:

1. **Install CA** (`ca.rs`): a dedicated CA per spec Decision 6 — keypair generated ONCE with `rcgen` (pure Rust, already a workspace dependency after CP-02/CT-01; verify: `grep -n "rcgen" Cargo.toml crates/uaa-control/Cargo.toml`), persisted under a configurable CA dir (production default `/var/lib/uaa/ca/`, mode `0600` files, `0700` dir), loaded only by uaa-control. **NEVER use the CockroachDB CA** — this is a locked decision (Decision 6); no code path may read `certs/ca.crt` from any cockroach directory. Signs agent certs: 90-day lifetime, SAN = hostname + `uaa-mac:` URI (spec C6).
2. **EnrollService** (`enroll.rs`): the `uaa.enroll.v1` gRPC service (`SubmitCsr`, `GetCredential` — tonic server types come from `crates/uaa-proto`, CP-02) PLUS the `:7444` HTTPS JSON mirror routes (axum handlers registered into the router scaffold CT-01 created; server-auth-only TLS, enrollment clients pin the install CA). Implements the C6 state machine verbatim (below).

REUSE — do not invent parallels:
- **Registry store trait** from CT-01 (`crates/uaa-control/src/store.rs` or as named by CT-01 — verify: `grep -rn "pub trait.*Store" crates/uaa-control/src/`) for all `enrollments`-table access. Tests use the CT-01 mock store; **cargo tests must NOT require a live CockroachDB**.
- **Audit hook** from CT-01 (`grep -rn "pub fn record\|pub async fn record" crates/uaa-control/src/audit.rs`) — every approve/reject/revoke/supersede/issue mutation records an audit event through it.
- `AutoInstallError` variants from `uaa-core` for error paths — no new error enum.

Purely additive: fill the two stubs, register routes/service in the wiring points CT-01 left (marked `// FILLED BY PK-01` or equivalent — verify with the greps below); modify nothing else.

## Background (verify before editing)

- Design spec: `docs/specs/constellation-design.md` — Decisions 6 (dedicated CA, rcgen, 0600, backup runbook is the M3 ship-gate), 7 (enrollment flow + repairs), 23 (service daemons NEVER use this flow), 25 (CRL — implemented by PK-03, NOT here; leave revocation as a state transition + audit event only).
- The `enrollments` CRDB table (spec "Data model"): PRIMARY KEY `spki_fingerprint` (sha256 of the CSR public key), columns `mac`, `csr_pem`, `state` (`pending|approved|issued|rejected|revoked|superseded`), `cert_pem`, `requested_at`, `decided_by`. CT-01 shipped the migration; this task only reads/writes through the store trait.
- **Enrollment state machine (spec C6, NORMATIVE — implement exactly):**

  ```text
  agent boot ──▶ load /var/lib/uaa/{agent.key,agent.csr,claim.json}   (create if absent)
     │  pin install-ca.crt (from seed/ISO)             (no CA file → abort + retry loop, fail-closed)
     ├─ SubmitCsr (idempotent upsert by SPKI fp)
     ├─ GetCredential poll (backoff 30s→5m cap) ──▶ pending: keep polling (survives reboot)
     │                                          └▶ issued: persist agent.crt → mTLS gRPC :7443
     └─ rejected/revoked: log loudly, hold at 1h poll (operator can re-approve)
  Approve (SPA): pending CSR list (SPKI fp + claimed MAC/hostname + discovery-inbox
  correlation) → approve/reject → control signs (rcgen, 90d, SAN = hostname + mac URI);
  approving a fp for a MAC with an existing issued row marks that row `superseded`.
  Renewal: same-key CSR at 2/3 lifetime; auto-issue iff unexpired+unrevoked cert exists for
  the SPKI; expired-through-outage agents fall back to pending (re-approve) — legacy :25000
  keeps working meanwhile. Revocation: CRL per Decision 25.
  Service daemons NEVER use this flow — Decision 23.
  ```

- Edge semantics (spell these in code AND tests): `SubmitCsr` for an ALREADY-KNOWN SPKI fp is an idempotent upsert — it returns the current state, never resets a decided row to `pending`, EXCEPT the renewal rule: a same-key CSR while an unexpired+unrevoked `issued` cert exists for that SPKI auto-issues a fresh 90-day cert (no operator round-trip). `GetCredential` for an UNKNOWN SPKI fp → 404 / NOT_FOUND, **never auto-issue, never auto-create** (fail-closed, spec C3 enrollment plane). Approving a fp whose claimed MAC already has a different `issued` row marks that old row `superseded` (reinstalls wipe the agent state dir and mint a new key — rows must not accrete). A `rejected` or `revoked` row stays terminal until an operator explicitly re-approves it.
- CA key material: written `0600`, directory `0700`; CA dir path is a constructor parameter (tests use `tempfile::tempdir()`); generation is create-if-absent — an existing CA is loaded, never regenerated (regenerating would orphan every issued cert).
- `shared_state` (skeleton): CA key `/var/lib/uaa/ca` 0600; **the offline encrypted CA-key backup runbook is the M3 ship-gate — a documentation deliverable owned by the coordinator, NOT code in this task.** Do not write the runbook here.
- `crates/uaa-control/src/ca.rs` is ALSO edited later by PK-03 (collision row: CT-01 stub → PK-01 → PK-03). You own it exclusively in wave 4; leave signing/loading helpers `pub` so PK-03 can reuse them.

**HARD RULES (non-negotiable):**
- NO hardware actions. Validate ONLY in-repo (`cargo`) and, where a brief says so, the QEMU+swtpm harness (`scripts/vm-validate.sh`). Code that COULD touch hardware is written and unit-tested against mock executors only.
- NEVER wipe, write to, or deploy on 172.16.2.30 ("the server") or len-serv-003.
- `disk_device` is read from the live target at runtime, never guessed or hardcoded.
- ipmitool runs via `ssh 172.16.2.30`, never on macOS.
- NEVER power on unimatrixone (U1).
- No real secret in any file: `REPLACE_AT_PLACE_TIME` placeholders stay placeholders.
- Stay inside your worktree; never `git push`, `gh pr`, or merge — report done and stop.

Plus, PKI-specific: **NEVER use the CockroachDB CA for anything in this workstream** — the install CA is a separate keypair under `/var/lib/uaa/ca/`. No test fixture may contain a real private key or certificate — generate ephemeral keys inside tests with rcgen.

- **Re-verify these anchors before editing** — line numbers drift; zero hits at both old and mapped path = STOP and report:
  ```bash
  grep -n "superseded" docs/specs/constellation-design.md            # expect: 3 hits (C6 rule, schema comment, approve rule)
  grep -n "issue-service" docs/specs/constellation-design.md          # expect: 2+ hits (Decision 23 — PK-03 scope, cited for boundary)
  test -f crates/uaa-control/src/ca.rs && test -f crates/uaa-control/src/enroll.rs && echo STUBS-PRESENT
  # expect: STUBS-PRESENT — zero/missing = CT-01 not merged yet; STOP and report (wave gate)
  grep -rn "mod ca" crates/uaa-control/src/                           # expect: 1+ hits (CT-01 registered the stub module)
  grep -rn "pub trait" crates/uaa-control/src/ | head                 # expect: hits — the store trait you must reuse
  ```

## Step-by-step

1. Run the ⛔ START HERE block, then the anchor greps above. Any zero-hit/missing-file result → STOP and report.
2. **`ca.rs` — CA lifecycle.** Implement `pub struct InstallCa` with `pub fn load_or_create(ca_dir: &Path) -> Result<InstallCa>` (create-if-absent; on create, write `ca.key` `0600` + `ca.crt` `0644` inside a `0700` dir via `std::os::unix::fs::PermissionsExt`; on load, never regenerate), `pub fn sign_agent_csr(&self, csr_pem: &str, hostname: &str, mac: &str) -> Result<String>` (rcgen, 90-day `not_after`, SAN = DNS hostname + URI `uaa-mac:<mac>`), and `pub fn ca_cert_pem(&self) -> &str`. Keep these `pub` — PK-03 reuses them.
3. **`enroll.rs` — state machine core**, storage-agnostic against the CT-01 store trait: `pub async fn submit_csr(store, csr_pem, claimed_mac, claimed_hostname) -> Result<EnrollState>` — compute the SPKI sha256 fingerprint from the CSR public key; upsert-if-absent as `pending`; known fp returns current state unchanged; known fp in `issued` with an unexpired+unrevoked cert AND identical public key → auto-issue a fresh cert (renewal rule) and return `issued`.
4. **`enroll.rs` — decisions**: `pub async fn approve(store, ca, fp, decided_by) -> Result<()>` — sign via `ca.sign_agent_csr`, set `issued` + `cert_pem` + `decided_by`; FIRST mark any other `issued` row for the same MAC `superseded`. `pub async fn reject(...)` / `pub async fn revoke(...)` set terminal states (revoke also emits the audit event PK-03's CRL will consume). Every mutation calls the CT-01 audit hook.
5. **`enroll.rs` — service surfaces**: implement the tonic `EnrollService` (`SubmitCsr`, `GetCredential`) delegating to steps 3–4; `GetCredential(unknown fp)` → `Status::not_found`, `pending` → a pending response (no cert bytes), `issued` → cert + CA chain. Register the JSON mirror (`POST /enroll/csr`, `GET /enroll/credential/<fp>`; JSON 404 for unknown fp) into the CT-01 `:7444` router wiring point.
6. **Tests** (`#[cfg(test)]` in each file, mock store, ephemeral rcgen keys): `test_submit_csr_idempotent_reclaim` (second submit of same CSR returns same state, one row), `test_unknown_fp_get_credential_404`, `test_approve_signs_90d_san` (parse the issued cert, assert lifetime ≈90d and both SANs), `test_supersede_on_reinstall` (MAC with issued row + approve of a NEW fp for same MAC → old row `superseded`, new row `issued`), `test_renewal_same_key_auto_issue`, `test_renewal_refused_when_revoked` (same-key CSR after revoke stays `revoked` — no auto-issue), `test_rejected_holds_until_reapprove`, `test_ca_load_or_create_idempotent` (second load returns same cert, key file mode `0600`), and the anti-over-suppression test in Acceptance.
7. Bump the file header (version + last-edited) on every file you touch; new files get a fresh 4-line header with a new uuid4 (`uuidgen | tr 'A-F' 'a-f'`).

## How to test

```bash
cargo test --lib --offline && cargo build --offline
# Expected: all tests pass (baseline 311 + your new tests; earlier waves added more — never fewer), build clean
cargo clippy --offline -- -D warnings
# Expected: no warnings
cargo test --offline -p uaa-control
# Expected: all uaa-control tests pass, incl. the 9+ enrollment tests above, with NO CockroachDB running
grep -rn "cockroach" crates/uaa-control/src/ca.rs crates/uaa-control/src/enroll.rs
# Expected: 0 hits (install CA is fully independent of the cockroach CA)
```

## Acceptance criteria

- [ ] State machine states match schema exactly: `grep -n "pending\|approved\|issued\|rejected\|revoked\|superseded" crates/uaa-control/src/enroll.rs` → all six appear.
- [ ] Fail-closed poll: `grep -n "not_found" crates/uaa-control/src/enroll.rs` → ≥1 hit, and `test_unknown_fp_get_credential_404` passes (never auto-issues).
- [ ] Supersede + renewal proven: `cargo test --offline -p uaa-control test_supersede_on_reinstall test_renewal` → all pass.
- [ ] CA custody: `test_ca_load_or_create_idempotent` asserts key file mode `0600` and no regeneration; `grep -rn "cockroach" crates/uaa-control/src/{ca,enroll}.rs` → 0 hits.
- [ ] **Anti-over-suppression:** `test_approved_fp_still_issued_happy_path` — a legitimate pending→approve→GetCredential flow returns the signed cert (the unknown-fp 404 guard and terminal-state holds do not block the happy path).
- [ ] `cargo test --lib --offline && cargo build --offline` exits 0; clippy clean.
- [ ] File headers bumped on every changed file (`git diff origin/main --stat` files each show a version bump; guids unchanged).

## Commit message

```
feat(pki): install CA (rcgen) + EnrollService with supersede + same-key renewal (ws4-pki)

Fills the CT-01 stubs crates/uaa-control/src/{ca,enroll}.rs: dedicated
install CA (load-or-create, 0600, never the cockroach CA), C6 enrollment
state machine (pending/approved/issued/rejected/revoked/superseded),
idempotent SubmitCsr upsert by SPKI fingerprint, fail-closed 404 poll,
supersede-on-reinstall, same-key renewal auto-issue. gRPC uaa.enroll.v1 +
:7444 JSON mirror. Mock-store tests only; no live CRDB.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

Additive (stub-fill): if `grep -n "pub fn sign_agent_csr" crates/uaa-control/src/ca.rs` and `grep -n "submit_csr" crates/uaa-control/src/enroll.rs` both hit, the task is already applied — run the Acceptance criteria checks instead of re-applying. Rollback = revert the single commit; the CT-01 stubs, the rest of `crates/uaa-control`, and all other crates stay untouched (no filesystem, server, or CA state exists outside tests).
