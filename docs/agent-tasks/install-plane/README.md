<!-- file: docs/agent-tasks/install-plane/README.md -->
<!-- version: 1.0.0 -->
<!-- guid: 1bd2933f-a5b6-4526-a2b2-055bb24e2569 -->
<!-- last-edited: 2026-07-10 -->

# Workstream — install-plane (:25000 machine-plane parity)

Port the 25-endpoint Python machine plane (`scripts/autoinstall-agent.py`, the :25000 service) into `crates/uaa-control/src/machine_plane/` with drop-in status/body parity, then prove it with a recorded fixture suite — the M2 gate artifact (spec Decision 12; `docs/specs/constellation-design.md`). IP-01/02/03 each fill exactly one `machine_plane` stub created by `control/TASK-01` (CT-01); IP-04 adds the dashboard trio and the parity fixtures over all three. From ws3-parity.

| Task | Src id | Title | Priority | Effort | Tier | Wave |
|------|--------|-------|----------|--------|------|------|
| TASK-01 | ws3-parity | /autoinstall/* parity: ip-neigh MAC resolution, empty-200 missing-seed-file vs hard-404 uaa-config | P1 | M | Sonnet-class | 4 |
| TASK-02 | ws3-parity | register/checkin/webhook parity: TPM-EK first-bind + mismatch-403, auto-flip-on-success, events.jsonl semantics, install_history persist | P1 | L | Sonnet-class | 4 |
| TASK-03 | ws3-parity | approve/deregister/flip/certs + yubikey + tang endpoint parity (incl. 403-unapproved rules) | P1 | M | Sonnet-class | 4 |
| TASK-04 | ws3-parity | Recorded parity fixture suite (request/response goldens per endpoint) + /dashboard + /api/health + /api/uaa-configs | P1 | M | Sonnet-class | 5 |

## Ground rules

- Rust only, inside `crates/uaa-control/` — each brief names its exact file (`machine_plane/{seeds,lifecycle,inventory,dashboard}.rs`; TASK-04 additionally owns `crates/uaa-control/tests/parity/**`); purely additive stub-fills, no edits to sibling machine_plane files.
- Build + test gate for every task in this workstream:
  ```bash
  cargo test --lib --offline && cargo build --offline
  # Expected: all tests pass (baseline 311 + new), build clean
  cargo clippy --offline -- -D warnings
  # Expected: no warnings
  cargo test -p uaa-control --offline
  # Expected: all uaa-control tests pass; 0 failed
  ```
- **Verify every file:line anchor with `grep` before editing** — these tasks run in waves 4–5, after the CP-01 workspace transform; line numbers WILL have drifted and `src/**` maps to `crates/uaa-core/src/**`. Grep the old path, then the mapped path; zero hits at both = STOP.
- Parity ground truth is `scripts/autoinstall-agent.py` — every status code, error string, and body convention in the briefs' parity tables is transcribed from it with line anchors; the fixture suite (TASK-04) is normative for M2.
- Tests use a mocked Registry trait + tempdir webroot + mock CommandExecutor — **no live CockroachDB, no network, no cockroach binary** in any test.
- File headers MANDATORY: new files get a fresh 4-line header; every edited file gets version bumped + `last-edited` updated, guid preserved.
- HARD RULES (operation contract, restated in every brief): NO hardware actions — cargo + (where stated) the QEMU harness only; NEVER wipe/write 172.16.2.30 or len-serv-003; `disk_device` never guessed; ipmitool via `ssh 172.16.2.30` only; NEVER power on unimatrixone; `REPLACE_AT_PLACE_TIME` placeholders stay placeholders — no real secret in any file or fixture; workers never push/PR/merge.

## Collision / wave note

**Execution mode: PARALLEL DISPATCH within wave — trigger: 3 disjoint wave-4 tasks (≥3 parallel threshold); IP-04 serial after.** Skeleton exec_mode, quoted: "PARALLEL DISPATCH within wave — IP-01/02/03 fill disjoint machine_plane stubs; IP-04 serialized after (depends on all three)".

From the skeleton collision matrix, the only row touching this workstream is the stub-pattern rule: "stub-pattern (… uaa-control stubs by CT-01 …) — serialize: dependency-ordered (stub wave precedes fill wave); each stub file has EXACTLY ONE filling task." Consequences:

| Constraint | Gate |
|---|---|
| IP-01/02/03 (global wave 4) | `control/TASK-01` (CT-01, wave 3) MERGED — it creates the crate + the three stubs; each IP task is the sole filler of its stub, so all three dispatch in parallel |
| IP-04 (global wave 5) | IP-01 AND IP-02 AND IP-03 MERGED — its fixture harness drives their handlers through the shared router |

No IP task shares a file with any other IP task or with any cross-workstream wave-4/5 peer (CT-02..06 fill different uaa-control stubs; PK-01 is `ca.rs`).

Link: See [ORCHESTRATION.md](../ORCHESTRATION.md) for the coordinator + worker protocol.
