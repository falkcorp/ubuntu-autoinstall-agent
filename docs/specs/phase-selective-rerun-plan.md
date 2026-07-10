<!-- file: docs/specs/phase-selective-rerun-plan.md -->
<!-- version: 1.0.0 -->
<!-- guid: 910e69ba-7d2a-4137-aa7f-08e420766f13 -->
<!-- last-edited: 2026-07-09 -->

# Phase-Selective Re-run — Implementation Plan

Companion to [phase-selective-rerun-design.md](phase-selective-rerun-design.md) (decisions there are LOCKED — do not reopen them here or in the briefs). Workstream `phase-rerun`, two tasks, executed in global waves 4 and 5 of the install-ops operation.

**Repo:** `/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent` · default branch `main` · workers in worktrees `$REPO/.worktrees/<ws>-<slug>` on branches `agent/<ws>-<slug>` off `origin/main` · coordinator owns all push/PR/merge.

**Gate (every step):**

- Run: `cargo test --lib --offline`
  Expected: `237+ passed; 0 failed` (baseline 237 — new tests only add)
- Run: `cargo build --offline`
  Expected: exit 0
- Run: `cargo clippy --offline`
  Expected: exit 0, no new warnings

## Locked decisions this plan implements (from the design spec)

1. `--phases <spec>` (`"5"`, `"4-6"`, `"0,1,5"`) / `--from-phase <n>`, mutually exclusive; DEFAULT (no flags) byte-identical to today's full 0–6 run.
2. Non-destructive mount-existing-target prep when phases 2–3 are skipped but later phases need a mounted target: assemble md (`mdadm --assemble --scan`, tolerate already-assembled) → `cryptsetup open` via keyfile → `zpool import -N -R /mnt/targetos rpool` THEN `bpool` → mount `/` (rpool ROOT) BEFORE `/boot` (bpool BOOT) BEFORE ESP. Order is load-bearing (faea48e; zfs_ops.rs:60–67 comment).
3. GUARD: every wipe-capable function (`wipe_disk`, `prepare_disk` destructive branch, `recover_after_failure_and_wipe`) requires a `WipeAuthorization` token only constructible when Phase 2 is selected; `preflight_checks`' residual-state wipe path (installer.rs ~271, call at ~344) is BYPASSED in selective mode.
4. `zpool import` is NEW code — `grep -rn 'zpool import' src/` → 0 hits today.
5. Rejected alternatives (do not resurrect): config-file-driven default phase set (deferred); auto-detect resume point (too magic for a wipe-adjacent path).

Hard operational rules apply to both steps: code/docs only, validated in VM/QEMU; NEVER touch 172.16.2.30 ("the server") or len-serv-003; `disk_device` is read from the live target, never guessed; workers never push/PR/merge.

## Step order and waves

| Step | Task brief | Src id | Title | Priority | Effort | Tier | Global wave | Depends on |
|---|---|---|---|---|---|---|---|---|
| 1 | [docs/agent-tasks/phase-rerun/TASK-01-phase-spec-cli.md](../agent-tasks/phase-rerun/TASK-01-phase-spec-cli.md) | todo:phase-selective | `--phases <spec>` / `--from-phase <n>`: CLI parsing + phase-selection plumbing (full run unchanged by default) | P1 | L | Opus-class · ⚠ review-critical | 4 | merged waves 1–3 |
| 2 | [docs/agent-tasks/phase-rerun/TASK-02-mount-existing-target.md](../agent-tasks/phase-rerun/TASK-02-mount-existing-target.md) | todo:phase-selective | Non-destructive mount-existing-target prep + NEVER-wipe guard when Phase 2/3 skipped | P1 | L | Opus-class · ⚠ review-critical | 5 | phase-rerun/TASK-01 |

Strictly sequential: TASK-02 builds on TASK-01's `PhaseSelection` type and merged `run_phase!` changes. Do not parallelize within this workstream.

### Wave-4 timing rationale (TASK-01)

