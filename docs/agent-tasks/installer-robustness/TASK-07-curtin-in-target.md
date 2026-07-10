<!-- file: docs/agent-tasks/installer-robustness/TASK-07-curtin-in-target.md -->
<!-- version: 1.0.0 -->
<!-- guid: bdc72ccd-4f18-4ef9-a06f-d04c1bf76dcd -->
<!-- last-edited: 2026-07-09 -->

# TASK-07 — curtin in-target compatibility: skip mounts+debootstrap when already inside the target chroot (todo:curtin)

**Priority:** P3 · **Effort:** M · **Recommended subagent:** Sonnet-class · rust-systems subagent · **Why:** additive detection + phase skip; moderate cross-fn logic · **Depends on:** none

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/installer-robustness-curtin-in-target" -b agent/installer-robustness-curtin-in-target origin/main
cd "$REPO/.worktrees/installer-robustness-curtin-in-target"
git rebase origin/main
```

(Protocol is also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

## Goal

Make `uaa install --config <path>` safe and useful when the binary is ALREADY running inside the installed/target chroot (the `curtin in-target -- ubuntu-autoinstall-agent install --config <path>` case from todo.md): detect that state via a deterministic **marker file** `/etc/uaa-target-marker`, and in that mode run ONLY post-install configuration (GRUB, LUKS crypttab, dracut, Tang — i.e. the existing Phase 5) — never disk prep, never debootstrap, never preflight wipes. Purely additive: with no marker present, every existing code path is byte-for-byte unchanged.

**Reuse — do NOT invent new configuration logic.** The post-install configuration ALREADY exists as `SshInstaller::phase_5_system_configuration` (installer.rs), which calls `SystemConfigurator::{configure_zfs_in_chroot, configure_grub_in_chroot, setup_luks_key_in_chroot}`. Those functions operate on `/mnt/targetos` via `chroot`, and their mount steps are already idempotent (`mountpoint -q … || mount …`, `|| true`). The in-target mode therefore just bind-mounts `/` to `/mnt/targetos` and reuses Phase 5 unchanged — do NOT copy, rewrite, or "de-chroot" any system_setup.rs command, and do NOT edit system_setup.rs at all (it is not in this task's file list and is collision-owned by other tasks).

Spec: `docs/specs/installer-robustness-design.md` (decision: marker-file detection — deterministic, chosen over rootfs-heuristics) and `docs/specs/installer-robustness-plan.md` (this is its TASK-07 step).

## Background (verify before editing)

- No curtin handling exists in any Rust source today; todo.md specifies the requirement: "when invoked as `curtin in-target -- ubuntu-autoinstall-agent install`, the binary is already inside the chroot; skip mount setup and debootstrap; only do post-install configuration (GRUB, LUKS crypttab, dracut, Tang)."
- `install_command` (cli/commands.rs) with no `remote` delegates to `local_install_command`, which is the exact entry point curtin would invoke — so detection lives in `local_install_command` and covers both spellings.
- **CRITICAL detection constraint:** `is_live_environment()` (cli/commands.rs) returns `true` when `DEBIAN_FRONTEND=noninteractive` — and curtin SETS that inside its in-target environment. So "not a live env" is NOT a usable in-target signal, and the existing live-env gate in `local_install_command` would actually PASS inside a curtin chroot and proceed toward a full wipe. Detection must key on the marker file ALONE. This is also why the marker check must run BEFORE the live-environment gate in the function.
- **CRITICAL safety constraint:** `preflight_checks` (installer.rs) calls `recover_after_failure_and_wipe` when it sees residual bpool/rpool/LUKS/mounts — running it from inside an installed target would see the running system's own pools as "residual" and try to WIPE them. The in-target path must never call `preflight_checks`, and must never call `final_cleanup`/Phase 6 (its `zpool export` + `cryptsetup close` would tear down the running root).
- Marker provenance (two writers, one contract): (a) uaa's own installs write `/etc/uaa-target-marker` into the target during Phase 4 (added by this task), so a uaa-installed host re-running `uaa install` gets non-destructive reconfiguration instead of a wipe; (b) curtin/subiquity targets were not installed by uaa, so the late-command contract is documented as: `curtin in-target -- sh -c 'touch /etc/uaa-target-marker'` before invoking the agent. Document this in the doc comment of the new function (code docs only — README/pipeline docs are owned by other tasks).
- **Wave/collision note:** this task runs in global wave 3. `installer.rs` is also touched by installer-robustness/TASK-01 and TASK-05 (waves 1–2), and `cli/commands.rs` by TASK-02/TASK-03; rebase onto origin/main first (the START HERE block does this) and re-run all anchor greps — surrounding code may have shifted.

- **Re-verify these anchors before editing** — line numbers drift, they are a starting point only:
  ```bash
  grep -rn 'curtin' --include='*.rs' src/
  # expect: 0 hits (mentions exist only in PLAN.md/PLAN-zfs-luks-multikey.md/todo.md/docs/netboot-autodeploy.md prose)
  grep -n 'run_phase!("Phase' src/network/ssh_installer/installer.rs
  # expect: 12 hits (lines ~144-147, ~160-165 in the pause variant; ~215-225 in perform_installation); the 2 multi-line Phase-5 invocations are matched by grep -n 'run_phase!' which yields 14
  grep -n 'recover_after_failure_and_wipe' src/network/ssh_installer/installer.rs
  # expect: 1 hit ~line 344 (fn defined in disk_ops.rs ~line 49) — the wipe path the in-target mode must NEVER reach
  grep -n 'mount --rbind /dev' src/network/ssh_installer/system_setup.rs
  # expect: 2 hits ~lines 244 and 495; fn headers via grep -n 'fn configure_system_in_chroot\|fn configure_grub_in_chroot' → ~238 and ~490
  grep -n 'fn phase_4_base_system\|fn phase_5_system_configuration\|fn phase_6_final_setup' src/network/ssh_installer/installer.rs   # expect: 3 hits — marker write goes in phase_4; in-target mode reuses phase_5; phase_6 is FORBIDDEN in-target
  grep -n 'fn local_install_command\|fn install_command\|fn is_live_environment' src/cli/commands.rs   # expect: 3 hits — detection + branch go in local_install_command
  grep -n 'DEBIAN_FRONTEND' src/cli/commands.rs                       # expect: 1 hit inside is_live_environment — why marker-only detection is required
  grep -n 'curtin in-target' todo.md                                  # expect: 2 hits (~lines 42, 59) — the source requirement
  grep -n 'pub mod installer' src/network/ssh_installer/mod.rs        # expect: 1 hit — installer module is pub, so cli code can reference installer::TARGET_MARKER_PATH without editing mod.rs
  grep -n 'mountpoint -q /mnt/targetos' src/network/ssh_installer/system_setup.rs   # expect: several hits — Phase-5 mounts are already idempotent, which is what makes the bind-mount reuse safe
  ```
  Zero hits on an anchor whose "expect" says ≥1 means STOP and report — do not guess. (The curtin grep EXPECTS 0 hits; ≥1 hit there means someone already started this work — see Idempotency.)

- **HARD RULES (restated):**
  1. NEVER wipe/reimage/touch 172.16.2.30 ("the server") or len-serv-003. This task is code-only, validated with `cargo test`/`cargo build` and later in VM/QEMU — never on live servers.
  2. `disk_device` is READ from the live target, never guessed — this task does not alter disk detection at all.
  3. Stay in your worktree; NEVER push/PR/merge — the coordinator owns all git.

## Step-by-step

1. Run every anchor grep above from the worktree root; re-locate all edit points by symbol name.
2. `src/network/ssh_installer/installer.rs` — add the shared marker constant near the top of the file (module scope, above `pub struct SshInstaller`):
   ```rust
   /// Marker written into the installed target's /etc during Phase 4.
   /// Presence on a RUNNING root means "we are inside an installed target"
   /// (e.g. under `curtin in-target`) — install commands must then reconfigure,
   /// never wipe. Curtin/subiquity callers create it via a late-command:
   /// `curtin in-target -- sh -c 'touch /etc/uaa-target-marker'`.
   pub const TARGET_MARKER_PATH: &str = "/etc/uaa-target-marker";
   ```
3. `installer.rs` — in `phase_4_base_system`, AFTER `sc.install_base_system(config).await?` succeeds, write the marker into the target NON-FATALLY (a marker failure must never fail an install):
   ```rust
   if let Err(e) = self
       .runner
       .execute(&format!(
           "printf 'installed-by=uaa\\n' > /mnt/targetos{p} && chmod 0644 /mnt/targetos{p}",
           p = TARGET_MARKER_PATH
       ))
       .await
   {
       tracing::warn!("Could not write target marker (non-fatal): {}", e);
   }
   ```
   (No secrets go in the marker — content is the fixed string only.)
4. `installer.rs` — add the new public method (place it next to `perform_installation`):
   ```rust
   /// In-target (curtin-compatible) mode: the binary is already running INSIDE
   /// the installed/target chroot. Runs ONLY post-install configuration
   /// (Phase 5: GRUB, LUKS crypttab, dracut, Tang) by bind-mounting / to
   /// /mnt/targetos so the existing chroot-based Phase-5 code is reused
   /// unchanged. NEVER runs preflight_checks (it wipes residual state),
   /// Phases 1-4 (packages/disk prep/ZFS/debootstrap), or Phase 6
   /// (final_cleanup would zpool-export / cryptsetup-close the RUNNING root).
   pub async fn perform_in_target_configuration(&mut self, config: &InstallationConfig) -> Result<()>
   ```
   Body, in order:
   1. `self.require_connected()?;`
   2. Guard (defense in depth — refuse to run this mode on a machine without the marker): `self.runner.execute(&format!("test -f {}", TARGET_MARKER_PATH)).await` — on `Err`, return `Err(AutoInstallError::ValidationError("no /etc/uaa-target-marker — refusing in-target configuration outside an installed target".into()))`.
   3. Bind prep (idempotent): `self.runner.execute("mkdir -p /mnt/targetos && { mountpoint -q /mnt/targetos || mount --bind / /mnt/targetos; }").await?;`
   4. `self.phase_5_system_configuration(config).await?;` — reuse, unchanged. (Its internal rbind/ESP/efivars mounts are all `mountpoint -q || …` guarded, so pre-mounted curtin environments are tolerated.)
   5. Best-effort teardown of ONLY the bind (never `zpool export`, never `cryptsetup close`): `let _ = self.runner.execute("umount -R /mnt/targetos 2>/dev/null || umount -l /mnt/targetos 2>/dev/null || true").await;`
   6. `info!(...)` completion log; `Ok(())`.
5. `src/cli/commands.rs` — add the detection helpers (near `is_live_environment`), split for testability:
   ```rust
   /// Pure core: marker presence under an arbitrary root (unit-testable).
   fn target_marker_present(root: &std::path::Path) -> bool {
       root.join(
           crate::network::ssh_installer::installer::TARGET_MARKER_PATH
               .trim_start_matches('/'),
       )
       .exists()
   }

   /// True when running inside an installed target (e.g. under `curtin
   /// in-target`). Marker-file ONLY — is_live_environment() is unusable here
   /// because curtin sets DEBIAN_FRONTEND=noninteractive.
   fn is_inside_installed_target() -> bool {
       target_marker_present(std::path::Path::new("/"))
   }
   ```
6. `cli/commands.rs` — branch in `local_install_command`. Insert IMMEDIATELY AFTER the root check (`SystemUtils::is_root()`), BEFORE the live-environment gate (order is load-bearing — see Background):
   ```rust
   let in_target = is_inside_installed_target();
   if in_target {
       info!("/etc/uaa-target-marker found — in-target mode: post-install configuration only (no disk prep, no debootstrap, no wipe)");
   }
   ```
   Then make the two existing live-env checks conditional on `!in_target` (change `if !force && !is_live_environment()` to `if !in_target && !force && !is_live_environment()`, and the matching `if force && !is_live_environment()` warn-block to `if !in_target && force && ...`). Do not otherwise alter them.
   Finally, at the execution point: where the function currently prints the destructive-confirmation block and calls `perform_installation_with_options_and_pause`, add the in-target branch FIRST:
   ```rust
   if in_target {
       if dry_run {
           info!("DRY RUN: would run in-target post-install configuration (Phase 5) for {}", config.hostname);
           return Ok(());
       }
       if hold_on_failure || pause_after_storage {
           warn!("--hold-on-failure/--pause-after-storage are ignored in in-target mode (no storage phases run)");
       }
       installer.perform_in_target_configuration(&config).await?;
       info!("In-target configuration completed for {}", config.hostname);
       return Ok(());
   }
   ```
   Skip the "THIS WILL BE COMPLETELY WIPED" confirmation block entirely in this branch (nothing destructive runs). Everything below the branch stays exactly as-is for the marker-absent case.
7. **Edge-case semantics (encode in code AND tests):**
   - Marker ABSENT → behavior is EXACTLY today's: live-env gate, confirmation, full 7-phase install. No default changes.
   - Marker present + `--dry-run` → print the in-target plan, exit 0, run nothing.
   - Marker present + `--force` → `--force` is about the live-env bypass, which in-target mode already skips; no special handling, no wipe.
   - Marker present + `investigate_only` → investigation already returns before the branch; unchanged.
   - `config_path` is still required to carry non-placeholder secrets — `validate_config_secrets` still runs (Phase 5 needs `luks_key` for Tang enrollment); do not bypass it.
8. Add unit tests in cli/commands.rs's existing `#[cfg(test)]` module (std-only, no new dev-deps):
   - `test_target_marker_present_detects_marker` — create a unique temp dir under `std::env::temp_dir()`, `std::fs::create_dir_all(root.join("etc"))`, write `etc/uaa-target-marker`, assert `target_marker_present(&root)` is true; clean up.
   - `test_target_marker_absent_means_full_install_path` — same temp root WITHOUT the marker file: assert `target_marker_present(&root)` is false (this is the anti-over-suppression case: no marker → the full-install path is selected).
