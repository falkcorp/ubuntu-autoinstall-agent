<!-- file: docs/agent-tasks/boot-prod/TASK-02-reset-partition-populate.md -->
<!-- version: 1.0.0 -->
<!-- guid: 61723f55-5445-4ee9-a89d-a063e11ce250 -->
<!-- last-edited: 2026-07-09 -->

# TASK-02 — Populate RESET p2: remastered USB ISO copy + debootstrap tarball + GRUB loopback recover entry gated on typing nuke-it (todo:RESET-partition)

**Priority:** P2 · **Effort:** L · **Recommended subagent:** Sonnet-class · rust-installer subagent · **Why:** new module (reset_partition.rs) keeps collisions minimal; the nuke gate lives in the recovery flow, not the installer · **Depends on:** none (wave 6 — the LAST global wave: starts only after all `installer.rs`/`mod.rs` colliders — installer-robustness/TASK-01/05/07, phase-rerun/TASK-01/02 — are merged and siblings rebased)

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/boot-prod-reset-partition-populate" -b agent/boot-prod-reset-partition-populate origin/main
cd "$REPO/.worktrees/boot-prod-reset-partition-populate"
git rebase origin/main
```

(Protocol is also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

## Goal

Implement `docs/specs/reset-partition-design.md` (companion plan:
`docs/specs/reset-partition-plan.md`; its decisions are OPERATOR-LOCKED — do not reopen them):
a NEW module `src/network/ssh_installer/reset_partition.rs` whose `ResetPartitionStager::stage`
mounts RESET p2, copies the re-mastered SSH-ready recovery ISO onto it (size-gated,
skip-with-warning if it doesn't fit the 4 GiB partition), copies the debootstrap base tarball
when the `uaacache` device has one, writes the gated `uaa-reset.sh` helper, writes the
`/etc/grub.d/40_uaa_reset` loopback menu-entry drop-in into the target chroot BEFORE
`update-grub` runs, and unmounts — all idempotently re-runnable. Wire it with two minimal
hunks: `pub mod reset_partition;` in `mod.rs`, and a scoped, NON-FATAL `rp.stage(config)` call
in `installer.rs::phase_5_system_configuration` placed before `sc.configure_grub_in_chroot`.

**This task ONLY STAGES artifacts + a menu entry. The DESTRUCTIVE reset flow lives in the
recovery environment and gates on the operator literally typing `nuke it` — the installer
itself never wipes anything as part of this feature.** Selecting the GRUB entry merely boots
the recovery live environment (non-destructive).

Reuse — do NOT invent parallel machinery:

- **`partition_path` suffix-aware helper** from installer-robustness/TASK-01 (merged in wave 1;
  you are wave 6). Verify: `grep -rn "fn partition_path" src/network/ssh_installer/` — expect
  ≥1 hit. If (and only if) it is genuinely absent, fall back to the legacy
  `format!("{}p2", config.disk_device)` idiom and leave a
  `// TODO: switch to partition_path (installer-robustness/TASK-01) once merged` comment.
- **The Phase 4 uaacache mount incantation + tarball filename convention** —
  `mountpoint -q /mnt/uaacache || mount -o ro /dev/disk/by-label/uaacache /mnt/uaacache` and
  `{release}-$(dpkg --print-architecture)-base.tar.gz`, with `release` from
  `config.debootstrap_release` (same `unwrap_or("resolute")` default). Copy verbatim from
  `system_setup.rs` (grep below).
- **`CommandExecutor`** (`runner: &'a mut dyn CommandExecutor`) — the same borrowed-executor
  shape `DiskManager`/`SystemConfigurator` use. No new executor abstraction.
- **The `DiskManager` pure-command-builder + `#[cfg(test)]` test idiom** (`disk_ops.rs`
  builders ~380–419) for every shell command this module constructs.

## Background (verify before editing)

- RESET p2 (4 GiB, GPT 8300, label `RESET`) is **created** in `DiskManager::create_partitions`
  and **formatted** ext4 in `format_partitions`, then **never mounted or populated anywhere**
  — this task fills that gap. There are **no `/etc/grub.d` writes anywhere in ssh_installer
  today**; the drop-in is net-new.
