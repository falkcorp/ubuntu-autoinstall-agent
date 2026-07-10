<!-- file: docs/agent-tasks/phase-rerun/TASK-01-phase-spec-cli.md -->
<!-- version: 1.0.0 -->
<!-- guid: 32363b2e-23be-4e76-95cc-f561f5443ad9 -->
<!-- last-edited: 2026-07-09 -->

# TASK-01 — `--phases <spec>` / `--from-phase <n>`: CLI parsing + phase-selection plumbing (full run unchanged by default) (todo:phase-selective)

**Priority:** P1 · **Effort:** L · **Recommended subagent:** Opus-class · rust-installer subagent · **Why:** touches destructive phase sequencing; a wrong default re-wipes the disk — the flagless path must be provably inert (byte-identical command stream) · ⚠ review-critical · **Depends on:** none (global wave 4 — the coordinator dispatches only after global waves 1–3 are merged)

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/phase-rerun-phase-spec-cli" -b agent/phase-rerun-phase-spec-cli origin/main
cd "$REPO/.worktrees/phase-rerun-phase-spec-cli"
git rebase origin/main
```

(Protocol is also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

## Goal

Add `--phases <spec>` (e.g. `"5"`, `"4-6"`, `"0,1,5"`) and `--from-phase <n>` to the `install`, `ssh-install`, and `local-install` subcommands, parse them into a new `PhaseSelection` type in `src/network/ssh_installer/installer.rs`, and thread that selection through both `run_phase!` sequences so only selected phases execute. With NEITHER flag, behavior is byte-identical to today: `PhaseSelection::full()` selects all 7 phases and every existing command runs in the same order (acceptance: all 237 baseline tests unchanged). This task also lands the wipe guard at the installer boundary: define `WipeAuthorization` (constructible ONLY via `PhaseSelection::authorize_wipe()`, which returns `Some` iff Phase 2 is selected) and gate every call into the wipe-capable `DiskManager` functions (`prepare_disk` from `phase_2_disk_preparation`; `recover_after_failure_and_wipe` from `preflight_checks`) on holding that token — a selective run that omits Phase 2 hard-errors instead of wiping.

REUSE — do not invent parallel machinery:

- The existing `run_phase!` macros in `perform_installation` and `perform_installation_with_options_and_pause` — wrap their invocations, do not rewrite them.
- `crate::error::AutoInstallError::ValidationError(String)` for all parse/guard errors — no new error variants.
- The existing `#[arg(long, help = "…")]` clap-derive idiom already used throughout `src/cli/args.rs`.
- For the mock test, mirror the `RecordingMock` pattern (a `CommandExecutor` impl recording commands) — copy-from source:
  ```bash
  grep -n 'struct RecordingMock' src/autoinstall/place.rs        # copy-from source, ~line 380, 1 hit
  grep -n 'pub trait CommandExecutor' src/network/executor.rs    # the trait to implement, line ~11, 1 hit
  ```
