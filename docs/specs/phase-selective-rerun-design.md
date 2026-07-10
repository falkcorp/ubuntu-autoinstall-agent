<!-- file: docs/specs/phase-selective-rerun-design.md -->
<!-- version: 1.0.0 -->
<!-- guid: a319f637-2e00-47e4-9a04-9bdb0b8df009 -->
<!-- last-edited: 2026-07-09 -->

# Phase-Selective Re-run (`--phases` / `--from-phase`) — Design Spec

**Status:** Approved — ready for implementation planning (operator decisions LOCKED below)
**Scope:** Rust CLI + Path B installer (`src/cli/`, `src/main.rs`, `src/network/ssh_installer/`). Code-only, VM-validated; never run against live servers (see HARD RULES).
**Parent task:** todo:phase-selective (workstream `phase-rerun`, 2 tasks)

---

## Motivation

Path B (`src/network/ssh_installer/`) runs a fixed 7-phase sequence (0–6) and offers no way to re-run only the phase that failed. On unimatrixone (2026-07-09), a Phase-5 GRUB failure meant the ONLY supported retry was a full re-run — and the full run's `preflight_checks` treats residual state (imported `bpool`/`rpool`, open LUKS mapper, `/mnt/targetos` mounts) as damage and **wipes the disk** via `recover_after_failure_and_wipe`. A 40-minute debootstrap and a perfectly good encrypted pool are destroyed to fix a bootloader flag.

Current behavior (grep-verified anchors from the evidence file — re-verify before editing):

