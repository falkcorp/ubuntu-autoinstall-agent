<!-- file: docs/agent-tasks/uaa-web/README.md -->
<!-- version: 1.0.0 -->
<!-- guid: b1bf9513-1cfe-41c1-be68-fe1acc7db3c4 -->
<!-- last-edited: 2026-07-10 -->

# Workstream — uaa-web (boot-artifact server + webroot write plane)

Build the `uaa-web` daemon (`crates/uaa-web`): the ONLY writer under `/var/www/html` (spec C4, Decision 12) and the read-only boot-artifact HTTP server on :8081 behind an explicit path allowlist (Decision 20), plus its :7445 mTLS gRPC plane (`WebService`: placement RPCs, ISO build jobs, signed-binary publish + manifest regeneration). From ws5-web. Spec: [docs/specs/constellation-design.md](../../specs/constellation-design.md) §C4; plan: [docs/specs/constellation-plan.md](../../specs/constellation-plan.md).

| Task | Src id | Title | Priority | Effort | Tier | Wave |
|------|--------|-------|----------|--------|------|------|
| TASK-01 | ws5-web | uaa-web crate: :8081 read-only ServeDir with explicit path allowlist + health + mTLS gRPC listener via tls.rs | P1 | M | Sonnet-class | 6 |
| TASK-02 | ws5-web | PlaceSeed/PlaceIpxe/FlipBootTarget/RemoveHost: typed placeholder gate, atomic tmp+rename writes, flip regex parity | P1 | L | Sonnet-class | 7 |
| TASK-03 | ws5-web | BuildIso/GetBuildJob/ListIsos: detached job runner wrapping the uaa-core iso pipeline (never inline) | P2 | M | Sonnet-class | 7 |
| TASK-04 | ws5-web | PublishAgentBinary (verify detached sig before placement) + update-manifest generation/serving | P1 | M | Sonnet-class | 7 |

Waves are GLOBAL constellation-plan waves. This workstream owns wave 6's `WB-01` (with peer `PX-01`, a disjoint new crate) and three of wave 7's six tasks.

## Ground rules

- Rust only, inside `crates/uaa-web/**` exclusively. TASK-01 creates the crate INCLUDING headered stub files `src/placement.rs`, `src/iso_jobs.rs`, `src/publish.rs`; TASK-02/03/04 each fill EXACTLY ONE of those stubs and touch no other file (this is what makes wave 7 parallel-safe).
- Build + test gate for every task:
  ```bash
  cargo test --lib --offline && cargo build --offline
  # Expected: all tests pass (baseline 311 + everything added by waves 1-6 + your new tests; 0 failed), build clean
  cargo clippy --offline -- -D warnings
  # Expected: no warnings
  ```
- **Verify every file:line anchor with `grep` before editing** — these tasks run in waves 6–7, after five waves of merges; line numbers in the briefs WILL have drifted and `src/**` will have moved to `crates/uaa-core/src/**` (path map below). The grep hits are authoritative; zero hits at both old and mapped paths = STOP and report.
- **Path map:** after CP-01 (wave 1) merges, `src/**` lives at `crates/uaa-core/src/**` and the CLI at `crates/uaa/src/**`. Briefs cite pre-move paths (verifiable on today's main); at execution time run each grep at the old path, then the mapped path.
- File headers MANDATORY: new files get a fresh 4-line `// file: / // version: / // guid: / // last-edited:` header (new uuid4 via `uuidgen | tr 'A-F' 'a-f'`); every edited file gets version bumped + `last-edited` updated, guid preserved.
- HARD RULES (operation contract, restated in every brief):
  - NO hardware actions; validate ONLY in-repo (`cargo`). NEVER wipe, write to, or deploy on 172.16.2.30 ("the server") or len-serv-003. NEVER power on unimatrixone.
  - No real secret in any file: `REPLACE_AT_PLACE_TIME` placeholders stay placeholders (TASK-02's gate ENFORCES this at runtime, fail-closed).
  - All webroot writes atomic (tmp+rename); uaa-web never holds a signing key (TASK-04).
  - Workers stay in their worktree and NEVER `git push` / `gh pr` / merge — report done and stop.

## Collision / wave note

Execution mode: SERIAL WAVES — WB-01 creates the crate, then WB-02/03/04 fill disjoint stubs in parallel — trigger: 3 parallel-safe filler tasks in wave 7 (one stub file each, meets the ≥3 parallel-dispatch threshold once WB-01 is merged).

From the operation collision matrix (stub-pattern row): "uaa-web stubs by WB-01 … each stub file has EXACTLY ONE filling task". Cross-workstream gates:

| Gate | Blocks | Why |
|---|---|---|
| CP-02 merged (wave 2) | TASK-01 | `uaa-proto` crate + `[workspace.dependencies]` (tonic, tower-http, axum, rust-embed, ed25519-dalek …) must exist |
| PK-03 merged (wave 5) | TASK-01 | `crates/uaa-core/src/tls.rs` mTLS helpers for the :7445 listener |
| TASK-01 (WB-01) merged | TASK-02/03/04 | the three stub files must exist on `origin/main` |
| TP-01 + TP-03 merged (waves 2–3) | TASK-03 | `crates/uaa-core/src/iso/{remaster,image_build}.rs` pipeline entry points |
| CP-05 merged (wave 3) | TASK-04 | `crates/uaa-core/src/update.rs` manifest model + dual-pubkey verify |

Wave-7 peers `PX-02/03/04` live in `crates/uaa-pxe/**` — no shared files with this workstream.

Link: See [ORCHESTRATION.md](../ORCHESTRATION.md) for the coordinator + worker protocol.
