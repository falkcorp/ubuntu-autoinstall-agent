<!-- file: docs/agent-tasks/phase-rerun/TASK-02-mount-existing-target.md -->
<!-- version: 1.0.0 -->
<!-- guid: aa87b757-7777-45ad-a430-2ac632ae9358 -->
<!-- last-edited: 2026-07-09 -->

# TASK-02 — Non-destructive mount-existing-target prep (assemble md, open LUKS, import rpool then bpool, mount `/` then `/boot` then ESP, binds) + NEVER-wipe guard when Phase 2/3 skipped (todo:phase-selective)

**Priority:** P1 · **Effort:** L · **Recommended subagent:** Opus-class · rust-installer subagent · **Why:** wipe-adjacent: `preflight_checks` currently WIPES residual state; `zpool import` is entirely NEW code; the mount ORDER is the historical grub-install root cause (faea48e) · ⚠ review-critical · **Depends on:** TASK-01 (`PhaseSelection` / `WipeAuthorization` types must be merged; global wave 5)

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/phase-rerun-mount-existing-target" -b agent/phase-rerun-mount-existing-target origin/main
cd "$REPO/.worktrees/phase-rerun-mount-existing-target"
git rebase origin/main
```

(Protocol is also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

## Goal

Make `--phases 5` (and every selection that skips Phases 2–3 but runs 4+) actually work against an EXISTING installed disk: (a) thread TASK-01's `WipeAuthorization` token into the wipe-capable `DiskManager` function signatures so the compiler — not convention — forbids wiping without Phase 2; (b) convert `preflight_checks`' selective-mode residual-state refusal (TASK-01's hard error) into a supported bypass: residual state is the EXPECTED input, log and continue, never wipe; (c) add a non-destructive **mount-existing-target prep** that runs after preflight and before the first selected phase: normalize stale mounts (umount only — NEVER `zpool export`/`cryptsetup close`), assemble md, open LUKS via keyfile, `zpool import -N -R /mnt/targetos` rpool THEN bpool (NEW code — no `zpool import` exists in src/ today), then mount `/` THEN `/boot` THEN the ESP — this ORDER IS LOAD-BEARING (reversed order shadowed `/boot` and broke grub-probe; root cause fixed by faea48e; the authoritative comment is at zfs_ops.rs:60–67, anchor below).

REUSE — do not invent parallel machinery:

- `PhaseSelection` methods from TASK-01: `is_explicit()`, `contains()`, `authorize_wipe()`, `needs_luks_reopen()`, `needs_pool_import()` — verify they exist: `grep -n 'fn needs_pool_import\|fn needs_luks_reopen\|fn authorize_wipe' src/network/ssh_installer/installer.rs` (expect 3 hits; zero hits = TASK-01 not merged, STOP and report).
- The LUKS **keyfile helper from installer-robustness/TASK-05** (merged in global wave 2 — it replaced `echo '<key>' | cryptsetup …` with a 0600 tempfile + shred): locate via `grep -n 'key-file\|keyfile' src/network/ssh_installer/disk_ops.rs` (expect ≥ 1 hit; zero hits = wave ordering violated, STOP and report). Reuse it for the re-open; NEVER interpolate `config.luks_key` into a command line.
- The **suffix-aware partition-path helper from installer-robustness/TASK-01** (merged in global wave 1) for the p4 (LUKS) and p1 (ESP-fallback) device paths: `grep -n 'pub fn' src/network/ssh_installer/partitions.rs` (expect ≥ 1 hit; if the file is missing, fall back to `grep -rn 'fn partition_path\|part_suffix' src/network/ssh_installer/` — zero hits everywhere = STOP and report). Do NOT write a new `format!("{}p4", …)`.
- The exact residual-check command strings already used by `preflight_checks` (`zpool list -H rpool >/dev/null 2>&1`, `zpool list -H bpool >/dev/null 2>&1`, `cryptsetup status luks >/dev/null 2>&1`) — reuse them verbatim as the prep guards so mocks and preflight agree.
- The ESP GUID-detection command: copy the command string built by `detect_esp_partition_path`/`build_esp_detection_command` in `system_setup.rs` (copy-from source anchor below). Do NOT edit `system_setup.rs` — it is not in this task's file list.
- Test scaffolding from TASK-01: `RecordingExecutor` + `SshInstaller::for_tests` in `installer.rs`'s test module (`grep -n 'struct RecordingExecutor\|fn for_tests' src/network/ssh_installer/installer.rs`, expect 2 hits).

Files you may touch — exactly these: `src/network/ssh_installer/installer.rs` (orchestration + preflight bypass), `src/network/ssh_installer/disk_ops.rs` (auth params; md-assemble + LUKS-reopen helpers), `src/network/ssh_installer/zfs_ops.rs` (import + ordered-mount helpers).

## Background (verify before editing)

- Spec (LOCKED decisions — especially Decision 6 mount order and Decision 8 `import -N`): `docs/specs/phase-selective-rerun-design.md` §C3, §C4; plan: `docs/specs/phase-selective-rerun-plan.md`.
- There is NO `zpool import` anywhere in src/ — pools are only created fresh in Phase 3; import is NEW code.
- Phase 3 creates pools with altroot `-R /mnt/targetos`; datasets auto-mount at creation. The creation-order comment at zfs_ops.rs:60–67 documents WHY `/` must mount before `/boot` (bpool `/boot` mounted first gets shadowed by the later rpool `/` mount → grub-probe resolves `/boot` to the LUKS device → grub-install dies "unknown filesystem"). Your prep mount order re-applies the same lesson at import time.
- `preflight_checks` calls `recover_after_failure_and_wipe` on residual state under a `selection.authorize_wipe()` match (TASK-01); the `None` arm currently hard-errors. Preflight errors are only LOGGED by both run sequences — the run continues — so your bypass must return `Ok` and the prep must make the run actually succeed.
- The inverse ops to reuse for normalization live in `final_cleanup` (system_setup.rs ~915): `umount -R /mnt/targetos/{sys,proc,dev,run} || true`, `umount /mnt/targetos/boot/efi || true`, then `zpool export` / `cryptsetup close`. Prep replays ONLY the umount subset — healthy pools and mappers are reused, not torn down.
- Chroot bind mounts are NOT part of prep: the existing idempotent `mountpoint -q … || mount --rbind …` blocks in `configure_system_in_chroot` (Phase 4) and `configure_grub_in_chroot` (Phase 5) re-establish them whenever those phases run. Name-and-reuse, do not duplicate.

- **Re-verify these anchors before editing** — line numbers drift, they are a starting point only:
  ```bash
  grep -rn 'zpool import' src/
  # expect: 0 hits (before your change — the import helpers are NEW code)
  grep -n -- '-R /mnt/targetos' src/network/ssh_installer/zfs_ops.rs
  # expect: 6 hits; the two command builders at ~lines 181 (rpool) and 200 (bpool)
  grep -n 'ORDER IS LOAD-BEARING' src/network/ssh_installer/zfs_ops.rs
  # expect: 1 hit (comment block ~lines 60-67) — the re-verify anchor for the mount order
  grep -n 'recover_after_failure_and_wipe' src/network/ssh_installer/installer.rs
  # expect: 1 hit ~line 344 (fn defined in disk_ops.rs ~line 49)
  grep -n 'async fn wipe_disk' src/network/ssh_installer/disk_ops.rs
  # expect: 1 hit ~line 222
  grep -n 'wipefs -a' src/network/ssh_installer/disk_ops.rs
  # expect: 1 hit ~line 227
  grep -n 'cryptsetup open' src/network/ssh_installer/disk_ops.rs
  # expect: 1 hit ~line 348 (post-TASK-05 it uses the keyfile form — reuse its helper)
  grep -n 'pub async fn prepare_disk\|pub async fn recover_after_failure_and_wipe' src/network/ssh_installer/disk_ops.rs
  # expect: 2 hits — ~22 and ~49
  grep -n 'fn final_cleanup' src/network/ssh_installer/system_setup.rs
  # expect: 1 hit ~line 915 (copy-from source for the umount subset ONLY)
  grep -n 'mount --rbind /dev' src/network/ssh_installer/system_setup.rs
  # expect: 2 hits ~lines 244 and 495 (the existing idempotent bind blocks Phases 4/5 re-run — do NOT duplicate in prep)
  grep -n 'fn detect_esp_partition_path' src/network/ssh_installer/system_setup.rs
  # expect: 1 hit ~line 67 (copy-from source for the ESP GUID-detection command string)
  grep -n 'zpool list -H rpool' src/network/ssh_installer/installer.rs
  # expect: 1 hit inside preflight_checks (~line 325) — reuse this exact check string in prep guards
  ```
  Zero hits on any anchor (other than the expected-0 `zpool import`) = STOP and report; do not guess.

- HARD RULES (restated): (1) NEVER run this against 172.16.2.30 ("the server") or len-serv-003 — code + unit tests only; end-to-end validation happens in QEMU (`scripts/vm-validate.sh`, testing-gates/TASK-01) BEFORE any hardware attempt. (2) `disk_device` comes from config/live detection — never hardcode `/dev/sd*` (U1's `/dev/sda` is an IMSM RAID member; the real volume is `/dev/md126` — hence the mdadm step). (3) SECRETS: `config.luks_key` goes through the 0600-tempfile keyfile helper only; never into a command line, never logged, no real values in tests. (4) Stay in your worktree; NEVER push/PR/merge — the coordinator owns git.

## Step-by-step

1. Run every anchor grep above (and the three REUSE greps in Goal). Any unexpected zero-hit → STOP and report.
2. **`disk_ops.rs` — thread the token (signature change; every call site listed).** Add `_auth: &crate::network::ssh_installer::installer::WipeAuthorization` as the last parameter of `wipe_disk`, `prepare_disk`, and `recover_after_failure_and_wipe`. Inside `prepare_disk`, pass `_auth` through to its `self.wipe_disk(config, _auth)` call. Call sites to update: `phase_2_disk_preparation` in `installer.rs` (obtain the token via `selection.authorize_wipe()` — TASK-01 already gates entry to Phase 2 on it, so thread the token down: give `phase_2_disk_preparation` an `auth: &WipeAuthorization` parameter and pass it from the run-loop's `if let Some(_wipe_auth) = …` binding), and the preflight residual branch (step 3). `cleanup_existing_mounts` and `destroy_existing_zfs_pools` stay private and are reached only through `prepare_disk`/`recover_after_failure_and_wipe`, so they inherit the guard. Purely mechanical otherwise — do NOT alter any command string in these functions.
3. **`installer.rs` — preflight bypass.** In `preflight_checks`' residual-state branch, replace TASK-01's `None => return Err(…)` arm with a logged bypass:
   ```rust
   None => {
       info!(
           "Preflight: residual state detected (bpool={} rpool={} luks={} mounts={}) — expected in selective mode; NOT wiping",
           has_bpool, has_rpool, luks_active, target_has_mounts
       );
   }
   ```
   The `Some(_auth)` arm now calls `recover_after_failure_and_wipe(config, &_auth)`. Edge case, spelled out: the bypass arm is reachable ONLY with an explicit selection omitting Phase 2; every flagless run still has `authorize_wipe() == Some` and wipes on residual exactly as today. Connectivity and mirror checks are unchanged in all modes.
4. **`disk_ops.rs` — two small prep helpers on `DiskManager` (non-destructive; no auth token — they must be callable without wipe rights):**
   - `pub async fn assemble_md_if_needed(&mut self, config: &InstallationConfig) -> Result<()>` — only when `config.disk_device.starts_with("/dev/md")`: run `mdadm --assemble --scan || true` (tolerate ok/already-assembled — mdadm exits non-zero when there is nothing new to assemble; that is success here, hence the `|| true`). Otherwise no-op.
   - `pub async fn reopen_luks_if_needed(&mut self, config: &InstallationConfig) -> Result<()>` — idempotency FIRST: if `check_silent("cryptsetup status luks >/dev/null 2>&1")` is true, log "LUKS mapper already open; skipping" and return `Ok` ("LUKS already open" failure mode). Otherwise open with the TASK-05 keyfile helper against partition 4 of `config.disk_device` (path via the partitions helper): `cryptsetup open <p4> luks --key-file <0600 tempfile>`, then shred the tempfile — exactly the pattern `setup_luks_encryption` uses post-TASK-05. A wrong key is a HARD error (propagate `Err` — fail-closed, nothing half-runs).
5. **`zfs_ops.rs` — import + ordered-mount helpers on `ZfsManager` (ALL new code):**
   - `pub async fn import_pools_for_rerun(&mut self) -> Result<()>` — for `rpool` THEN `bpool`, in that order: skip if `check_silent("zpool list -H <pool> >/dev/null 2>&1")` is already true ("pool already imported" failure mode — reuse preflight's exact check strings); otherwise `zpool import -N -R /mnt/targetos <pool>`. `-R` keeps datasets under the altroot; `-N` defers ALL mounting to the next helper so the order stays ours (LOCKED Decision 8). If import fails (no such pool), propagate the `Err` with the pool name — fail-closed before any phase runs.
   - `pub async fn mount_target_for_rerun(&mut self, esp_partition: &str) -> Result<()>` — the LOAD-BEARING order (LOCKED Decision 6; same lesson as the zfs_ops.rs:60–67 comment):
     1. Discover the root dataset: `zfs list -H -o name -r rpool/ROOT | grep -m1 '^rpool/ROOT/ubuntu_'` via `execute_with_output`; empty output → hard `Err` ("no rpool/ROOT/ubuntu_* dataset found — is this an installed target?").
     2. Mount `/` FIRST: `mountpoint -q /mnt/targetos || zfs mount <root_dataset>`.
     3. THEN `/boot`: derive `bpool/BOOT/ubuntu_<uuid>` from the same uuid suffix; `mountpoint -q /mnt/targetos/boot || zfs mount bpool/BOOT/ubuntu_<uuid>`.
     4. THEN remaining child datasets: `zfs mount -a || true` (children nest under the already-mounted `/` and cannot shadow `/boot`).
     5. THEN the ESP, LAST: `mkdir -p /mnt/targetos/boot/efi; mountpoint -q /mnt/targetos/boot/efi || mount <esp_partition> /mnt/targetos/boot/efi`.
     Every mount is `mountpoint -q … ||`-guarded (partial-mounts failure mode → idempotent re-run).
6. **`installer.rs` — prep orchestration.** New private method `async fn mount_existing_target(&mut self, config: &InstallationConfig, selection: &PhaseSelection) -> Result<()>`, called in BOTH run sequences immediately after `preflight_checks` and before the first `run_phase!`, gated on `if selection.needs_luks_reopen() || selection.needs_pool_import() { self.mount_existing_target(config, selection).await?; }` (a hard prep failure aborts the run BEFORE any selected phase — fail-closed). Body, in order:
   1. **Normalize stale state** (partial mounts / stale binds failure mode): replay ONLY the umount inverse ops from `final_cleanup` (copy the command strings verbatim from the anchor at system_setup.rs ~915): `umount -R /mnt/targetos/sys || true`, same for `/proc`, `/dev`, `/run`, then `umount /mnt/targetos/boot/efi || true`. NEVER `zpool export`, NEVER `cryptsetup close` — healthy pools/mappers are reused, not torn down.
   2. If `selection.needs_luks_reopen()`: `DiskManager::assemble_md_if_needed` then `reopen_luks_if_needed` (step 4).
   3. If `selection.needs_pool_import()`: `ZfsManager::import_pools_for_rerun` then `mount_target_for_rerun` (step 5). For the ESP argument, run the GUID-detection command copied verbatim from `detect_esp_partition_path`/`build_esp_detection_command` (anchor above) via `execute_with_output`; if the trimmed output is empty, fall back to partition 1 of `config.disk_device` via the partitions helper.
   4. Chroot binds: do NOTHING — the idempotent `mountpoint -q … || mount --rbind …` blocks in `configure_system_in_chroot` (~244) and `configure_grub_in_chroot` (~495) establish them when Phases 4/5 run. (Named here so nobody re-invents them.)
7. **Tests** (extend `installer.rs`'s test module reusing `RecordingExecutor` + `for_tests`; zfs helper tests may live in `zfs_ops.rs`'s existing test module). All configs use fake values (`luks_key: "test-not-real"`). Semantics restated for each failure mode:
   - `test_preflight_selective_skips_recovery_wipe` — residual presets (`zpool list -H rpool >/dev/null 2>&1` → true) + `parse("5")`: `preflight_checks` returns `Ok` AND the recorded stream contains no `wipefs`, no `sgdisk --zap-all`, no `zpool destroy`, no `cryptsetup close`.
   - `test_default_run_still_wipes_on_residual` (from TASK-01) must STILL pass unchanged (anti-over-suppression: full flagless run with residual state wipes + installs as today — the bypass must not leak into the default path).
   - `test_prep_pool_already_imported_skips_import` — preset both `zpool list -H rpool…` and `…bpool…` true: recorded stream contains NO `zpool import`.
   - `test_prep_luks_already_open_skips_open` — preset `cryptsetup status luks >/dev/null 2>&1` true: recorded stream contains NO `cryptsetup open`.
   - `test_prep_mount_order_root_then_boot_then_esp` — preset the `zfs list…rpool/ROOT` output to `rpool/ROOT/ubuntu_abc123`: in the recorded stream, index of the `zfs mount rpool/ROOT/ubuntu_abc123` command < index of `zfs mount bpool/BOOT/ubuntu_abc123` < index of the `/mnt/targetos/boot/efi` mount command (THE faea48e regression test).
   - `test_prep_normalizes_partial_mounts_first` — the four `umount -R /mnt/targetos/…` commands and the ESP umount appear BEFORE any `zpool import`/`zfs mount`, and NO `zpool export` appears anywhere in prep.
   - `test_prep_import_uses_altroot_no_automount` — every `zpool import` command contains both `-N` and `-R /mnt/targetos`.
8. Bump the file header (`// version:` minor bump + `// last-edited: <today>`) on all three touched files; keep existing guids.

