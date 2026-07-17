<!-- file: docs/agent-tasks/operator-api/README.md -->
<!-- version: 1.0.0 -->
<!-- guid: 47a53a37-c8df-4048-a219-96d00bef3e04 -->
<!-- last-edited: 2026-07-16 -->

# Workstream — operator-api (HTTP surface, place-from-registry, SPA)

Expose profiles and drift review over the authenticated operator plane, wire resolution into `config place`, and render it. From spec C7, D-B, M4.

Design: [`deploy-system-design.md`](../../specs/deploy-system-design.md) · Plan + authoritative wave/tier table: [`deploy-system-plan.md`](../../specs/deploy-system-plan.md) · Bucket sort: [`BREAKDOWN-2026-07-16.md`](../BREAKDOWN-2026-07-16.md)

| Task | Src id | Title | Priority | Effort | Tier | Wave |
|------|--------|-------|----------|--------|------|------|
| TASK-01 | DS-OPS-01 | `/api/profiles` + `/api/groups` route group + DTOs | P2 | M | Sonnet-class | 4 |
| TASK-02 | DS-OPS-02 | `/api/drift` review routes (accept/revert) | P2 | M | Sonnet-class | 5 |
| TASK-03 ⚠ | DS-OPS-03 | `config place --from-registry` (dry-run default, `.bak`) | P1 | L | **Opus-class** | 6 |
| TASK-04 | DS-OPS-04 | SPA: profile + drift screens, staleness rendering | P3 | M | Haiku-class | 6 |

**Waves are GLOBAL across the deploy-system package** (not per-workstream) — see [`../../specs/deploy-system-plan.md`](../../specs/deploy-system-plan.md) § Parallel execution groups. A task does not start until every task it depends on has MERGED.

## Ground rules

- Rust only (except the SPA workstream), in exactly the files each brief names. Purely additive — no existing signature changes.
- Build + test gate for every task in this workstream:
  ```bash
  cargo test --lib --offline && cargo build --offline
  # Expected: >=634 passed (planning baseline), build exit 0
  cargo clippy --offline -- -D warnings
  # Expected: no warnings
  ```
- **Verify every file:line anchor with `grep` before editing** — line numbers in each brief are a starting point, not a guarantee. Zero hits = STOP and report.
- File headers MANDATORY: new files get a fresh 4-line header with a new uuid4; edited files keep their guid and bump `version` + `last-edited`.
- **NO SQL, NO migration anywhere in this package.** `uaa-control` has no database connection in production (`default_state()` builds `FileRegistry` + `Mem*Store`; `db::migrations::apply` has no caller). Profiles persist in the `StatePaths` snapshot — spec [D4](../../specs/deploy-system-design.md).
- HARD RULES (restated in every brief): NO hardware actions — all commands go through `CommandExecutor` mocks; NEVER wipe/write/deploy on 172.16.2.30 or len-serv-003; NEVER power on unimatrixone; `disk_device` read from the live target, never guessed; no real secret anywhere (`REPLACE_AT_PLACE_TIME` stays a placeholder); workers stay in their worktree and never push/PR/merge.

## Collision / wave note

**TASK-01 and TASK-02 both edit `operator/handlers.rs` and `operator/api_types.rs`** — they MUST run in different waves (TASK-02 serialized after TASK-01 merges). TASK-03 (`config_place.rs`) and TASK-04 (`web/**`) are single-writer and collide with nothing.

> **⚠ TASK-01/02: a route added outside `build_router`'s role-grouping convention is silently UNAUTHENTICATED.** That is the exact class of bug PR #92 fixed when `auth.rs` existed but was unmounted. Every mutating route is `require_role(.., Role::Operator)`-wrapped and takes `Extension<auth::Session>`; the actor is `&session.login`, never a placeholder.

> **⚠ TASK-03 is the ONLY behavior-changing task in the package, and it overwrites data.** `place_configs` does an in-place `fs::write` of every host's `/var/www/html/cloud-init/<hexmac>/uaa.yaml` with no backup. `--from-registry` defaults **off**, `--dry-run` defaults **on**, and a `.bak` precedes every overwrite. Opus-class, review-critical, never parallelized. **Flipping the flag in production is Bucket 3 — an operator action after a human reviews the dry-run diff.**

See [ORCHESTRATION.md](../ORCHESTRATION.md) (one level up) for the coordinator + worker protocol.
