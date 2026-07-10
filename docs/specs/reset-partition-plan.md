<!-- file: docs/specs/reset-partition-plan.md -->
<!-- version: 1.0.0 -->
<!-- guid: 18f91449-2a3a-4b40-9248-c40319bee1d1 -->
<!-- last-edited: 2026-07-09 -->

# RESET Partition Population — Implementation Plan (workstream boot-prod)

Companion to `docs/specs/reset-partition-design.md` (decisions there are **LOCKED** — do not
reopen). This plan maps 1:1 onto the boot-prod task briefs:

| Plan step | Brief | Global wave |
|---|---|---|
| Step 1 | `docs/agent-tasks/boot-prod/TASK-01-efibootmgr-chroot.md` | 3 |
| Steps 2–5 | `docs/agent-tasks/boot-prod/TASK-02-reset-partition-populate.md` | 6 |

**Wave order (from the skeleton — this workstream must wait on cross-WS waves):**
TASK-01 runs in global wave 3 (after installer-robustness/TASK-01's `partition_path` helper and
the wave-2 tasks merge). TASK-02 runs in global wave **6 — the final wave** — because
`installer.rs` is the most-collided file in the operation (shared with installer-robustness/
TASK-01/05/07 and phase-rerun/TASK-01/02) and `mod.rs` is shared with installer-robustness/
TASK-01; running last means one clean rebase instead of five conflict windows.

**Gate for every step:** `cargo test --lib --offline` (baseline 237 passed) &&
`cargo build --offline` && `cargo clippy --offline`. Hard rules apply throughout: code/docs
only, validated in VM/QEMU — NEVER on 172.16.2.30 ("the server") or len-serv-003;
`disk_device` is read from the live target, never guessed; workers stay in their worktree and
never push/PR/merge.

---

## Step 1 — TASK-01: efibootmgr boot-order in chroot (wave 3)