TASK-01 touches `src/cli/args.rs`, `src/main.rs`, `src/cli/commands.rs`, `src/network/ssh_installer/installer.rs`. Per the operation collision matrix these files are shared with:

- `installer.rs`: installer-robustness/TASK-01 (wave 1), TASK-05 (wave 2), TASK-07 (wave 3), boot-prod/TASK-02 (wave 6)
- `src/cli/commands.rs`: installer-robustness/TASK-02 (wave 1), TASK-03 (wave 2), TASK-07 (wave 3), remote-power/TASK-01 (wave 5)
- `src/cli/args.rs`, `src/main.rs`: remote-power/TASK-01 (wave 5)

Wave 4 starts only after waves 1–3 are MERGED to `main`; the TASK-01 worker branches off `origin/main` and therefore sees the partition-suffix helper (installer-robustness/TASK-01), the LUKS keyfile change (TASK-05), and curtin detection (TASK-07) already in place. remote-power/TASK-01 (wave 5) rebases onto TASK-01's merged args/main/commands changes, not the other way around.

## Step 1 — CLI parsing + phase-selection plumbing (TASK-01, wave 4)

Brief: `docs/agent-tasks/phase-rerun/TASK-01-phase-spec-cli.md`. Implements design components C1 + C2.

1. Re-verify anchors (grep commands from the brief, copied from the evidence file) — especially `grep -n 'run_phase!("Phase' src/network/ssh_installer/installer.rs` (12 hits) and the two entry points `perform_installation` / `perform_installation_with_options_and_pause`.
2. Add `PhaseSelection` + `WipeAuthorization` types to `installer.rs` with `parse` / `from_phase` / `full` / `contains` / `is_explicit` / `authorize_wipe` / `needs_luks_reopen` / `needs_pool_import` exactly as specified in the design's Data model section. Parsing is fail-closed (empty/`>6`/reversed-range/non-digit → `Err`).
3. Add `phases: Option<String>` (`conflicts_with = "from_phase"`) and `from_phase: Option<u8>` to `Install`, `SshInstall`, `LocalInstall` in `args.rs`; thread through `main.rs` match arms into `install_command` / `ssh_install_command` / `local_install_command` in `commands.rs`; build the selection before connecting, erroring out on invalid specs.
4. Extend `perform_installation_with_options_and_pause` (and `perform_installation`) with `&PhaseSelection`; when `!is_explicit()` take the EXACT existing path; when explicit, gate each `run_phase!` with `contains(n)` and log skips.
5. Tests: parse matrix, default-is-full, wipe-token denial without Phase 2, needs-prep matrix, mock-executor proof that `--phases 5` issues no `wipefs`/`sgdisk`/`debootstrap` commands, and the default-run byte-identity test (design Testing table).
6. Bump file headers (version + last-edited) on all four touched files.

- Run: `cargo test --lib --offline`
  Expected: `237+ passed; 0 failed` (new `phase_selection` tests included)
- Run: `cargo build --offline && cargo clippy --offline`
  Expected: exit 0, no new warnings
- Run: `grep -n 'conflicts_with' src/cli/args.rs | grep -c from_phase`
  Expected: `3` (one per install-flavored subcommand)
- Run: `grep -rn 'zpool import' src/`
  Expected: 0 hits — Step 1 must NOT introduce import code; that is Step 2's scope

Merge gate: coordinator merges TASK-01 to `main` (wave-4 close) before Step 2 begins.

## Step 2 — Mount-existing-target prep + NEVER-wipe guard (TASK-02, wave 5)

Brief: `docs/agent-tasks/phase-rerun/TASK-02-mount-existing-target.md`. Implements design components C3 + C4. Depends on TASK-01 merged (uses `PhaseSelection`/`WipeAuthorization`) and on installer-robustness/TASK-05's keyfile helper (merged in wave 2).

