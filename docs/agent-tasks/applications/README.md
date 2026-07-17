<!-- file: docs/agent-tasks/applications/README.md -->
<!-- version: 1.0.0 -->
<!-- guid: 1a30363c-89b5-4c97-a9bb-253acc7bc543 -->
<!-- last-edited: 2026-07-16 -->

# Workstream — applications (CockroachDB in the native installer)

Make the native ZFS-on-LUKS installer able to stand up a CockroachDB node end-to-end, and make the QEMU gate able to prove it. Today `cockroach_advertise`/`cockroach_join` exist only in the retiring curtin path as template placeholders, and the real install is `setup_cockroachdb.sh` — a script that lives **only on the netboot server**, is fetched over plain HTTP at first boot, and `rm`s itself. From spec C4/D13–D16.

Design: [`deploy-system-design.md`](../../specs/deploy-system-design.md) · Plan + authoritative wave/tier table: [`deploy-system-plan.md`](../../specs/deploy-system-plan.md) · Bucket sort: [`BREAKDOWN-2026-07-16.md`](../BREAKDOWN-2026-07-16.md)

| Task | Src id | Title | Priority | Effort | Tier | Wave |
|------|--------|-------|----------|--------|------|------|
| TASK-01 | DS-APP-01 | `ApplicationSpec` + defaulted `applications` field (+ `PartialEq`) | P1 | M | Sonnet-class | 1 |
| TASK-02 | DS-APP-02 | `ApplicationInstaller` + Phase-5 wiring (fail-closed scaffold) | P1 | M | Sonnet-class | 2 |
| TASK-03 | DS-APP-03 | Cockroach install step (port `setup_cockroachdb.sh`) | P1 | L | Sonnet-class | 3 |
| TASK-04 | DS-APP-04 | Fix `vm-validate.sh` accepting `degraded` as PASS | **P0** | S | Haiku-class | 2 |
| TASK-05 | DS-APP-05 | VM gate: Cockroach readiness + `vm-test.yaml` spec | P1 | M | Sonnet-class | 5 |

**Waves are GLOBAL across the deploy-system package** (not per-workstream) — see [`../../specs/deploy-system-plan.md`](../../specs/deploy-system-plan.md) § Parallel execution groups. A task does not start until every task it depends on has MERGED.

## Ground rules

- Rust only, in exactly the files each brief names. Purely additive — no existing signature changes.
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

**TASK-02 and TASK-03 both edit `applications.rs` and `installer.rs`** — they MUST run in different waves (TASK-03 serialized after TASK-02 merges); running them in parallel produces a same-file conflict on every rebase cycle. **TASK-04 and TASK-05 both edit `scripts/vm-validate.sh`** — same rule (wave 2, then wave 5).

> **⚠ TASK-04 is P0 and blocks nothing — dispatch it immediately, ahead of wave 1.** `vm-validate.sh` currently accepts `systemctl is-system-running` returning `degraded` as a PASS, and systemd returns `degraded` **precisely when one or more units have FAILED**. The gate that authorizes touching real hardware can pass on a machine with broken services. This is a pre-existing bug, not new work.

> **⚠ TASK-02's `ApplicationInstaller` mirrors `ResetPartitionStager`'s SHAPE but NOT its error handling.** The reset stager is deliberately non-fatal (`warn` and continue); an application failing to install is a **failed deployment** and must propagate with `?`. `system_setup.rs` is saturated with the warn-and-continue idiom, so copying the wrong wrapper is the most likely defect in this workstream.

See [ORCHESTRATION.md](../ORCHESTRATION.md) (one level up) for the coordinator + worker protocol.