- Do NOT create a new module/file for the types: `PhaseSelection` and `WipeAuthorization` live in `installer.rs` (locked in the design spec — TASK-01's file list is exactly `src/cli/args.rs`, `src/main.rs`, `src/cli/commands.rs`, `src/network/ssh_installer/installer.rs`). Import into `commands.rs` via `crate::network::ssh_installer::installer::PhaseSelection` — do NOT edit `mod.rs` to add a re-export (that file belongs to other tasks).

## Background (verify before editing)

- Spec (read it — decisions are LOCKED): `docs/specs/phase-selective-rerun-design.md` §Decisions, §Data model, §C1, §C2; plan: `docs/specs/phase-selective-rerun-plan.md`. Do not reopen locked decisions (no config-file phase sets, no auto-detected resume point).
- Path B runs 7 phases (0–6) via a `run_phase!` macro in TWO sequences in `installer.rs`: `perform_installation` (~line 177, continues past failures, progress 10/20/35/50/75/90/95) and `perform_installation_with_options_and_pause` (~line 99, hold-on-failure + pause-after-storage; delegates to `perform_installation` when neither option is set).
- `preflight_checks` (~line 271) detects residual bpool/rpool/LUKS/mounts and calls `DiskManager::recover_after_failure_and_wipe` — i.e. residual state currently triggers a WIPE. In BOTH sequences a preflight `Err` is only logged; the run continues. Your guard must therefore prevent the wipe *call*, not rely on aborting the run.
- The `install` subcommand delegates to `ssh_install_command` / `local_install_command`; `local-install`'s `main.rs` arm passes literal `None` for `config_path` and `report_url` — you will add your two new values into those call sites too.
- `src/cli/args.rs` has `#[cfg(test)]` tests that match `Commands::SshInstall { … }` / `Commands::LocalInstall { … }` EXHAUSTIVELY (no `..`) — adding fields makes them fail to compile until you add the new fields to those patterns.

- **Re-verify these anchors before editing** — line numbers drift, they are a starting point only:
  ```bash
  grep -n 'run_phase!("Phase' src/network/ssh_installer/installer.rs
  # expect: 12 hits (lines ~144-147, ~160-165 in the pause variant; ~215-225 in perform_installation); the 2 multi-line Phase-5 invocations are matched by grep -n 'run_phase!' which yields 14
  grep -n 'pub async fn perform_installation\|async fn perform_installation_with_options_and_pause' src/network/ssh_installer/installer.rs
  # expect: 2 hits — pause variant ~99, perform_installation ~177
  grep -n 'recover_after_failure_and_wipe' src/network/ssh_installer/installer.rs
  # expect: 1 hit ~line 344 (fn defined in disk_ops.rs ~line 49)
  grep -n 'async fn wipe_disk' src/network/ssh_installer/disk_ops.rs
  # expect: 1 hit ~line 222 (context only — do NOT edit disk_ops.rs in this task)
  grep -n 'async fn preflight_checks\|async fn phase_2_disk_preparation' src/network/ssh_installer/installer.rs
  # expect: 2 hits — preflight ~271, phase_2 ~514
  grep -n 'perform_installation_with_options_and_pause' src/cli/commands.rs
  # expect: 2 call sites, ~lines 363 and 492
  grep -n 'pub async fn ssh_install_command\|pub async fn local_install_command\|pub async fn install_command' src/cli/commands.rs
  # expect: 3 hits ~275, ~379 (wrapped in doc comment above ~386), ~516
  grep -n 'Install {\|SshInstall {\|LocalInstall {' src/cli/args.rs
  # expect: 7 hits — enum variants ~92, ~125, ~164; the rest (~520+) are exhaustive #[cfg(test)] match patterns you must extend
  grep -n 'Commands::Install\|Commands::SshInstall\|Commands::LocalInstall' src/main.rs
  # expect: 3 hits ~63, ~87, ~111 (the match arms to thread through)
  ```
  Zero hits on any anchor = STOP and report; do not guess.

- HARD RULES (restated): (1) NEVER run this against 172.16.2.30 ("the server") or len-serv-003 — this task is code + unit tests only; validation beyond `cargo test` happens later in QEMU. (2) No real `luks_key`/`root_password`/`tpm2_pin` values anywhere — tests use obviously-fake strings. (3) Stay in your worktree; NEVER push/PR/merge — the coordinator owns git.

## Step-by-step

1. Run every anchor grep above. Any zero-hit → STOP and report.
2. **`src/cli/args.rs`** — purely additive: append exactly these two fields to EACH of the `Install`, `SshInstall`, and `LocalInstall` variants (do not touch other variants or existing args):
   ```rust
   #[arg(
       long,
       conflicts_with = "from_phase",
       help = "Run only these phases, e.g. \"5\", \"4-6\", \"0,1,5\" (default: all)"
   )]
   phases: Option<String>,

   #[arg(long, help = "Run phases n..=6 (shorthand for \"n-6\")")]
   from_phase: Option<u8>,
   ```
   Then extend the exhaustive `#[cfg(test)]` match patterns (the ~520+ grep hits) with `phases, from_phase,` bindings; in the "minimal" tests additionally assert `assert!(phases.is_none()); assert!(from_phase.is_none());`. Do not weaken existing assertions.
3. **`src/network/ssh_installer/installer.rs`** — add the two public types exactly per the spec's data model (before the `SshInstaller` struct or after it; keep the file's existing section style):
   ```rust
   /// Which of phases 0..=6 run. Parsed from --phases / --from-phase.
   #[derive(Debug, Clone, PartialEq, Eq)]
   pub struct PhaseSelection {
       selected: [bool; 7],
       explicit: bool,
   }

   /// Zero-sized wipe token. Private field ⇒ only constructible inside this
   /// module via PhaseSelection::authorize_wipe(). No token, no wipe.
   pub struct WipeAuthorization(pub(crate) ());
   ```
   Implement on `PhaseSelection`: `full()` (all 7 true, `explicit: false`), `parse(spec: &str) -> Result<Self, String>`, `from_phase(n: u8) -> Result<Self, String>` (exactly `Self::parse(&format!("{n}-6"))`), `contains(&self, phase: u8) -> bool`, `is_explicit(&self) -> bool`, `authorize_wipe(&self) -> Option<WipeAuthorization>` (`Some(WipeAuthorization(()))` iff `selected[2]`), `needs_luks_reopen(&self) -> bool` (Phase 2 NOT selected AND any of 3..=6 selected), `needs_pool_import(&self) -> bool` (Phases 2 AND 3 both NOT selected AND any of 4..=6 selected). The last two are consumed by TASK-02 — define them now so the type is complete.
   `parse` grammar, spelled out (fail-closed — every malformed input is `Err`, never a best-effort selection): split on `,`; each token after `trim()` is either a single digit `0..=6` or an inclusive range `a-b` with `a <= b`, both in `0..=6`. Errors: empty spec, empty token (`"1,,2"`), non-digit token (`"a"`), phase `> 6` (`"7"`), reversed range (`"6-4"`). Result has `explicit: true`. Duplicate/overlapping tokens are fine (idempotent set-inserts).
