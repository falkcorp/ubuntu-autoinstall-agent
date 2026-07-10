<!-- file: docs/agent-tasks/control/README.md -->
<!-- version: 1.0.0 -->
<!-- guid: c9fa2b1a-d6c3-46dd-abec-d181486b50f4 -->
<!-- last-edited: 2026-07-10 -->

# Workstream — control (uaa-control central daemon + operator SPA)

Build the constellation's central daemon `crates/uaa-control` — registry system-of-record (CockroachDB + snapshot/WAL degraded mode), the four listeners (:25000 socket-activated legacy plane, :7443/:7444/:8443), GitHub-OAuth RBAC, the hash-chained audit log, the ApproveMachine SAGA, one-click reinstall, the operator JSON+OpenAPI plane — plus the React+Vite SPA it embeds. From ws2-control. Spec: [docs/specs/constellation-design.md](../../specs/constellation-design.md) (Decisions 3, 4, 8, 13, 19, 21, 24 + component C3).

| Task | Src id | Title | Priority | Effort | Tier | Wave |
|------|--------|-------|----------|--------|------|------|
| TASK-01 | ws2-control | uaa-control crate: listeners + socket activation, embedded CRDB migrations (full spec schema), snapshot+WAL degraded mode, follower stubs | P1 | L | Opus-class | 3 |
| TASK-02 | ws2-control | Registry CRUD + `import --from` (insert-if-absent) + `export --to-json` + luks/tang store surface | P1 | M | Sonnet-class | 4 |
| TASK-03 | ws2-control | GitHub OAuth web flow + org/team RBAC middleware + signed session cookies | P1 | M | Sonnet-class | 4 |
| TASK-04 | ws2-control | Hash-chained audit log: FOR-UPDATE-serialized append, zero genesis, daily signed checkpoints, backfill cmd | P1 | M | Sonnet-class | 4 |
| TASK-05 | ws2-control | ApproveMachine SAGA: ordered placement→activation, compensation_pending retry, saga_log resume | P1 | L | Sonnet-class | 4 |
| TASK-06 | ws2-control | One-click ReinstallMachine: dual-layer boot-target reconciliation, power cycle, bounded watch + fail-safe flip-back + cooldown | P1 | M | Sonnet-class | 4 |
| TASK-07 | ws2-control | Operator JSON API (axum+utoipa, /api/openapi.json) + rust-embed SPA hosting | P2 | M | Sonnet-class | 5 |
| TASK-08 | ws2-control | React+Vite+TS SPA: machines, pending approvals (machines+CSRs), discovery inbox, audit view | P2 | L | Sonnet-class | 2 |

Waves are GLOBAL numbers from the constellation plan (local order: TASK-08 → TASK-01 → TASK-02..06 → TASK-07).

## Ground rules

- Rust confined to `crates/uaa-control/**` (TASK-08 is the exception: `web/**` + `.github/workflows/spa-build.yml`, NO Rust). Each wave-4 task fills EXACTLY the CT-01 stub file(s) its brief names — one filler per stub, no exceptions.
- Build + test gate for every task in this workstream:
  ```bash
  cargo test --lib --offline && cargo build --offline
  # Expected: all tests pass (baseline 311 + this workstream's accumulating tests), build clean
  cargo clippy --offline -- -D warnings
  # Expected: no warnings
  # TASK-08 additionally:
  cd web && npm ci && npm run build
  # Expected: clean install from the committed lockfile; tsc + vite build exit 0
  ```
- **Cargo tests must NEVER require a live CockroachDB, network, GitHub, or sibling daemon** — every external surface sits behind a trait with an in-crate mock (`MemRegistryStore`, `MockHealth`, mock `GithubApi`, mock `WebClient`/`PxeClient`, mock `PowerControl`).
- **Verify every file:line anchor with `grep` before editing** — briefs run waves after other tasks have reshaped the tree; the grep hits are authoritative, line numbers are not. Zero hits at both the cited and path-mapped location = STOP and report.
- File headers MANDATORY: new files get a fresh 4-line header (`file:/version:/guid:/last-edited:`) with a new uuid4; edited files get a version bump + `last-edited`, guid preserved.
- HARD RULES (operation contract, restated in the briefs): no hardware actions; never wipe/write 172.16.2.30 or len-serv-003; ipmitool via `ssh 172.16.2.30`, never macOS; NEVER power on unimatrixone; no real secret anywhere (`REPLACE_AT_PLACE_TIME` stays a placeholder; OAuth client secret is env-only); workers never push/PR/merge.

## Collision / wave note

Execution mode: "SERIAL WAVES (coordinator-driven) between waves; PARALLEL DISPATCH within — CT-02..06 fill disjoint CT-01 stubs" — trigger: 5 parallel-safe tasks in global wave 4 (≥3 parallel-sweep threshold).

Collision rows touching this workstream (from the plan collision matrix):

| Shared file | Tasks | Resolution |
|---|---|---|
| `crates/uaa-control/src/ca.rs` | CT-01 (stub), pki PK-01, pki PK-03 | serialize: wave 3 = CT-01 stub, wave 4 = PK-01 fill, wave 5 = PK-03 extend |
| stub-pattern (uaa-control stubs created by CT-01) | CT-01 + exactly one filler each | stub wave (3) precedes fill wave (4/5); fillers: CT-02..07 here, PK-01 (`ca.rs`+`enroll.rs`), IP-01..04 (`machine_plane/*`) in their own workstreams |

Additional cross-wave facts the coordinator must honor: TASK-06 also needs core-proto CP-03 (FleetConfig) merged; TASK-07 needs `web/dist/.gitkeep` from TASK-08 (global wave 2) and CT-03's `require_role`; the wave-4 peers IP-01..03 and PK-01 fill disjoint uaa-control stubs and are safe to dispatch alongside CT-02..06.

Link: See [ORCHESTRATION.md](../ORCHESTRATION.md) for the coordinator + worker protocol.
