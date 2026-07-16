<!-- file: docs/agent-tasks/applications/TASK-02-application-installer-phase5.md -->
<!-- version: 1.0.0 -->
<!-- guid: 4c935472-faf8-4b87-9f43-e22bcfa53037 -->
<!-- last-edited: 2026-07-16 -->

# TASK-02 — `ApplicationInstaller` module + Phase-5 wiring (scaffold, no application logic) (DS-APP-02)

**Priority:** P1 · **Effort:** M · **Recommended subagent:** Sonnet-class · rust-core subagent · **Why:** inserts a new fallible step into the proven 7/7 install flow at a point reached from two different call sites; ordering and error propagation are load-bearing. · **Depends on:** TASK-01 (needs `InstallationConfig::applications`)

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/applications-application-installer-phase5" -b agent/applications-application-installer-phase5 origin/main
cd "$REPO/.worktrees/applications-application-installer-phase5"
git rebase origin/main
```

(Protocol also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

**Wave gate:** TASK-01 (DS-APP-01) must be merged first — it defines `InstallationConfig::applications`, which this task iterates. If `grep -n "pub applications" crates/uaa-core/src/network/ssh_installer/config.rs` returns 0 hits, the gate is not met: STOP and report.

## Goal

Create `crates/uaa-core/src/network/ssh_installer/applications.rs` containing an `ApplicationInstaller` that iterates `config.applications` and dispatches per variant, and call it from `phase_5_system_configuration`. **This task ships the scaffold and the dispatch loop only** — the Cockroach install body is TASK-03. With `applications: []` (every committed config today) the loop body never runs and behavior is byte-identical to today.

REUSE — do not invent parallels:

- **Mirror `ResetPartitionStager`'s SHAPE** (`crates/uaa-core/src/network/ssh_installer/reset_partition.rs` — verify: `grep -n "pub struct ResetPartitionStager" crates/uaa-core/src/network/ssh_installer/reset_partition.rs`): a self-contained module holding `runner: &'a mut dyn CommandExecutor`, constructed `pub fn new(runner: &'a mut dyn CommandExecutor) -> Self`, with one primary `pub async fn` taking `&InstallationConfig`. Copy that structure.
- **⚠ Do NOT mirror its error handling.** `ResetPartitionStager` is deliberately non-fatal — `phase_5_system_configuration` wraps it in `if let Err(e) = rp.stage(config).await { tracing::warn!(...) }` because it stages a recovery nicety. **An application failing to install is a failed deployment and MUST propagate with `?`.** Copying the warn-and-continue wrapper is the single most likely defect in this task.
- **The chroot command shape** — the file-wide literal pattern is `chroot /mnt/targetos bash -lc '<cmd>'`. Verify: `grep -c "chroot /mnt/targetos bash -lc" crates/uaa-core/src/network/ssh_installer/system_setup.rs` (expect ~21). Copy this string shape; do NOT invent a different chroot invocation.
- **`crate::error::AutoInstallError`** for error paths. Do NOT add a new error enum.
- Do NOT add any `Cargo.toml` dependency.

## Background (verify before editing)

- `phase_5_system_configuration` currently calls, in order: `ResetPartitionStager::stage` (non-fatal, wrapped), then `SystemConfigurator::configure_zfs_in_chroot`, `configure_grub_in_chroot`, `setup_luks_key_in_chroot`, `install_ca_cert_in_chroot` — each `?`-propagating.
- **Insert the application step LAST**, after `install_ca_cert_in_chroot`. Rationale: applications may need the install CA trust anchor that step writes (the Cockroach step in TASK-03 fetches its node cert over HTTP from the control server), so it must run after it. Do not insert it earlier.
- **Phase 5 runs from TWO call sites**, and your step must be correct under both:
  1. `perform_installation` — the full fresh-install flow.
  2. `perform_in_target_configuration` — the curtin in-target flow, which bind-mounts the **already-installed, running root** at `/mnt/targetos`. Your step must not assume Phase 4's transient setup state.
- `PhaseSelection` is hardcoded to `0..=6` in three places. **Do NOT add a new numbered phase** (spec Decision 12) — fold into Phase 5. If you find yourself editing `parse_phase`'s bound or the `[bool;7]` array, you have the wrong design.
- `ApplicationInstaller` needs NO `WipeAuthorization`. It runs post-Phase-4 and has no legitimate reason to touch a disk. If you reach for the wipe token, stop.
- Edge semantics (spelled out here AND in acceptance):
  - **`applications: []`** → the loop body never executes; `install` returns `Ok(())` having run **zero commands**. This is every committed config today and must be byte-identical to current behavior.
  - **An application's install fails** → return `Err`, propagating out of Phase 5 and failing the install. **Never** warn-and-continue.
  - **Duplicate variants in the list** (e.g. two `cockroach` entries) → hard `ConfigError` naming the duplicate kind, before running anything. Two nodes of the same app on one host is always a config mistake, and installing the second over the first would silently corrupt the first.
  - **Unknown variant** → unreachable; the enum is closed and TASK-01 made parsing reject unknown kinds.

**HARD RULES (non-negotiable):**
- NO hardware actions. All commands go through the `CommandExecutor` seam; tests inject mocks. Never shell out directly.
- NEVER wipe, write to, or deploy on 172.16.2.30 ("the server") or len-serv-003.
- NEVER power on unimatrixone (U1).
- No real secret in any file: `REPLACE_AT_PLACE_TIME` placeholders stay placeholders.
- Stay inside your worktree; never `git push`, `gh pr`, or merge — report done and stop.

- **Re-verify these anchors before editing** — line numbers drift; zero hits = STOP and report:
  ```bash
  grep -n "async fn phase_5_system_configuration" crates/uaa-core/src/network/ssh_installer/installer.rs
  # expect: 1 hit (~line 905) — the edit target
  grep -n "install_ca_cert_in_chroot" crates/uaa-core/src/network/ssh_installer/installer.rs
  # expect: 1 hit (~line 919) — insert your call immediately AFTER this line
  grep -n "RESET partition staging skipped" crates/uaa-core/src/network/ssh_installer/installer.rs
  # expect: 1 hit (~line 912) — the non-fatal wrapper you must NOT copy
  grep -n "pub struct ResetPartitionStager" crates/uaa-core/src/network/ssh_installer/reset_partition.rs
  # expect: 1 hit — the module SHAPE to mirror (structure only, not error handling)
  grep -n "pub async fn perform_in_target_configuration" crates/uaa-core/src/network/ssh_installer/installer.rs
  # expect: 1 hit (~line 479) — the SECOND call site your step must be safe under
  grep -n "pub applications" crates/uaa-core/src/network/ssh_installer/config.rs
  # expect: 1 hit — TASK-01's field (0 hits = wave gate not met, STOP)
  grep -n "pub mod reset_partition" crates/uaa-core/src/network/ssh_installer/mod.rs
  # expect: 1 hit — mirror this line to declare your new module
  ```

## Step-by-step

1. Create `crates/uaa-core/src/network/ssh_installer/applications.rs` with a fresh 4-line header (new uuid4 via `uuidgen | tr '[:upper:]' '[:lower:]'`, version `1.0.0`, `last-edited: 2026-07-16`, `file:` matching the real path).
2. Implement, mirroring `ResetPartitionStager`'s structure:
   ```rust
   pub struct ApplicationInstaller<'a> {
       runner: &'a mut dyn CommandExecutor,
   }

   impl<'a> ApplicationInstaller<'a> {
       pub fn new(runner: &'a mut dyn CommandExecutor) -> Self { Self { runner } }

       /// Install every application in `config.applications` into the target.
       /// Empty list = no-op returning Ok(()) with zero commands executed.
       /// FAIL-CLOSED: any application's failure propagates and fails the install.
       pub async fn install(&mut self, config: &InstallationConfig) -> Result<()> {
           if config.applications.is_empty() {
               return Ok(());
           }
           Self::reject_duplicates(&config.applications)?;
           for app in &config.applications {
               match app {
                   ApplicationSpec::Cockroach(spec) => {
                       self.install_cockroach(config, spec).await?;
                   }
               }
           }
           Ok(())
       }

       /// TASK-03 fills this. Scaffold only: return a NotImplemented-style
       /// ConfigError naming cockroach so a config that requests it fails
       /// loudly rather than silently installing nothing.
       async fn install_cockroach(
           &mut self,
           _config: &InstallationConfig,
           _spec: &CockroachSpec,
       ) -> Result<()> {
           Err(AutoInstallError::ConfigError(
               "cockroach application install not yet implemented (DS-APP-03)".to_string(),
           ))
       }

       fn reject_duplicates(apps: &[ApplicationSpec]) -> Result<()> { /* ... */ }
   }
   ```
   The stub returning `Err` is deliberate: a half-wired scaffold that returned `Ok(())` would let a Cockroach-requesting config report a successful install having installed nothing. Fail loudly instead.
3. Declare the module in `crates/uaa-core/src/network/ssh_installer/mod.rs`, mirroring the existing `pub mod reset_partition;` line. Re-export `ApplicationInstaller` if and only if the sibling modules are re-exported there — match what you find, do not add a new export convention.
4. In `installer.rs`, inside `phase_5_system_configuration`, **immediately after** the `install_ca_cert_in_chroot` call, add:
   ```rust
   let mut ai = ApplicationInstaller::new(&mut *self.runner);
   ai.install(config).await?;
   ```
   Note the `?` — fail-closed. Do **not** wrap it in the `if let Err(e) = … { warn! }` pattern used for `ResetPartitionStager` two lines above; that pattern is correct for the reset stager and wrong here.
5. Keep the change purely additive — do not modify the existing Phase-5 calls, do not reorder them, do not change any signature, do not touch `PhaseSelection`.
6. Add tests in `applications.rs`'s `mod tests` using the crate's existing mock-executor pattern (mirror `installer.rs`'s test module — verify: `grep -n "fn sample_config" crates/uaa-core/src/network/ssh_installer/installer.rs`):
   - `test_empty_applications_runs_no_commands` — a config with `applications: vec![]` yields `Ok(())` and the mock recorded **zero** executed commands (assert on the recorded-command count, not just `is_ok()`).
   - `test_duplicate_application_kind_rejected` — two `Cockroach` entries → `Err` naming `cockroach`, and **zero** commands ran (rejection happens before any execution).
   - `test_cockroach_scaffold_returns_not_implemented` — a single Cockroach entry → `Err` mentioning `DS-APP-03` (this test is replaced by TASK-03).
   - `test_application_failure_propagates` — the anti-over-suppression case in reverse: prove the error does **not** get swallowed. Assert `install(...)` returns `Err`, not `Ok`.
7. Bump file headers (`version` + `last-edited`) on every file you touch; keep existing guids.

**Anti-over-suppression:** `test_empty_applications_runs_no_commands` is the happy-path guard — it proves the new step, which is a conditional path added into a proven 7/7 flow, does not disturb the existing zero-application behavior (zero commands executed, `Ok`). Paired with `test_application_failure_propagates`, which proves the guard does not over-suppress in the other direction by swallowing a real failure.

## How to test

```bash
cargo test --lib --offline
# Expected: 639+ passed, 0 failed (634 baseline + 5 from TASK-01 + your 4).
cargo build --offline
# Expected: exit 0.
cargo clippy --offline -- -D warnings
# Expected: no warnings.
```

## Acceptance criteria

- [ ] `cargo test --lib --offline` exits 0 — verify: `cargo test --lib --offline 2>&1 | grep -E "^test result"`
- [ ] `cargo build --offline` exits 0 — verify: `cargo build --offline && echo BUILD_OK`
- [ ] The call is `?`-propagating, NOT warn-wrapped — verify: `grep -A2 "ApplicationInstaller::new" crates/uaa-core/src/network/ssh_installer/installer.rs | grep -c "warn!"` returns **0**
- [ ] The call sits after the CA step — verify: `grep -n "install_ca_cert_in_chroot\|ai.install(config)" crates/uaa-core/src/network/ssh_installer/installer.rs` shows `install_ca_cert_in_chroot` on a **lower** line number than `ai.install`
- [ ] No new numbered phase — verify: `git diff origin/main -- crates/uaa-core/src/network/ssh_installer/installer.rs | grep -c "parse_phase\|\[bool; 7\]"` returns **0**
- [ ] Empty list runs zero commands — verify: `cargo test --lib --offline test_empty_applications_runs_no_commands`
- [ ] Anti-over-suppression: `test_empty_applications_runs_no_commands` + `test_application_failure_propagates` both pass — verify: `cargo test --lib --offline test_application_failure_propagates`
- [ ] `cargo clippy --offline -- -D warnings` clean
- [ ] File headers bumped on every changed file — verify: `git diff origin/main --name-only | xargs -I{} grep -l "last-edited: 2026-07" {}`

## Commit message

```
feat(installer): add ApplicationInstaller and wire it into Phase 5 (DS-APP-02)

Adds crates/uaa-core/src/network/ssh_installer/applications.rs — an
ApplicationInstaller mirroring ResetPartitionStager's module shape — and
calls it from phase_5_system_configuration immediately after
install_ca_cert_in_chroot, so applications can rely on the install CA trust
anchor.

Unlike ResetPartitionStager, this step is FAIL-CLOSED: an application that
fails to install is a failed deployment and propagates with `?` rather than
warn-and-continue. The Cockroach body is a deliberate Err stub (DS-APP-03);
returning Ok would let a Cockroach-requesting config report success having
installed nothing.

Behavior-neutral today: every committed config has applications: [], for
which install() runs zero commands and returns Ok.

Co-Authored-By: Claude <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

**Polarity: additive.** If `grep -n "pub struct ApplicationInstaller" crates/uaa-core/src/network/ssh_installer/applications.rs` hits AND `grep -n "ai.install(config)" crates/uaa-core/src/network/ssh_installer/installer.rs` hits, this task is already applied — run the acceptance checks instead of re-applying. Rollback = revert the single commit; Phase 5 returns to its four existing chroot steps, no data or schema is touched, and `applications: []` means no committed config's behavior changes either way. Siblings that also edit `installer.rs` (TASK-03) must rebase after this merges — see the collision table in `../BREAKDOWN-2026-07-16.md`.