Brief: `docs/agent-tasks/boot-prod/TASK-01-efibootmgr-chroot.md` (fully specified there; one
step here for workstream completeness). Insert an efibootmgr BootOrder step (network #1,
ubuntu #2) immediately after the "Updating GRUB config" `update-grub` call inside
`configure_grub_in_chroot` (`src/network/ssh_installer/system_setup.rs` ~562) — efivarfs is
already mounted at that point ("Ensure efivarfs", ~523). Mirror the proven `set_boot_order()`
regexes from `installer-image/nocloud/uaa-usb-bootstrap.sh` (definition line 84, call line
142) verbatim; non-fatal on legacy BIOS / missing-ubuntu-entry (grub-install fallbacks 2/3 use
`--no-nvram`/`--removable`).

- Run: `grep -n "Updating GRUB config" src/network/ssh_installer/system_setup.rs`
  Expected: `1 hit ~line 562` (insertion anchor still present)
- Run: `cargo test --lib --offline && cargo build --offline && cargo clippy --offline`
  Expected: `237+ passed; 0 failed; build + clippy clean`

Note: TASK-01 does NOT touch the RESET flow; the `40_uaa_reset` drop-in (Step 4) is picked up
by the same `update-grub` regardless of whether TASK-01 has merged, because the drop-in write
happens earlier in Phase 5 (design Decision 4).

## Step 2 — TASK-02: new module `reset_partition.rs` — consts, builders, tests (wave 6)

Brief: `docs/agent-tasks/boot-prod/TASK-02-reset-partition-populate.md`. Design §C1–C4, §Data
model. Create `src/network/ssh_installer/reset_partition.rs` (4-line Rust header, version
1.0.0, fresh guid) containing:

1. `ResetPartitionStager<'a> { runner: &'a mut dyn CommandExecutor }` + `new()` — same shape
   as `DiskManager`/`SystemConfigurator`.
2. Consts: `RESET_MOUNT`, `RESET_ISO_NAME`, `RESET_HELPER_SH` (the `nuke it`-gated script —
   exact-literal match, tty check, abort on anything else), `GRUB_DROPIN` (the
   `40_custom`-style loopback entry: `search --label RESET` → `loopback loop` →
   `configfile /boot/grub/loopback.cfg`).
3. Pure command builders (`build_mount_reset_cmd`, `build_iso_size_cmd`, `build_iso_copy_cmd`,
   `build_tarball_copy_cmd`, `build_grub_dropin_cmd`, `build_grub_dropin_remove_cmd`) —
   `build_mount_reset_cmd` MUST route the p2 path through the `partition_path` helper merged
   by installer-robustness/TASK-01 in wave 1 (soft dep: `{}p2` works on the current nvme/md
   fleet, but the helper is the required form).
4. `#[cfg(test)] mod tests` with the six tests from the design's Testing table, including the
   anti-over-suppression gate test (`nuke it` matches; `NUKE IT`, `nuke it!`, empty do not).

Nothing calls the module yet — this step is provably inert (design M1).

- Run: `cargo test --lib --offline`
  Expected: `243+ passed; 0 failed` (baseline 237 + ≥6 new)
- Run: `grep -n "nuke it" src/network/ssh_installer/reset_partition.rs`
  Expected: `>=2 hits (helper const + gate test)`

## Step 3 — TASK-02: staging logic — mount, ISO copy with size gate, tarball, idempotency (wave 6)

Same brief. Design §C1–C3. Implement `stage()` and internals on the executor:

1. `mount_reset`: `mkdir -p /mnt/reset; mountpoint -q /mnt/reset || mount <p2> /mnt/reset`
   (idempotent guard as in `system_setup.rs` ~108). Mount failure → warn + return (no staging).
2. `copy_recovery_iso`: `findmnt -n -o SOURCE /cdrom` → `isosize <dev>` → df free-space gate
   (256 MiB reserve; **ISO too large for 4 GiB ⇒ warn + skip ISO AND menu entry — LOCKED**)
   → stamp-file idempotency check (`uaa-recovery.iso.stamp`: size match ⇒ skip re-copy;
   mismatch/missing ⇒ re-copy) → `dd ... count_bytes conv=fsync` → write stamp AFTER the
   successful copy. Returns `bool` iso_present. No `/cdrom` (netboot/curtin) → warn + `false`.
3. `copy_debootstrap_tarball`: reuse the Phase 4 uaacache mount + filename convention
   VERBATIM (`system_setup.rs` ~107–116; `{release}-$(dpkg --print-architecture)-base.tar.gz`,
   `release` default `resolute`); copy into `/mnt/reset/cache/` only if present and fits;
   absent → info log only.
4. `write_reset_helper`: `RESET_HELPER_SH` → `/mnt/reset/uaa-reset.sh` (0755) +
   `README.uaa-reset`. The installer stays NON-DESTRUCTIVE — the wipe gate exists only inside
   the staged script, which runs in the recovery environment.
5. `unmount_reset` on all exits.

Every content failure degrades to warn-and-continue (design Decision 6); `stage()` returns
`Err` only on executor/transport failure.

- Run: `cargo test --lib --offline && cargo clippy --offline`
  Expected: `243+ passed; 0 failed; clippy clean`
- Run: `grep -n "uaacache" src/network/ssh_installer/reset_partition.rs`
  Expected: `>=1 hit (Phase 4 convention reused, not reinvented)`

## Step 4 — TASK-02: GRUB drop-in write/remove + wiring into Phase 5 (wave 6)

Same brief. Design §C4–C5.

1. `write_grub_dropin(iso_present)`: `true` ⇒ write `GRUB_DROPIN` to
   `/mnt/targetos/etc/grub.d/40_uaa_reset` + `chmod 0755`; `false` ⇒
   `rm -f /mnt/targetos/etc/grub.d/40_uaa_reset` (no ISO ⇒ no dangling entry — Decision 7).
2. `src/network/ssh_installer/mod.rs`: add `pub mod reset_partition;` (header version bump).
3. `src/network/ssh_installer/installer.rs` `phase_5_system_configuration` (~538): insert a
   scoped, NON-FATAL stager call BEFORE `let mut sc = SystemConfigurator::new(...)` /
   `sc.configure_grub_in_chroot(config)` (~542), exactly as in the design's Before/After pair
   (`if let Err(e) = rp.stage(config).await { warn!(...) }` inside a `{ }` block so the
   `&mut *self.runner` reborrow ends before `sc`). This guarantees the drop-in exists before
   `configure_grub_in_chroot`'s final "Updating GRUB config" `update-grub` step — with ZERO
   edits to `system_setup.rs` (three other tasks collide there).
4. Bump headers on both wired files.

- Run: `grep -n "sc.configure_grub_in_chroot" src/network/ssh_installer/installer.rs`
  Expected: `1 hit; the ResetPartitionStager block appears ABOVE it in the same fn`
- Run: `grep -rn "grub.d" src/network/ssh_installer/`
  Expected: `hits only in reset_partition.rs (was 0 before this workstream)`
- Run: `cargo test --lib --offline && cargo build --offline && cargo clippy --offline`
  Expected: `243+ passed; 0 failed; build + clippy clean`

## Step 5 — TASK-02: acceptance sweep + idempotency proof (wave 6)

Same brief (its Acceptance criteria section is authoritative; this step is the plan-level
summary).

1. Already-done / idempotency grep (additive polarity → presence of the new thing):
   - Run: `grep -n "pub mod reset_partition" src/network/ssh_installer/mod.rs`
     Expected: `1 hit`
   - Run: `grep -n "ResetPartitionStager" src/network/ssh_installer/installer.rs`
     Expected: `>=1 hit inside phase_5_system_configuration`
2. Non-destructive proof: `grep -n "nuke it" src/network/ssh_installer/` shows the literal
   only inside `RESET_HELPER_SH`/tests — no wipe command (`sgdisk -o`, `wipefs`, `mkfs`,
   `luksFormat`) is added to any installer path by this workstream.
   Expected: `no new destructive commands outside the staged helper script`
3. Runtime validation happens ONLY in the QEMU harness (`testing-gates/TASK-01`,
   `scripts/vm-validate.sh`): after a VM install, assert p2 contains `uaa-recovery.iso` +
   stamp + `uaa-reset.sh`, `grub.cfg` contains `UAA reset/recover`, and a second Phase 5 run
   logs "already staged" without re-copying. Never on 172.16.2.30 or len-serv-003.
4. Rollback recorded: `git revert` of the TASK-02 commit restores today's behavior (RESET
   formatted-but-empty, no drop-in); on-host rollback = delete `/etc/grub.d/40_uaa_reset` +
   `update-grub`.

- Run: `cargo test --lib --offline && cargo build --offline && cargo clippy --offline`
  Expected: `243+ passed; 0 failed; build + clippy clean` (final gate before hand-off to the
  coordinator, who owns push/PR/merge)