- `update-grub` runs as the LAST step of `configure_grub_in_chroot` ("Updating GRUB config").
  `installer.rs::phase_5_system_configuration` calls `sc.configure_grub_in_chroot(config)`;
  your `stage()` call goes BEFORE the `SystemConfigurator` is constructed, so the drop-in
  exists when update-grub generates `grub.cfg` — **zero edits to `system_setup.rs`** (three
  other tasks collide there; the spec locked this placement, Decision 4).
- **ISO source (LOCKED, Decision 2): the live boot medium itself.** No in-repo kernel/initrd
  pipeline exists; the only bootable recovery artifact is the re-mastered stock Ubuntu Server
  ISO from `scripts/make-ssh-ready-iso.sh`, and the Path B live session was dd-booted from
  exactly that ISO. Detect it via `findmnt -n -o SOURCE /cdrom` + `isosize <dev>`; NO new
  config field (keeps this task out of the `config.rs` collision set). For the record, the
  same re-mastered ISO is also served from the server's web root `isos/` directory
  (`/var/www/html/isos` on 172.16.2.30, i.e. `http://172.16.2.30/isos/` — see the tree in
  `docs/netboot-autodeploy.md`); that is documentation for operators re-staging by hand, NOT a
  fetch path for this code (no HTTP download in v1).
- Debootstrap runs live from a mirror (no implicit tarball on the target) — the offline-reinstall
  tarball comes only from the optional `uaacache` device convention above.
- `todo.md` records the operator's original intent for this feature ("Populate the RESET
  partition": bootable USB copy + debootstrap tarball + GRUB reset entry gated on typing
  `nuke it`).
- **Re-verify these anchors before editing** — line numbers drift, they are a starting point
  only:

  ```bash
  grep -n "Create RESET (p2)" src/network/ssh_installer/disk_ops.rs   # expect: 1 hit ~line 273 (inside create_partitions, fn at ~245)
  grep -n "mkfs.ext4 -F -L RESET" src/network/ssh_installer/disk_ops.rs   # expect: 3 hits ~lines 325 (real, in format_partitions at ~314), 395 + 419 (cfg(test) builders)
  grep -rn "mount.*RESET\|RESET.*mount" src/ installer-image/   # expect: 0 hits
  grep -rn "grub.d" src/network/ssh_installer/   # expect: 0 hits
  grep -n "sc.configure_grub_in_chroot" src/network/ssh_installer/installer.rs   # expect: 1 hit ~line 542
  grep -n "Updating GRUB config" src/network/ssh_installer/system_setup.rs   # expect: 1 hit ~line 562
  grep -n "debootstrap {} /mnt/targetos" src/network/ssh_installer/installer.rs   # expect: 2 hits ~lines 572 (primary) and 580 (old-releases fallback)
  grep -n "Re-master a stock Ubuntu Server ISO" scripts/make-ssh-ready-iso.sh   # expect: 1 hit line 7
  grep -n "Populate the RESET partition" todo.md   # expect: 1 hit (the todo item; its line number drifts)
  grep -n "uaacache" src/network/ssh_installer/system_setup.rs   # expect: ~5 hits (Phase 4 cache mount + {release}-$(dpkg --print-architecture)-base.tar.gz convention)
  grep -n "fn phase_5_system_configuration" src/network/ssh_installer/installer.rs   # expect: 1 hit ~line 538
  grep -n "pub mod" src/network/ssh_installer/mod.rs   # expect: 8 module lines today; you add reset_partition
  grep -rn "fn partition_path" src/network/ssh_installer/   # DEPENDENCY-GATED anchor: >=1 hit only AFTER installer-robustness/TASK-01 merges; 0 hits at planning-time HEAD is EXPECTED and is NOT a STOP condition for THIS anchor only -> use the format!("{}p2") fallback + TODO documented below
  grep -n "isos/" docs/netboot-autodeploy.md   # expect: >=1 hit (the served ISO location under the server web root)
  ```

  Zero hits on any anchor marked ≥1 = STOP and report; do not guess. SOLE EXCEPTION: the
  `fn partition_path` anchor above is dependency-gated with its own documented fallback —
  0 hits there selects the fallback, it does not STOP the task.