1. Re-verify anchors: `grep -n 'recover_after_failure_and_wipe' src/network/ssh_installer/installer.rs` (~344), `grep -n 'async fn wipe_disk' src/network/ssh_installer/disk_ops.rs` (~222), `grep -rn 'zpool import' src/` (0 hits), `grep -n -- '-R /mnt/targetos' src/network/ssh_installer/zfs_ops.rs` (6 hits), `grep -n 'fn final_cleanup' src/network/ssh_installer/system_setup.rs` (~915), the `ORDER IS LOAD-BEARING` comment in zfs_ops.rs (~60–67).
2. Thread `&WipeAuthorization` into `wipe_disk`, `prepare_disk`'s destructive chain, and `recover_after_failure_and_wipe`; obtain tokens only via `selection.authorize_wipe()`. Compile failure at any wipe call site without a token is the point.
3. Bypass the preflight residual-state wipe when `is_explicit() && !contains(2)`: log residual state as EXPECTED input and continue; keep connectivity/mirror checks unchanged in all modes. Full-run behavior (wipe on residual) must be preserved verbatim.
4. Add prep helpers: md assemble (`mdadm --assemble --scan || true`, only for `/dev/md*` devices), keyfile LUKS reopen (skip when `cryptsetup status luks` succeeds; reuse TASK-05's 0600-tempfile + shred helper; never interpolate the key), NEW `zpool import -N -R /mnt/targetos` for rpool THEN bpool (skip pools already imported), ordered mounts `/` → `/boot` → ESP (`mountpoint -q`-guarded; ESP via `choose_esp_partition`), and stale-bind normalization reusing `final_cleanup`'s unmount inverse ops WITHOUT its export/close steps.
5. Wire prep into the selective path of `perform_installation_with_options_and_pause`: runs after preflight, before the first selected phase, only when `needs_luks_reopen()`/`needs_pool_import()`. Any hard prep failure aborts before any phase runs.
6. Tests: preflight-bypass (plus anti-over-suppression: full run with residual state STILL wipes), prep idempotency matrix (already-imported / already-open / partial-mount cases), mount-order sequence assertion, wipe-guard compile-level coverage via the `authorize_wipe` unit tests.
7. Bump file headers on `installer.rs`, `disk_ops.rs`, `zfs_ops.rs`.

- Run: `cargo test --lib --offline`
  Expected: `237+ passed; 0 failed` (incl. new prep/guard tests)
- Run: `cargo build --offline && cargo clippy --offline`
  Expected: exit 0, no new warnings
- Run: `grep -rn 'zpool import' src/ | wc -l`
  Expected: `>= 2` (rpool + bpool import commands now exist, only in zfs_ops.rs)
- Run: `grep -n 'WipeAuthorization' src/network/ssh_installer/disk_ops.rs | wc -l`
  Expected: `>= 3` (wipe_disk, prepare_disk chain, recover_after_failure_and_wipe all guarded)

Merge gate: coordinator merges TASK-02 at wave-5 close. Note wave-5 sibling remote-power/TASK-01 shares NO files with TASK-02 (its overlap — args.rs/main.rs/commands.rs — is with the already-merged TASK-01), so both wave-5 tasks may run concurrently.

## Post-merge validation (before ANY hardware use)

The feature is wipe-adjacent; it must pass the QEMU gate (testing-gates/TASK-01, `scripts/vm-validate.sh`) before anyone types `--phases` at real hardware:

- Run: full VM install, then force a Phase-5 failure, then `uaa install … --phases 5`
  Expected: VM boots; debootstrap ran exactly once across both invocations; no `wipefs`/`sgdisk` in the second invocation's logs
- Run: `uaa install …` (no flags) against a VM with residual state
  Expected: today's behavior exactly — preflight recovers/wipes and the full run proceeds

Never validate on 172.16.2.30 or len-serv-003 (HARD RULE 1). Human operators own any eventual hardware run.

## Rollback

Two independent `git revert`s, in reverse order (TASK-02 then TASK-01), restore today's behavior exactly; nothing is persisted and the default flagless path never changed. See the design spec's Rollback section.

See [ORCHESTRATION.md](../agent-tasks/ORCHESTRATION.md) and [docs/agent-tasks/phase-rerun/README.md](../agent-tasks/phase-rerun/README.md) for the coordinator + worker protocol.