4. **`installer.rs` — thread the selection.** Add `selection: &PhaseSelection` as the LAST parameter of BOTH `perform_installation` and `perform_installation_with_options_and_pause`; the early delegate inside the pause variant (`return self.perform_installation(config).await;`) passes `selection` through. In BOTH sequences wrap each `run_phase!` invocation:
   ```rust
   if selection.contains(0) {
       run_phase!("Phase 0: Setup variables", 10, self.setup_installation_variables(config));
   } else {
       info!("Phase 0: Setup variables — SKIPPED (--phases)");
   }
   ```
   (Pause-variant invocations have no progress number — keep each macro call's existing arguments EXACTLY as they are; only wrap.) For **Phase 2 only**, gate on the token instead of `contains(2)` so the wipe path is structurally unreachable without authorization:
   ```rust
   if let Some(_wipe_auth) = selection.authorize_wipe() {
       run_phase!("Phase 2: Disk preparation", 35, self.phase_2_disk_preparation(config));
   } else {
       info!("Phase 2: Disk preparation — SKIPPED (--phases)");
   }
   ```
   Semantics, spelled out: `full()` selects everything, so the flagless path takes every `if` branch and issues the identical command stream in the identical order — including the identical progress reports. Skipped phases emit NO `self.report(...)` call and no progress. The `pause_after_storage` block between Phases 3 and 4 stays exactly where it is, unconditional.
5. **`installer.rs` — guard `preflight_checks`.** Add `selection: &PhaseSelection` to `preflight_checks` and update its two callers (one per sequence). Replace the residual-state recovery call (the `recover_after_failure_and_wipe` anchor) with:
   ```rust
   match selection.authorize_wipe() {
       Some(_auth) => {
           let mut disk_manager = DiskManager::new(&mut *self.runner);
           let _ = disk_manager.recover_after_failure_and_wipe(config).await;
       }
       None => {
           return Err(crate::error::AutoInstallError::ValidationError(
               "Residual install state detected and Phase 2 is not selected; refusing to wipe. \
                Non-destructive mount-existing-target prep lands in phase-rerun/TASK-02."
                   .to_string(),
           ));
       }
   }
   ```
   Edge case, spelled out: the `None` arm can only be reached with an explicit selection omitting Phase 2 (the default `full()` always authorizes), so today's wipe-on-residual behavior is untouched for every flagless run. Because both sequences merely LOG a preflight `Err`, this `Err` does not abort the run — its purpose is that NO wipe command is ever issued (TASK-02 turns this refusal into supported prep). Do NOT change `DiskManager` signatures in this task — `disk_ops.rs` belongs to TASK-02 (wave-5 collision).
6. **`src/cli/commands.rs`** — signature change (this task IS the signature change; every call site is listed here). Add `phases: Option<String>, from_phase: Option<u8>` as the final two parameters of `ssh_install_command`, `local_install_command`, and `install_command`. In `ssh_install_command` and `local_install_command`, build the selection as the FIRST statements (fail fast BEFORE any connection or root check):
   ```rust
   use crate::network::ssh_installer::installer::PhaseSelection;

   let selection = match (phases.as_deref(), from_phase) {
       (Some(spec), _) => PhaseSelection::parse(spec)
           .map_err(crate::error::AutoInstallError::ValidationError)?,
       (None, Some(n)) => PhaseSelection::from_phase(n)
           .map_err(crate::error::AutoInstallError::ValidationError)?,
       (None, None) => PhaseSelection::full(),
   };
   ```
   Pass `&selection` to both `perform_installation_with_options_and_pause` call sites (~363, ~492 — re-locate via the anchor grep). `install_command` forwards `phases`/`from_phase` verbatim to the two handlers it delegates to (call sites inside its `match remote` — re-locate via `grep -n 'pub async fn install_command' src/cli/commands.rs`).
7. **`src/main.rs`** — destructure `phases` and `from_phase` in the three match arms (`Commands::Install` ~63, `Commands::SshInstall` ~87, `Commands::LocalInstall` ~111) and append them to the corresponding handler calls, after the existing final argument (`report_url` / the `None` literals in the LocalInstall arm).
8. **Tests** (in `installer.rs`'s existing `#[cfg(test)] mod tests` — anchor: `grep -n 'mod tests' src/network/ssh_installer/installer.rs`, 1 hit ~616). Add a test-only recording executor and constructor:
   - `struct RecordingExecutor` implementing `CommandExecutor` — mirror `RecordingMock` in `src/autoinstall/place.rs` (anchor grep in Goal): record every `execute`/`execute_with_output`/`check_silent` command into an `Arc<Mutex<Vec<String>>>`; `execute` returns `Ok(())`; `execute_with_output` / `execute_with_error_collection` return preset responses or empty defaults; `check_silent` returns `Ok(true)` iff the command has a non-empty preset response, else `Ok(false)`.
   - `#[cfg(test)] pub(crate) fn for_tests(runner: Box<dyn CommandExecutor>) -> Self` on `SshInstaller` setting `connected: true`, empty `variables`, `report_url: None`.
   - Pure unit tests: `test_phase_selection_parse_single_range_list` (`"5"` → only 5; `"4-6"` → 4,5,6; `"0,1,5"` → 0,1,5; all `is_explicit()`), `test_phase_selection_parse_rejects_invalid` (`""`, `"7"`, `"6-4"`, `"a"`, `"1,,2"` all `Err`), `test_phase_selection_default_is_full_not_explicit` (all 7 selected, `!is_explicit()`, `authorize_wipe().is_some()`), `test_authorize_wipe_denied_without_phase2` (`parse("4-6").unwrap().authorize_wipe().is_none()`), `test_needs_prep_matrix` (`"5"` → reopen AND import; `"3-6"` → reopen only; `"2-6"` and `full()` → neither).
   - Mock-driven tests (tokio async, use a fake config like the existing tests' configs — never real secrets): `test_selective_run_skips_unselected_phases` — run `perform_installation` with `parse("5")`; assert the recorded stream contains NO command containing `wipefs`, `sgdisk`, `debootstrap`, `zpool create`, or `cryptsetup luksFormat`, and DOES contain at least one `grub` command. `test_default_run_full_sequence` — `full()` with no residual presets: recorded stream contains `wipefs -a`, `sgdisk`, `zpool create`, `debootstrap`, and `grub-install`, in that relative order. `test_preflight_selective_no_wipe_on_residual` — preset `check_silent` responses so `zpool list -H rpool >/dev/null 2>&1` reads true (residual state), run with `parse("5")`: recorded stream contains NO `wipefs` and no `sgdisk --zap-all`. `test_default_run_still_wipes_on_residual` (ANTI-OVER-SUPPRESSION) — same residual presets with `full()`: recorded stream DOES contain `wipefs -a` (the guard must not block the normal path).
9. Bump the file header (`// version:` minor bump + `// last-edited: <today>`) on all four touched files; keep existing guids.

## How to test

```bash
cargo test --lib --offline
# Expected: >= 246 passed; 0 failed (237 baseline + the new tests; NO baseline test modified except the exhaustive-match extensions in args.rs tests, whose assertions all still pass)
cargo build --offline
# Expected: exit 0, no warnings introduced
cargo clippy --offline
# Expected: exit 0, no new lints
```

## Acceptance criteria

- [ ] `cargo test --lib --offline` exits 0 with ≥ 246 passed / 0 failed (baseline 237 intact).
- [ ] `cargo build --offline` and `cargo clippy --offline` exit 0.
- [ ] `grep -n 'pub struct PhaseSelection' src/network/ssh_installer/installer.rs` → 1 hit; `grep -n 'pub struct WipeAuthorization' src/network/ssh_installer/installer.rs` → 1 hit.
- [ ] `grep -c 'phases: Option<String>' src/cli/args.rs` → 3 and `grep -c 'from_phase: Option<u8>' src/cli/args.rs` → 3 (Install, SshInstall, LocalInstall).
- [ ] `grep -n 'conflicts_with = "from_phase"' src/cli/args.rs` → 3 hits (mutual exclusion wired).
- [ ] `grep -n 'authorize_wipe' src/network/ssh_installer/installer.rs` → ≥ 3 hits (definition + Phase-2 gate + preflight gate).
- [ ] `cargo test --lib --offline test_phase_selection` → all parse/default tests pass; `cargo test --lib --offline test_authorize_wipe_denied_without_phase2` passes.
- [ ] `cargo test --lib --offline test_selective_run_skips_unselected_phases` passes (no wipe/debootstrap commands under `--phases 5`).
- [ ] Anti-over-suppression: `cargo test --lib --offline test_default_run_still_wipes_on_residual` and `test_default_run_full_sequence` pass — the flagless full install still wipes + installs exactly as today.
- [ ] File headers bumped on every changed file (`grep -n 'version:' src/cli/args.rs src/main.rs src/cli/commands.rs src/network/ssh_installer/installer.rs` shows bumped versions; `last-edited:` shows the execution date).

## Commit message

```
feat(installer): add --phases/--from-phase selective phase runs with wipe-authorization guard (phase-rerun/TASK-01)

PhaseSelection parses "5" | "4-6" | "0,1,5"; flagless runs stay byte-identical
(PhaseSelection::full()). Phase 2 and the preflight residual-state recovery are
gated on WipeAuthorization, constructible only when Phase 2 is selected — a
selective run can never wipe. Anti-over-suppression tests prove the default
full run still wipes and installs unchanged.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

Idempotency (additive polarity — check for the NEW thing's presence): `grep -n 'pub struct PhaseSelection' src/network/ssh_installer/installer.rs && grep -c 'phases: Option<String>' src/cli/args.rs` — if the struct exists and the count is 3, this task may already be applied; run the acceptance checks instead of re-applying. Rollback: revert the single commit — the flags and types disappear entirely, the flagless install path returns to the pre-change code verbatim, no data/config/schema is touched, siblings unaffected.