- HARD RULES (restated):
  1. **NON-DESTRUCTIVE by construction:** this task stages files and a menu entry ONLY. The
     wipe gate (`Type exactly 'nuke it' to continue:`) exists solely inside the staged
     `uaa-reset.sh`, which runs in the recovery environment — never add a wipe/reset path to
     the installer binary's normal flow.
  2. NEVER install to, wipe, or touch 172.16.2.30 ("the server") or len-serv-003 — code-only,
     validated by unit tests + the QEMU gate (`testing-gates/TASK-01`), never on live hosts.
  3. `disk_device` comes from config (read from the live target by the operator/flow), and the
     p2 path comes from `partition_path` — never guess `/dev/sda2`-style paths.
  4. No secrets: nothing in this module reads or writes `luks_key`/passwords; the staged
     helper fetches config at recovery time through the existing placeholder-refusing flow.
  5. Workers stay in their worktree and NEVER push/PR/merge — the coordinator owns all git.
  6. Purely additive: do not move or modify any existing phase call; `installer.rs` gets one
     scoped block, `mod.rs` one line.

## Step-by-step

1. Run the ⛔ START HERE block, then all anchor greps. Read
   `docs/specs/reset-partition-design.md` in your worktree — it is the normative source for
   every literal below (signatures, script bodies, edge-case semantics). Confirm baseline:
   `cargo test --lib --offline` ≥237 passed.
2. **Create `src/network/ssh_installer/reset_partition.rs`** with the 4-line repo file header
   (`// file:` / `// version: 1.0.0` / `// guid:` fresh guid / `// last-edited: 2026-07-09`)
   and the spec's normative surface:

   ```rust
   pub struct ResetPartitionStager<'a> { runner: &'a mut dyn CommandExecutor }

   impl<'a> ResetPartitionStager<'a> {
       pub fn new(runner: &'a mut dyn CommandExecutor) -> Self;
       pub async fn stage(&mut self, config: &InstallationConfig) -> Result<()>;
       // internal steps:
       async fn mount_reset(&mut self, config: &InstallationConfig) -> Result<()>;
       async fn copy_recovery_iso(&mut self, config: &InstallationConfig) -> Result<bool>;
       async fn copy_debootstrap_tarball(&mut self, config: &InstallationConfig) -> Result<()>;
       async fn write_reset_helper(&mut self) -> Result<()>;
       async fn write_grub_dropin(&mut self, iso_present: bool) -> Result<()>;
       async fn unmount_reset(&mut self) -> Result<()>;
       // pure builders (unit-testable, non-test — stage() uses them):
       fn build_mount_reset_cmd(disk: &str) -> String;
       fn build_iso_size_cmd() -> String;
       fn build_iso_copy_cmd(src_dev: &str, size_bytes: u64) -> String;
       fn build_tarball_copy_cmd(release: &str) -> String;
       fn build_grub_dropin_cmd() -> String;
       fn build_grub_dropin_remove_cmd() -> String;
   }
   const RESET_HELPER_SH: &str = /* uaa-reset.sh body from the spec (C3) */;
   const GRUB_DROPIN: &str = /* 40_uaa_reset body from the spec (C4) */;
   const RESET_MOUNT: &str = "/mnt/reset";
   const RESET_ISO_NAME: &str = "uaa-recovery.iso";
   ```