## How to test

```bash
cargo test --lib --offline
# Expected: >= 252 passed; 0 failed (TASK-01's total + the new prep/guard tests; zero baseline regressions)
cargo build --offline
# Expected: exit 0 — compilation proves the WipeAuthorization threading: no wipe-capable fn is callable without a token
cargo clippy --offline
# Expected: exit 0, no new lints
```

(Do NOT attempt an end-to-end run from this task: the on-hardware/VM validation is testing-gates/TASK-01's `scripts/vm-validate.sh` — full install, induced Phase-5 failure, then `--phases 5` re-run to a bootable VM without a second debootstrap.)

## Acceptance criteria

- [ ] `cargo test --lib --offline` exits 0, ≥ 252 passed / 0 failed; `cargo build --offline` and `cargo clippy --offline` exit 0.
- [ ] `grep -n 'WipeAuthorization' src/network/ssh_installer/disk_ops.rs` → ≥ 3 hits (`wipe_disk`, `prepare_disk`, `recover_after_failure_and_wipe` signatures) — the compiler-enforced guard is threaded.
- [ ] `grep -rn 'zpool import' src/network/ssh_installer/zfs_ops.rs` → ≥ 1 hit containing both `-N` and `-R /mnt/targetos` (and `grep -rn 'zpool import' src/` hits ONLY in `zfs_ops.rs` + its tests).
- [ ] `grep -n 'mdadm --assemble --scan' src/network/ssh_installer/disk_ops.rs` → 1 hit ending `|| true` (tolerates already-assembled).
- [ ] `grep -n 'fn mount_existing_target' src/network/ssh_installer/installer.rs` → 1 hit, and `grep -n 'needs_luks_reopen() || ' src/network/ssh_installer/installer.rs` → ≥ 1 hit per run sequence (prep gated, both sequences).
- [ ] `grep -n "echo '" src/network/ssh_installer/disk_ops.rs | grep -c cryptsetup` → 0 (no passphrase interpolation anywhere — keyfile only).
- [ ] Failure-mode tests pass: `cargo test --lib --offline test_prep_pool_already_imported_skips_import test_prep_luks_already_open_skips_open test_prep_normalizes_partial_mounts_first` (already-imported pool, already-open LUKS, partial mounts).
- [ ] Mount-order regression test passes: `cargo test --lib --offline test_prep_mount_order_root_then_boot_then_esp` (`/` before `/boot` before ESP — faea48e).
- [ ] `test_preflight_selective_skips_recovery_wipe` passes AND no prep/bypass code path contains `zpool export` or `cryptsetup close` (`grep -n 'zpool export\|cryptsetup close' src/network/ssh_installer/zfs_ops.rs src/network/ssh_installer/installer.rs` → no hits inside the new prep helpers).
- [ ] Anti-over-suppression: `cargo test --lib --offline test_default_run_still_wipes_on_residual test_default_run_full_sequence` still pass — the flagless full default install still wipes + installs exactly as today; the guard and bypass never block the normal path.
- [ ] File headers bumped on every changed file (`grep -n 'version:' src/network/ssh_installer/installer.rs src/network/ssh_installer/disk_ops.rs src/network/ssh_installer/zfs_ops.rs` shows bumped versions; `last-edited:` shows the execution date).

## Commit message

```
feat(installer): non-destructive mount-existing-target prep + compiler-enforced wipe guard (phase-rerun/TASK-02)

Selective runs that skip Phases 2-3 now bypass the preflight residual-state
wipe (residual state is the expected input), assemble md, reopen LUKS via
keyfile, zpool import -N -R /mnt/targetos rpool then bpool, and mount / then
/boot then the ESP (order is load-bearing — faea48e). Wipe-capable DiskManager
fns now require a WipeAuthorization token, constructible only when Phase 2 is
selected. Default flagless runs are unchanged (anti-over-suppression tested).

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

Idempotency (additive polarity — check for the NEW thing's presence): `grep -n 'fn mount_existing_target' src/network/ssh_installer/installer.rs && grep -rn 'zpool import' src/network/ssh_installer/zfs_ops.rs` — if both hit, this task may already be applied; run the acceptance checks instead of re-applying. Rollback: revert the single commit — prep, bypass, and the `WipeAuthorization` threading disappear; `preflight_checks` returns to TASK-01's refuse-with-error behavior and the default path to today's wipe-on-residual; nothing persisted, no config/schema change, siblings unaffected. Note the guard only ever REMOVES the ability to wipe in selective mode — rollback cannot make anything more destructive than the status quo.