| Fact | Anchor (run from repo root) |
|---|---|
| Phase sequence lives in two `run_phase!` loops (`perform_installation` ~line 177, pause variant ~line 99), phases 0–6 | `grep -n 'run_phase!("Phase' src/network/ssh_installer/installer.rs` → 12 hits (~144–147, ~160–165, ~215–225; `grep -n 'run_phase!'` → 14 incl. 2 multi-line Phase-5 calls) |
| `preflight_checks` (installer.rs ~271) wipes on residual state | `grep -n 'recover_after_failure_and_wipe' src/network/ssh_installer/installer.rs` → 1 hit ~line 344 (fn defined in disk_ops.rs ~line 49) |
| Phase 2 wipe is `wipe_disk`, called from `prepare_disk` and `recover_after_failure_and_wipe` | `grep -n 'async fn wipe_disk' src/network/ssh_installer/disk_ops.rs` → 1 hit ~line 222 |
| `wipe_disk` runs `wipefs -a` / `blkdiscard -f` / `sgdisk --zap-all` | `grep -n 'wipefs -a' src/network/ssh_installer/disk_ops.rs` → 1 hit ~line 227 |
| There is **NO `zpool import` anywhere in src/** — pools are only created fresh in Phase 3; import is NEW code | `grep -rn 'zpool import' src/` → 0 hits |
| Phase 3 creates pools with altroot `-R /mnt/targetos` | `grep -n -- '-R /mnt/targetos' src/network/ssh_installer/zfs_ops.rs` → 6 hits (builders ~181 rpool, ~200 bpool) |
| LUKS open command that mount-existing prep must replay | `grep -n 'cryptsetup open' src/network/ssh_installer/disk_ops.rs` → 1 hit ~line 348 |
| Inverse ops to reuse (unmounts, export, close) live in `final_cleanup` | `grep -n 'fn final_cleanup' src/network/ssh_installer/system_setup.rs` → 1 hit ~line 915 |
| Chroot bind mounts that later phases replay/need | `grep -n 'mount --rbind /dev' src/network/ssh_installer/system_setup.rs` → 2 hits ~244 and ~495 |
| Mount ORDER is load-bearing (`/` before `/boot`) — the historical grub-install "unknown filesystem" root cause, fixed by faea48e | `grep -n 'ORDER IS LOAD-BEARING' src/network/ssh_installer/zfs_ops.rs` → 1 hit (comment block ~lines 60–67) |

**Goal:** `uaa install --phases 5` (etc.) re-runs exactly the selected phases against the EXISTING on-disk system, non-destructively, while a flagless run stays byte-identical to today's full 0–6 run.

## Goals

- `--phases <spec>` (e.g. `"5"`, `"4-6"`, `"0,1,5"`) and `--from-phase <n>` on the install commands, mutually exclusive with each other.
- DEFAULT (no flags): byte-identical to today's full 0–6 run — same commands, same order, same preflight, same wipe-on-residual behavior.
- Non-destructive **mount-existing-target prep** when phases 2–3 are skipped but later phases need a mounted target: assemble md → open LUKS (keyfile) → `zpool import -R /mnt/targetos` rpool THEN bpool → mount `/` (rpool ROOT) BEFORE `/boot` (bpool BOOT) BEFORE ESP.
- GUARD: skipping Phase 2/3 makes a wipe **IMPOSSIBLE** — enforced in code, not by convention.
- Idempotent prep: tolerate pool already imported, LUKS already open, partial mounts, stale chroot binds.

## Non-goals (v1)

- Config-file-driven default phase set — deferred; CLI flags only for v1 (LOCKED).
- Auto-detect resume point from on-disk state — rejected; too magic for a wipe-adjacent path (LOCKED).
- Any change to Path A (`src/autoinstall/`) — out of scope; Path A stays live.
- Per-phase sub-step selection (e.g. "only grub-install inside Phase 5") — a phase is the unit.
- Running against 172.16.2.30 or len-serv-003 — validation is VM/QEMU only (HARD RULE 1).

## Decisions (locked during design)

1. **Flag surface:** `--phases <spec>` accepting `"5"`, `"4-6"`, `"0,1,5"` (comma-separated singles and inclusive ranges, values 0–6), plus `--from-phase <n>` as sugar for `n-6`. `conflicts_with` each other. Losing alternative: a config-file-driven default phase set — deferred to keep v1 auditable from the command line alone.
2. **Default inertness:** with neither flag, execution is byte-identical to today — the selective path is only entered when a flag is present. Losing alternative: always routing through the new selection loop — rejected because "provably inert" beats "probably equivalent" on a wipe-adjacent path.
3. **Resume point is explicit, never inferred.** Losing alternative: auto-detect resume point from residual state — rejected as too magic for a path one bug away from `wipefs`.
4. **Wipe guard is a type, not a review comment:** every wipe-capable function (`wipe_disk`, the destructive branch of `prepare_disk`, `recover_after_failure_and_wipe`) requires a `WipeAuthorization` token constructible ONLY from a `PhaseSelection` that includes Phase 2. Skipping Phase 2/3 makes a wipe impossible by construction.
5. **Preflight residual-state wipe is BYPASSED in selective mode:** when a `--phases`/`--from-phase` selection omits Phase 2, `preflight_checks` must NOT call `recover_after_failure_and_wipe` — residual state is the EXPECTED input, not damage. Connectivity and mirror checks still run.
6. **Mount order in prep:** `zpool import -R /mnt/targetos` rpool THEN bpool; mount `/` (rpool ROOT dataset) BEFORE `/boot` (bpool BOOT dataset) BEFORE the ESP. This ORDER is load-bearing: the reversed order was the root cause of the grub-install "unknown filesystem" failure (fixed by faea48e; see the `ORDER IS LOAD-BEARING` comment at zfs_ops.rs:60–67).
7. **LUKS open uses a keyfile,** never `echo '<key>' |` interpolation — aligns with installer-robustness/TASK-05 (luks-keyfile, wave 2, merged before this work in wave 5). Prep reuses that task's keyfile helper.
8. **Imports use `-N` (no auto-mount)** so dataset mounting is explicit and ordered by our code, not by pool `canmount` side effects.

## Data model

```rust
// src/network/ssh_installer/installer.rs (new public types; no new files —
// TASK-01's file list is args.rs / main.rs / commands.rs / installer.rs)

/// Which of phases 0..=6 run. Parsed from --phases / --from-phase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhaseSelection {
    /// selected[n] == true → Phase n runs. Index 0..=6.
    selected: [bool; 7],
    /// true only when the user passed --phases/--from-phase.
    /// false == default full run (must stay byte-identical to today).
    explicit: bool,
}

impl PhaseSelection {
    /// Today's behavior: all phases, not explicit.
    pub fn full() -> Self;
    /// Parse "5" | "4-6" | "0,1,5" (singles + inclusive ranges, 0..=6).
    /// Errors: empty spec, non-digit token, phase > 6, reversed range (e.g. "6-4").
    pub fn parse(spec: &str) -> Result<Self, String>;
    /// --from-phase n  ==  parse("{n}-6").
    pub fn from_phase(n: u8) -> Result<Self, String>;
    pub fn contains(&self, phase: u8) -> bool;
    pub fn is_explicit(&self) -> bool;
    /// Some(token) iff Phase 2 is selected (explicitly, or via the full default).
    pub fn authorize_wipe(&self) -> Option<WipeAuthorization>;
    /// Phase 2 skipped AND any of 3..=6 selected → md+LUKS layer needed.
    pub fn needs_luks_reopen(&self) -> bool;
    /// Phases 2 AND 3 both skipped AND any of 4..=6 selected → pool import + mounts needed.
    pub fn needs_pool_import(&self) -> bool;
}

/// Zero-sized wipe token. Private field ⇒ only constructible inside this
/// module via PhaseSelection::authorize_wipe(). Wipe-capable DiskManager
/// functions take `&WipeAuthorization`; no token, no wipe — the compiler
/// enforces the guard.
pub struct WipeAuthorization(pub(crate) ());
```

### Persistence

None. Selection lives for one process invocation; nothing is written to disk or config. (Config-file default phase sets are the deferred alternative.)

## Components

### C1. Phase-spec parsing + CLI plumbing (`src/cli/args.rs`, `src/main.rs`, `src/cli/commands.rs`) — TASK-01

- `Install`, `SshInstall`, and `LocalInstall` each gain:
  `#[arg(long, conflicts_with = "from_phase", help = "Run only these phases, e.g. \"5\", \"4-6\", \"0,1,5\" (default: all)")] phases: Option<String>` and
  `#[arg(long, help = "Run phases n..=6")] from_phase: Option<u8>`.
- `main.rs` threads both values through the existing match arms (~lines 63–130) into `install_command` / `ssh_install_command` / `local_install_command`.
- `commands.rs` converts to `PhaseSelection` (`parse` / `from_phase` / else `PhaseSelection::full()`), failing fast with the parse error BEFORE any connection is made.
- Fail-closed semantics: an invalid spec is a hard error (exit non-zero, nothing executed). **Default: `PhaseSelection::full()`** — the installer entry point receives exactly what it effectively receives today.

### C2. Selective run loop (`src/network/ssh_installer/installer.rs`) — TASK-01

- `perform_installation_with_options_and_pause` gains a `selection: &PhaseSelection` parameter (existing callers pass `PhaseSelection::full()`).
- When `!selection.is_explicit()`: identical code path to today — same `preflight_checks`, same seven `run_phase!` invocations, in both the standard (~215–225) and pause (~144–165) sequences.
- When explicit: each `run_phase!` call is wrapped in `if selection.contains(n)`; skipped phases log `Phase n: SKIPPED (--phases)`. Phase 0 (`setup_installation_variables`) is cheap and side-effect-light but is still selection-controlled — the spec grammar allows `0`.
- Progress percentages (10/20/35/50/75/90/95) are unchanged for phases that run; skipped phases report nothing.

### C3. Wipe guard + preflight bypass (`installer.rs`, `disk_ops.rs`) — TASK-02

- `DiskManager::wipe_disk`, the destructive branch of `prepare_disk` (the `wipe_disk` → `create_partitions` → `format_partitions` → `setup_luks_encryption` chain), and `recover_after_failure_and_wipe` gain a `_auth: &WipeAuthorization` parameter. Call sites obtain the token via `selection.authorize_wipe()`; when `None`, Phase 2 cannot have been selected and the compiler prevents any wipe call.
- `preflight_checks` gains the selection: when `selection.is_explicit() && !selection.contains(2)`, the residual-state branch (installer.rs ~344) logs `Preflight: residual state detected — expected in selective mode; NOT wiping` and returns `Ok` instead of calling `recover_after_failure_and_wipe`. Connectivity + mirror checks are unchanged in all modes.

### C4. Mount-existing-target prep (`installer.rs` orchestration; helpers in `zfs_ops.rs`, `disk_ops.rs`) — TASK-02

Runs after preflight, before the first selected phase, only when `needs_luks_reopen()` / `needs_pool_import()` demand it. Every step is idempotent (check-then-act) and non-destructive:

1. **Normalize stale state:** lazily unmount stale chroot binds and ESP with the inverse ops from `final_cleanup` (system_setup.rs ~915: `umount -R /mnt/targetos/{sys,proc,dev,run} || true`, `umount /mnt/targetos/boot/efi || true`) — but NEVER `zpool export` / `cryptsetup close`: healthy pools and mappers are reused, not torn down. Handles the "partial mounts / stale chroot binds" failure mode.
2. **Assemble md (if `config.disk_device` starts with `/dev/md`):** `mdadm --assemble --scan || true` — tolerate "already assembled" (mdadm exits non-zero when nothing new to assemble; that is success here).
3. **Open LUKS (keyfile):** skip if `cryptsetup status luks` already succeeds ("LUKS already open" failure mode); otherwise `cryptsetup open <p4> luks --key-file <0600 tempfile>` using the keyfile helper from installer-robustness/TASK-05, then shred the tempfile. Never interpolate `config.luks_key` into a command line.
4. **Import pools — rpool THEN bpool (NEW code; `grep -rn 'zpool import' src/` → 0 hits today):** for each pool, skip if `zpool list -H <pool>` already succeeds ("pool already imported" failure mode); otherwise `zpool import -N -R /mnt/targetos <pool>`. `-R` keeps every dataset under the altroot; `-N` defers mounting to step 5 so ORDER stays ours.
5. **Mount in the load-bearing order** (Decision 6): discover the root dataset (`zfs list -H -o name -r rpool/ROOT | grep '^rpool/ROOT/ubuntu_'`), then `zfs mount rpool/ROOT/ubuntu_<uuid>` (mounts `/`), THEN `zfs mount bpool/BOOT/ubuntu_<uuid>` (mounts `/boot` on top), THEN mount the ESP via the existing ESP-selection helper (`choose_esp_partition`, system_setup.rs ~57) onto `/mnt/targetos/boot/efi`. Each mount is `mountpoint -q … ||`-guarded for idempotency.
6. Chroot bind mounts are NOT part of prep — Phases 4/5 already establish their own rbind blocks (system_setup.rs ~244 and ~495) idempotently.

Failure semantics: any prep step that fails hard (e.g. LUKS open with wrong key, import finds no pool) aborts the run with a diagnostic BEFORE any selected phase executes — fail-closed, nothing half-runs.

## Migration / integration

Purely additive. Existing callers change mechanically:

```rust
// Before (src/cli/commands.rs ~363, ~492):
.perform_installation_with_options_and_pause(&config, hold_on_failure, pause_after_storage)

// After:
.perform_installation_with_options_and_pause(&config, hold_on_failure, pause_after_storage, &selection)
// where `selection` is PhaseSelection::full() unless --phases/--from-phase was given
```

```rust
// Before (disk_ops.rs, inside prepare_disk):
self.wipe_disk(config).await?;

// After:
self.wipe_disk(config, auth).await?;   // auth: &WipeAuthorization threaded from the caller
```

Exact call-site line numbers get pinned during implementation via the anchor greps above (never bare line numbers). Wave-ordering note: installer-robustness/TASK-01 (partition-suffix helper), TASK-05 (LUKS keyfile), and TASK-07 (curtin) all touch `installer.rs`/`disk_ops.rs` in waves 1–3; phase-rerun lands in waves 4–5 and rebases onto their merged results — prep step 3 consumes TASK-05's keyfile helper rather than reinventing it.

## Milestones

- **M1 — CLI + selection plumbing (phase-rerun/TASK-01, wave 4).** `--phases`/`--from-phase` parsing, `PhaseSelection`, selective `run_phase!` loop. Additive — no existing behavior changes: without flags every command executed is byte-identical to today.
- **M2 — mount-existing prep + wipe guard (phase-rerun/TASK-02, wave 5).** The ONE behavior-changing milestone, gated by the flags themselves (default **off**: no flags → full-run semantics incl. today's preflight wipe-on-residual). `WipeAuthorization` guard, preflight bypass, md/LUKS/import/mount prep. Validate in QEMU (testing-gates/TASK-01 harness) before ANY hardware attempt; never on 172.16.2.30 or len-serv-003.

Each milestone is independently shippable and additive until M2, and M2's new behavior is unreachable without an explicit flag.

## Files modified

| File | Change |
|---|---|
| `src/cli/args.rs` | `phases` / `from_phase` args on `Install`, `SshInstall`, `LocalInstall` (TASK-01) |
| `src/main.rs` | Thread new args through the three match arms (TASK-01) |
| `src/cli/commands.rs` | Build `PhaseSelection`, fail fast on parse error, pass to installer (TASK-01) |
| `src/network/ssh_installer/installer.rs` | `PhaseSelection` + `WipeAuthorization` types, selective run loop (TASK-01); preflight bypass + prep orchestration (TASK-02) |
| `src/network/ssh_installer/disk_ops.rs` | Wipe-capable fns take `&WipeAuthorization`; md-assemble + keyfile LUKS-reopen helpers (TASK-02) |
| `src/network/ssh_installer/zfs_ops.rs` | NEW `zpool import -N -R` + ordered-mount helpers (TASK-02) |

## Testing

Gate everywhere: `cargo test --lib --offline` (baseline **237 passed** — must not regress) + `cargo build --offline` + `cargo clippy --offline`.

| Test | Asserts |
|---|---|
| `test_phase_selection_parse_single/range/list` | `"5"` → only 5; `"4-6"` → 4,5,6; `"0,1,5"` → 0,1,5 |
| `test_phase_selection_parse_rejects_invalid` | `""`, `"7"`, `"6-4"`, `"a"`, `"1,,2"` all `Err` — fail-closed |
| `test_phase_selection_default_is_full_not_explicit` | `full()`: all 7 selected, `is_explicit()==false`, `authorize_wipe().is_some()` |
| `test_authorize_wipe_denied_without_phase2` | `parse("4-6")` → `authorize_wipe().is_none()` (anti-wipe guard) |
| `test_needs_prep_matrix` | `"5"` → reopen+import; `"3-6"` → reopen only; `"2-6"`/full → neither |
| `test_selective_run_skips_unselected_phases` (mock `CommandExecutor`) | `--phases 5`: no `wipefs`/`sgdisk`/`debootstrap` command ever issued; Phase-5 commands issued |
| `test_default_run_byte_identical` (mock) | flagless command stream == pre-change recorded stream for a full run |
| `test_preflight_selective_skips_recovery_wipe` (mock) | residual state + `--phases 5` → no `recover_after_failure_and_wipe` commands (anti-over-suppression: full run WITH residual state still wipes) |
| `test_prep_idempotent_steps` (mock) | pool-already-imported → no `zpool import`; LUKS-open → no `cryptsetup open`; mount order `/` → `/boot` → ESP asserted by command sequence |

VM gate before any hardware: `scripts/vm-validate.sh` (testing-gates/TASK-01) full run, then induced Phase-5 failure, then `--phases 5` re-run must reach a bootable VM without a second debootstrap.

## Rollback

- M1 alone is dormant: no flag, no new code path — revert the TASK-01 commit to remove the flags entirely.
- M2's behavior change is opt-in per invocation (flags default off); reverting the TASK-02 commit restores wipe-on-residual preflight and removes prep/guard code. No data migration, no config format change, nothing persisted — `git revert` of the two commits is a complete rollback.
- The `WipeAuthorization` guard only ever REMOVES ability to wipe in selective mode; the default path keeps today's exact wipe behavior, so rollback cannot make anything more destructive than the status quo.

## Open questions (resolved — recorded for the plan)

1. ~~Config-file-driven default phase set?~~ → Deferred (LOCKED) — CLI flags only for v1.
2. ~~Auto-detect resume point from residual state?~~ → Rejected (LOCKED) — too magic for a wipe-adjacent path; the operator states intent explicitly.
3. ~~Reuse `final_cleanup` wholesale for prep normalization?~~ → Reuse its unmount inverse ops only; prep must NOT export pools or close LUKS (it reuses them).
4. ~~Import with auto-mount?~~ → `-N` always; mounting is explicit so the `/` → `/boot` → ESP order (faea48e lesson, zfs_ops.rs:60–67) is enforced by our code.