3. **Mount/unmount (spec C1):** `build_mount_reset_cmd` uses `partition_path(disk, 2)` (or the
   documented fallback) and the idempotent guard
   `mkdir -p /mnt/reset; mountpoint -q /mnt/reset || mount <p2> /mnt/reset` — the same
   `mountpoint -q ||` idiom Phase 4 uses for `/mnt/uaacache`. `unmount_reset` runs
   `umount /mnt/reset || true` and MUST run on every exit path of `stage()` (including the
   warn-and-return paths) so RESET is never left mounted into Phase 6. Mount failure (e.g. p2
   missing on a hand-partitioned disk) → `warn!` + return `Ok(())` without staging.
4. **ISO copy with size gate (spec C2):**
   - Source: `findmnt -n -o SOURCE /cdrom` → the casper boot medium. Empty/missing (netboot or
     curtin session) → `warn!` + skip; `copy_recovery_iso` returns `false`. Do NOT error.
   - Exact length: `isosize <dev>`. Failure → warn + skip, return `false`.
   - **Size gate (LOCKED):** compare against `df --output=avail -B1 /mnt/reset`, reserving
     256 MiB headroom for tarball + helper. Too large for the 4 GiB partition →
     `warn!("RESET: recovery ISO ({} bytes) exceeds free space on p2 — skipping ISO + GRUB entry")`
     and return `false`. The install continues.
   - **Idempotent re-run:** if `/mnt/reset/uaa-recovery.iso.stamp` exists and records the same
     byte size as the current `isosize` output AND the on-disk ISO size matches, skip the copy
     (log "already staged") and return `true`. Any mismatch (interrupted prior copy) → re-copy.
   - Copy: `dd if=<dev> of=/mnt/reset/uaa-recovery.iso bs=4M count=<size> iflag=count_bytes
     conv=fsync`, THEN write the stamp (`<size> <dev> <date>`) — stamp only after a successful
     dd, so a crashed copy retries next run (fail-closed marker ordering).
5. **Tarball + helper (spec C3):**
   - Tarball: re-run the Phase 4 cache mount verbatim
     (`mountpoint -q /mnt/uaacache || mount -o ro /dev/disk/by-label/uaacache /mnt/uaacache || true`),
     compute `/mnt/uaacache/{release}-$(dpkg --print-architecture)-base.tar.gz` with `release`
     from `config.debootstrap_release` (`unwrap_or("resolute")`), and copy into
     `/mnt/reset/cache/` only if present AND it fits (df check again). Absent cache →
     info-level "no uaacache tarball; RESET gets ISO only" — NOT a warning, NOT an error. Use
     `cmp -s || cp` so an unchanged tarball is not rewritten on re-run.
   - Helper: write `RESET_HELPER_SH` to `/mnt/reset/uaa-reset.sh` (mode 0755) plus a
     `README.uaa-reset`. The helper body comes from the spec and is the ONLY place the
     destructive gate exists. Gate semantics (normative, implement exactly): interactive tty
     required (`[ -t 0 ]` else abort), prompt `Type exactly 'nuke it' to continue:`, and ONLY
     the exact literal `nuke it` proceeds — empty input, EOF, `NUKE IT`, `nuke it!`, or any
     other string aborts with exit 1. No `--yes` bypass flag.
6. **GRUB drop-in (spec C4):** `write_grub_dropin(iso_present)`:
   - `iso_present == false` → run `build_grub_dropin_remove_cmd()`:
     `rm -f /mnt/targetos/etc/grub.d/40_uaa_reset` (Decision 7: no menu entry without an ISO —
     also removes a stale entry from a prior run whose ISO no longer staged) and return.
   - `iso_present == true` → write `GRUB_DROPIN` to `/mnt/targetos/etc/grub.d/40_uaa_reset`
     and `chmod 0755` it (update-grub only executes executable drop-ins). Body is the spec's
     normative `40_custom`-style script: `exec tail -n +4 $0`, then a
     `menuentry "UAA reset/recover (boots recovery environment — non-destructive)"` block with
     `insmod part_gpt/ext2/loopback/iso9660`, `search --no-floppy --label RESET
     --set=reset_root`, `set isofile=/uaa-recovery.iso`, `loopback loop ($reset_root)$isofile`,
     `export iso_path`, `configfile /boot/grub/loopback.cfg`. The ISO's own `loopback.cfg` is
     already patched by `make-ssh-ready-iso.sh` with the NoCloud seed cmdline, so the loopback
     boot inherits SSH-ready behavior for free.
   - Ordering is guaranteed by step 8's placement: the drop-in lands before
     `configure_grub_in_chroot`'s final `update-grub` picks it up, in the same run.