9. Purely additive everywhere: no signature changes to existing fns, no edits to system_setup.rs/disk_ops.rs/zfs_ops.rs/mod.rs, no reordering of phases, no changes to `perform_installation` / `perform_installation_with_options_and_pause` bodies.
10. Bump the file header (`// version:` minor bump, `// last-edited: 2026-07-09`) on installer.rs and cli/commands.rs; keep guids.

## How to test

```bash
cargo test --lib --offline
# Expected: 239+ passed (baseline 237 + the 2 marker tests); 0 failed
cargo build --offline
# Expected: exit 0
cargo clippy --offline
# Expected: no new warnings
```

## Acceptance criteria

- [ ] `grep -rn 'uaa-target-marker' src/ --include='*.rs'` returns ≥3 hits (const in installer.rs, Phase-4 write, cli detection).
- [ ] `grep -n 'fn perform_in_target_configuration' src/network/ssh_installer/installer.rs` returns 1 hit.
- [ ] The in-target method contains NO wipe/teardown calls: `grep -n 'preflight_checks\|final_cleanup\|zpool export\|cryptsetup close' src/network/ssh_installer/installer.rs` shows none of these inside `perform_in_target_configuration`'s body (inspect the function; the only mount ops are the `/` bind and its umount).
- [ ] `grep -n 'fn target_marker_present\|fn is_inside_installed_target' src/cli/commands.rs` returns 2 hits.
- [ ] The marker check precedes the live-env gate: in `local_install_command`, `is_inside_installed_target()` appears BEFORE the first `is_live_environment()` use (verify by reading the function; `grep -n 'is_inside_installed_target\|is_live_environment' src/cli/commands.rs` line order).
- [ ] Edge cases proven: `test_target_marker_present_detects_marker` passes (marker → in-target) and `test_target_marker_absent_means_full_install_path` passes.
- [ ] Anti-over-suppression: with the marker ABSENT the default full-install path is untouched — `test_target_marker_absent_means_full_install_path` is green AND `git diff origin/main -- src/network/ssh_installer/installer.rs` shows no modification inside `perform_installation` / `perform_installation_with_options_and_pause` bodies (only additions elsewhere).
- [ ] Tests green: `cargo test --lib --offline` reports 239+ passed / 0 failed; `cargo build --offline` and `cargo clippy --offline` clean.
- [ ] File headers bumped on both changed files (`git diff origin/main -- src/network/ssh_installer/installer.rs src/cli/commands.rs | grep -c '^+// version:'` → 2 — both version lines touched IN THIS DIFF; a bare date grep is vacuous since both files already carry today's date at HEAD).

## Commit message

```
feat(installer): curtin in-target mode — marker-file detection, Phase-5-only reconfiguration

`uaa install` run inside an installed target (marker /etc/uaa-target-marker,
written by Phase 4 or by a curtin late-command) now bind-mounts / to
/mnt/targetos and reuses Phase 5 (GRUB, crypttab, dracut, Tang) unchanged —
skipping preflight wipes, disk prep, ZFS creation, debootstrap, and final
cleanup. Marker absent = existing behavior, byte-for-byte.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

Already-done check (additive polarity — presence of the new marker plumbing):
```bash
grep -rn 'uaa-target-marker' src/ --include='*.rs'                     # ≥3 hits → marker const/write/detection present
grep -n 'fn perform_in_target_configuration' src/network/ssh_installer/installer.rs   # 1 hit → in-target mode present
```
If both hit, the task is already applied — run the acceptance checks instead of re-applying. Rollback: `git revert` the single commit removes the marker write, the in-target method, and the CLI branch; the default install path was never modified, so reverting restores today's behavior exactly and affects no sibling task.
