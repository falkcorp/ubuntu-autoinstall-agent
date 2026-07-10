<!-- file: docs/agent-tasks/core-proto/README.md -->
<!-- version: 1.0.0 -->
<!-- guid: 119eac65-2baf-498a-b11e-5203c27a92f5 -->
<!-- last-edited: 2026-07-10 -->

# Workstream — core-proto (workspace foundation, proto surface, fleet config, discovery, self-update, musl matrix)

Convert the repo into the constellation cargo workspace (uaa-core lib + uaa bin, behavior frozen at 311 tests), add the `uaa-proto` crate + full `[workspace.dependencies]`, and land the shared uaa-core foundations every other workstream builds on: `FleetConfig`, mDNS/static discovery, the signed self-update library, and the per-binary musl CI matrix. Scope, locked decisions, and data model come from the spec: [docs/specs/constellation-design.md](../../specs/constellation-design.md) (Decisions 2, 9, 10, 11, 17, 18; components C1/C2/C7). From ws1-core.

**Execution mode:** W1 SINGLE-AGENT (strong model) — CP-01 is judgment work colliding with everything; W2 PARALLEL DISPATCH within wave — disjoint files per collision matrix — trigger: 6 tasks, 3 of them parallel-safe in wave 2 (≥3 parallel-sweep threshold met inside wave 2; wave 1 is always a single dispatch).

| Task | Src id | Title | Priority | Effort | Tier | Wave |
|------|--------|-------|----------|--------|------|------|
| TASK-01 | ws1-core | Convert to cargo workspace (uaa-core lib + uaa bin) with pre-declared stub modules and CLI variants | P1 | L | Opus-class | 1 |
| TASK-02 | ws1-core | uaa-proto crate: proto/uaa/** packages, protox build.rs, full workspace.dependencies population | P1 | M | Sonnet-class | 2 |
| TASK-03 | ws1-core | FleetConfig: parameterize hardcoded fleet constants behind /etc/uaa/fleet.yaml with today's values as defaults | P1 | M | Sonnet-class | 2 |
| TASK-04 | ws1-core | discovery.rs: mDNS advertise (daemons only) + resolve() returning UNION of mDNS+static candidates | P1 | M | Sonnet-class | 3 |
| TASK-05 | ws1-core | update.rs: manifest model, dual-pubkey ed25519 verify, min_version floor, stage/apply modes, hold pin, prev-swap rollback | P1 | M | Sonnet-class | 3 |
| TASK-06 | ws1-core | musl-build.yml: build every workspace binary, static-verify each, artifact per binary | P2 | S | Haiku-class | 2 |

(Waves are GLOBAL wave numbers from the constellation skeleton; this workstream's local waves map 1→[TASK-01], 2→[TASK-02, TASK-03, TASK-06], 3→[TASK-04, TASK-05].)

## Ground rules

- Rust + proto + one workflow yml, in exactly the files each brief names. TASK-01 is the ONLY transform (git-mv workspace conversion, behavior frozen); every other task is purely additive and fills a CP-01 stub or extends a manifest/workflow without touching sibling files.
- Build + test gate for every task in this workstream:
  ```bash
  cargo test --lib --offline && cargo build --offline
  # Expected: baseline 311 passing lib tests (+ each task's new tests), 0 failed
  cargo clippy --offline -- -D warnings
  ```
- **Verify every file:line anchor with `grep` before editing** — TASK-02..06 run AFTER the wave-1 workspace conversion, so every `src/**` path in the skeleton has moved: grep the old path, then the mapped path (`src/**` → `crates/uaa-core/src/**`, CLI → `crates/uaa/src/**`); zero hits at BOTH = STOP and report.
- File headers MANDATORY: new files get a fresh 4-line header (`file:`/`version: 1.0.0`/`guid:` uuid4/`last-edited:`); edited files get version bumped + `last-edited` updated, guid preserved; files MOVED by TASK-01 keep their guid with the `file:` path updated.
- HARD RULES (operation contract, restated in the briefs): NO hardware actions — validation is cargo (+ VM harness only where a brief says so); NEVER wipe/write/deploy on 172.16.2.30 ("the server") or len-serv-003; ipmitool only via `ssh 172.16.2.30`; NEVER power on unimatrixone; no real secret in any file (`REPLACE_AT_PLACE_TIME` stays a placeholder; the update-signing private key lives offline, tests use throwaway keypairs); workers stay in their worktree and never push/PR/merge.

## Collision / wave note

Collision rows from the operation skeleton that involve this workstream:

| Shared file | Colliding tasks | Resolution |
|---|---|---|
| `Cargo.toml` (root) | TASK-01 (CP-01), TASK-02 (CP-02) | serialize: wave 1 = CP-01, wave 2 = CP-02 |
| `crates/uaa-core/src/power/mod.rs` | TASK-01 (CP-01), TASK-03 (CP-03) | serialize: wave 1 = CP-01 (stub lines), wave 2 = CP-03 (fill) |
| `crates/uaa-core/src/luks_keys.rs` | TASK-01 (CP-01), luks-keys LK-01/LK-02 | serialize: wave 1 = CP-01 (stub), wave 2 = LK-01, wave 3 = LK-02 |
| stub-pattern (all 14 uaa-core stubs + 6 CLI pre-wirings created by CP-01) | creator + exactly one filler each | dependency-ordered: the stub wave (1) precedes every fill wave; each stub file has EXACTLY ONE filling task |

Wave discipline: **nothing anywhere in the constellation plan dispatches until TASK-01 is merged** (it collides with everything). Wave 2 (TASK-02/03/06 + cross-WS peers TG-04, LK-01, TP-01/02/04, CT-08) is parallel-safe because each task fills its own stub / own new file and only CP-02 touches the root Cargo.toml. Wave 3 (TASK-04/05) needs CP-02's workspace deps merged. Cross-workstream consumers: CP-02 gates CT-01/PK-02/WB-01/PX-01; CP-03 gates CT-06/RP-02/RP-03; CP-05 gates WB-04.

Execution mode (from the skeleton, stamped): `W1 SINGLE-AGENT (strong model) — CP-01 is judgment work colliding with everything; W2 PARALLEL DISPATCH within wave — disjoint files per collision matrix` — trigger: 3 parallel-safe tasks in wave 2.

See [ORCHESTRATION.md](../ORCHESTRATION.md) for the coordinator + worker protocol.