7. **Wire `mod.rs`:** add `pub mod reset_partition;` alphabetically into the existing module
   list; bump the file header (version + last-edited, keep guid).
8. **Wire `installer.rs` (spec C5, the ONLY behavior change):** in
   `phase_5_system_configuration`, BEFORE the existing `let mut sc =
   SystemConfigurator::new(&mut *self.runner);`, insert a scoped block so the `&mut
   *self.runner` reborrow ends before `sc` takes it:

   ```rust
   {
       // Stage RESET p2 (recovery ISO + tarball + gated helper + GRUB drop-in).
       // Non-fatal by design: a staging problem must not break the proven 7/7 flow.
       let mut rp = ResetPartitionStager::new(&mut *self.runner);
       if let Err(e) = rp.stage(config).await {
           warn!("RESET partition staging skipped: {e}");
       }
   }
   ```

   `stage()` itself returns `Err` only on executor/transport failure; every CONTENT problem
   (no /cdrom, isosize failure, ISO too big, no uaacache, mount failure) degrades to a logged
   warning internally and the phase proceeds (LOCKED Decision 6: fail-open on staging,
   fail-closed only inside the recovery gate). Do not reorder or modify any existing call in
   the function. Bump the `installer.rs` header.
9. **Edge-case semantics recap (implement exactly; re-asserted in Acceptance):** no `/cdrom` →
   skip ISO + remove drop-in, install continues; ISO exceeds free space → skip ISO AND menu
   entry with warning, tarball/helper still staged if they fit; stamp size mismatch → re-copy;
   no uaacache → ISO-only, info log; p2 mount failure → skip everything with warning; every
   path unmounts `/mnt/reset`.
10. **Tests** (`#[cfg(test)] mod tests`, pure builder assertions per the `disk_ops.rs` idiom —
    no live executor). Implement the spec's test table verbatim:
    - `test_mount_reset_cmd_uses_partition_path` — p2 path correct for `/dev/nvme0n1`
      (`nvme0n1p2`) AND `/dev/sda` (`sda2`, NOT `sdap2`); contains `mountpoint -q` guard.
    - `test_iso_copy_cmd_exact_bytes` — dd carries `count=<size> iflag=count_bytes conv=fsync`
      and targets `/mnt/reset/uaa-recovery.iso`.
    - `test_grub_dropin_contents` — `GRUB_DROPIN` contains
      `search --no-floppy --label RESET`, `loopback loop`,
      `configfile /boot/grub/loopback.cfg`, and the literal title `UAA reset/recover`.
    - `test_grub_dropin_remove_cmd` — remove builder is `rm -f` on
      `/mnt/targetos/etc/grub.d/40_uaa_reset`.
    - `test_reset_helper_gate_literal` — `RESET_HELPER_SH` contains the exact string
      `nuke it`, the `[ -t 0 ]` tty check, and `exit 1` on mismatch; assert the gate is an
      exact-match comparison (rejects `NUKE IT` / `nuke it!` / empty) AND that the exact
      literal `nuke it` is what the comparison accepts — the intended confirmation must pass
      (anti-over-suppression).
    - `test_tarball_copy_uses_uaacache_convention` — builder embeds `/mnt/uaacache/`,
      `-base.tar.gz`, and `dpkg --print-architecture` verbatim.
11. Bump headers on all three touched/created files. Commit with the message below.

## How to test

```bash
cargo test --lib --offline
# Expected: >=243 passed (baseline 237 + the 6 new reset_partition tests); 0 failed
cargo build --offline
# Expected: exit 0
cargo clippy --offline
# Expected: no new warnings
```

## Acceptance criteria

