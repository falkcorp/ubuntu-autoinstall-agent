<!-- file: docs/agent-tasks/pki/README.md -->
<!-- version: 1.0.0 -->
<!-- guid: 9af8f19f-9177-40cb-8c41-4e1247553a6f -->
<!-- last-edited: 2026-07-10 -->

# Workstream — enrollment PKI (install CA, agent enroll, mTLS/CRL, seed embedding)

Build the constellation's trust plane per spec C6 and Decisions 6/7/23/25 (`docs/specs/constellation-design.md`): a dedicated install CA + EnrollService in uaa-control (PK-01), the agent-side pinned-CA enroll client (PK-02), mTLS helpers + `ca issue-service` bootstrap + enforced CRL (PK-03), and install-ca.crt placement baked into the golden-tested user-data template and USB seed (PK-04). **NEVER the CockroachDB CA** — the install CA is a separate keypair under `/var/lib/uaa/ca/`. From ws4-pki.

| Task | Src id | Title | Priority | Effort | Tier | Wave |
|------|--------|-------|----------|--------|------|------|
| TASK-01 | ws4-pki | Install CA (rcgen) + EnrollService: CSR upsert by SPKI fp, approve/sign, supersede-on-reinstall, same-key renewal | P1 | L | Sonnet-class | 4 |
| TASK-02 | ws4-pki | Agent-side pki.rs: keypair+CSR gen, pinned-CA poll loop with persistence/resume, `uaa enroll` fill | P1 | M | Sonnet-class | 3 |
| TASK-03 | ws4-pki | tls.rs mTLS helpers + `ca issue-service` bootstrap + CRL publish/enforce (15m fetch, fail-open stale, fail-closed listed) | P1 | M | Sonnet-class | 5 |
| TASK-04 | ws4-pki | Bake install-ca.crt placement into user-data template + USB seed (golden regen) | P2 | S | Sonnet-class | 5 |

Waves are GLOBAL across the constellation plan (see `docs/specs/constellation-plan.md`): this workstream runs TASK-02 in wave 3, TASK-01 in wave 4, TASK-03 + TASK-04 in wave 5.

## Ground rules

- Rust + one cloud-config seed file, in exactly the files each brief names: PK-01 `crates/uaa-control/src/{ca,enroll}.rs` (CT-01 stubs), PK-02 `crates/uaa-core/src/pki.rs` + `crates/uaa/src/cli/enroll.rs` (CP-01 stubs), PK-03 `crates/uaa-core/src/tls.rs` (CP-01 stub) + `crates/uaa-control/src/ca.rs`, PK-04 template + goldens + `installer-image/nocloud/user-data`. Purely additive stub-fills throughout.
- Build + test gate for every task in this workstream:
  ```bash
  cargo test --lib --offline && cargo build --offline
  # Expected: all tests pass (baseline 311 + prior waves + the task's new tests), build clean
  cargo clippy --offline -- -D warnings
  # Expected: no warnings
  ```
- **Verify every file:line anchor with `grep` before editing** — these tasks run in waves 3–5, AFTER the CP-01 workspace transform and CT-01 crate creation, so every pre-move `src/**` path has relocated (path map: `src/**` → `crates/uaa-core/src/**`, CLI → `crates/uaa/src/**`) and line numbers WILL have drifted; the grep hits are authoritative. Zero hits at both old and mapped path = STOP and report.
- File headers MANDATORY: new files get a fresh 4-line header (`file:`/`version:`/`guid:`/`last-edited:`); edited files get version bumped + `last-edited` updated, guid preserved.
- HARD RULES (operation contract, restated in every brief):
  - **NEVER use the CockroachDB CA** — dedicated install CA only (Decision 6); service daemons never traverse the enrollment flow (Decision 23).
  - No real secret or certificate PEM in source, tests, fixtures, or seeds — `REPLACE_AT_PLACE_TIME`-style placeholders stay placeholders; tests generate ephemeral rcgen material.
  - NO hardware actions; validation is cargo-only (mock store/transport — no live CockroachDB, no sockets). NEVER wipe/write 172.16.2.30 or len-serv-003; NEVER power on unimatrixone.
  - CA/cert/key writes are 0600 (keys) with tmp+rename atomicity; production paths (`/var/lib/uaa/ca`, `/var/lib/uaa/certs`, `/etc/uaa`) are parameter DEFAULTS — tests use tempdirs.
  - Workers stay in their worktree and NEVER push/PR/merge — the coordinator owns all git.

## Collision / wave note

From the operation collision matrix (skeleton `.collision_rows`):

| Shared file | Colliding tasks | Resolution |
|---|---|---|
| `crates/uaa-control/src/ca.rs` | CT-01 (stub), PK-01, PK-03 | serialize: wave3=CT-01 (stub), wave4=PK-01, wave5=PK-03 |
| stub-pattern (uaa-core stubs by CP-01; uaa-control stubs by CT-01) | creator + exactly one filler each | stub wave strictly precedes fill wave; PK-02 fills `pki.rs`+`enroll.rs`(CLI), PK-03 fills `tls.rs` — each has exactly one filler |

Execution mode: **SERIAL WAVES — PK-01→PK-03 share ca.rs (collision row); PK-02 parallel-safe with PK-01** — trigger: 2 tasks on one shared file (`ca.rs`) forces serialization across waves 4→5; the wave-5 pair PK-03 + PK-04 is parallel-safe (disjoint files: tls.rs+ca.rs vs template+goldens+seed). Cross-workstream gates: PK-02 needs CP-02 merged (wave 2); PK-01 needs CT-01 merged (wave 3); PK-03/PK-04 need PK-01 merged (wave 4).

Link: See [ORCHESTRATION.md](../ORCHESTRATION.md) for the coordinator + worker protocol.