- [ ] New module exists with the stager:
      `grep -n "pub struct ResetPartitionStager" src/network/ssh_installer/reset_partition.rs` — 1 hit.
- [ ] Exported: `grep -n "pub mod reset_partition" src/network/ssh_installer/mod.rs` — 1 hit.
- [ ] Wired non-fatally BEFORE the SystemConfigurator:
      `grep -n -B2 -A4 "ResetPartitionStager::new" src/network/ssh_installer/installer.rs`
      shows the scoped block with `if let Err(e) = rp.stage(config)` + `warn!`, and it appears
      at a LOWER line number than `SystemConfigurator::new` within
      `phase_5_system_configuration` (so the drop-in precedes update-grub).
- [ ] No `system_setup.rs` edits: `git diff --name-only origin/main` lists exactly
      `src/network/ssh_installer/reset_partition.rs`, `src/network/ssh_installer/mod.rs`,
      `src/network/ssh_installer/installer.rs` (locked Decision 4).
- [ ] Drop-in + gate literals present:
      `grep -c "40_uaa_reset" src/network/ssh_installer/reset_partition.rs` ≥2 (write + remove) and
      `grep -n "nuke it" src/network/ssh_installer/reset_partition.rs` ≥1 and
      `grep -n "configfile /boot/grub/loopback.cfg" src/network/ssh_installer/reset_partition.rs` ≥1.
- [ ] Idempotency stamp implemented:
      `grep -n "uaa-recovery.iso.stamp" src/network/ssh_installer/reset_partition.rs` ≥1.
- [ ] Skip-not-fail semantics: `grep -c "warn!" src/network/ssh_installer/reset_partition.rs`
      ≥3 (no-/cdrom, size-gate, mount-failure paths) and no `?` on those content paths (they
      return `Ok`/`false`).
- [ ] Anti-over-suppression: `test_reset_helper_gate_literal` passes — the gate rejects
      near-misses AND accepts the exact `nuke it` literal; and `test_grub_dropin_contents`
      passes — with an ISO present the menu entry IS written (the no-ISO removal path does not
      suppress the happy path).
- [ ] Tests green: `cargo test --lib --offline` ≥243 passed, 0 failed (all 6 spec-table test
      names present in the output); `cargo build --offline` exit 0; `cargo clippy --offline`
      clean.
- [ ] File headers: fresh guid + `last-edited: 2026-07-09` in `reset_partition.rs`; version
      bumped + `last-edited: 2026-07-09` in `mod.rs` and `installer.rs`
      (`grep -n "last-edited: 2026-07-09" <file>` each).

## Commit message

```
feat(installer): stage RESET p2 recovery ISO + tarball + gated GRUB reset entry

New reset_partition.rs module (ResetPartitionStager): mounts RESET p2, copies
the SSH-ready recovery ISO from the live boot medium (size-gated, stamp-file
idempotent), copies the uaacache debootstrap tarball when present, stages the
'nuke it'-gated uaa-reset.sh helper, and writes /etc/grub.d/40_uaa_reset so
update-grub emits a loopback "UAA reset/recover" entry. Wired as a scoped,
non-fatal call in phase_5_system_configuration before configure_grub_in_chroot.
Staging only — the destructive reset gate lives in the recovery environment.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

Idempotency (additive): `grep -n "pub mod reset_partition" src/network/ssh_installer/mod.rs && test -f src/network/ssh_installer/reset_partition.rs`
— if both hit, the module is already applied; run the acceptance checks instead of
re-applying. (At runtime, `stage()` is itself re-run-safe via the `mountpoint -q` guard, the
ISO stamp file, and `cmp -s || cp`.) Rollback: `git revert` the single commit — RESET returns
to created+formatted-but-never-populated, no `40_uaa_reset` drop-in, no menu entry, phase
sequence byte-identical to before; siblings unaffected. On an already-installed host,
operational rollback is `rm /etc/grub.d/40_uaa_reset && update-grub` (entry gone) and
optionally re-mkfs of p2 (content gone); ESP/BPOOL/LUKS layout untouched.
