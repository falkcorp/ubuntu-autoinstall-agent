<!-- file: docs/agent-tasks/boot-prod/TASK-01-efibootmgr-chroot.md -->
<!-- version: 1.0.0 -->
<!-- guid: d6086c66-f52d-453a-b098-d5b9dcf6804b -->
<!-- last-edited: 2026-07-09 -->

# TASK-01 — efibootmgr in chroot post-update-grub: BootOrder = network #1, ubuntu #2 (non-fatal on legacy BIOS) (todo:efibootmgr)

**Priority:** P1 · **Effort:** M · **Recommended subagent:** Sonnet-class · rust-installer subagent · **Why:** single insertion point after update-grub; mirrors proven set_boot_order regexes from uaa-usb-bootstrap.sh · **Depends on:** none (wave 3 — starts only after installer-robustness/TASK-01 and TASK-04 merge; same-file collision on `system_setup.rs`)

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/boot-prod-efibootmgr-chroot" -b agent/boot-prod-efibootmgr-chroot origin/main
cd "$REPO/.worktrees/boot-prod-efibootmgr-chroot"
git rebase origin/main
```

(Protocol is also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

## Goal

Add a **UEFI boot-order step** to `SystemConfigurator::configure_grub_in_chroot`
(`src/network/ssh_installer/system_setup.rs`) that runs **immediately AFTER** the existing
`update-grub` call ("Updating GRUB config" step) and sets BootOrder to: all network entries
first, the `ubuntu` entry second, everything else after — inside `chroot /mnt/targetos`, where
efivarfs is already mounted by the earlier "Ensure efivarfs" step. The step MUST be non-fatal:
on legacy-BIOS hosts (no EFI variables — e.g. U1's legacy-OpROM IMSM array) it logs and
continues, and Phase 5 still completes.

Reuse — do NOT invent parallel machinery:

- **`log_and_execute`** (`system_setup.rs`, helper on `SystemConfigurator`) for running the
  command — same as every other step in `configure_grub_in_chroot`. Call it with `let _ =` so
  a non-zero exit never propagates.
- **The `chroot /mnt/targetos bash -lc '...'` idiom** used by the neighboring steps (e.g. the
  "Ensure efivarfs" and `update-grub` commands) — copy that invocation shape verbatim.
- **The exact entry-matching regexes from `set_boot_order`** in
  `installer-image/nocloud/uaa-usb-bootstrap.sh` (definition ~line 84, called ~line 142):
  network entries via `PXE\|[Nn]etwork\|IPv[46]`, ubuntu via `^Boot####\*? [Uu]buntu`,
  dedup via awk — so live-env (USB) and chroot behavior stay identical. Do NOT redesign the
  matching.
- **`efibootmgr` is already installed in the chroot** — it is in the apt package lists at
  `system_setup.rs` (~376) and `installer.rs` (~603). Do NOT add a package install step.

## Background (verify before editing)

- `configure_grub_in_chroot` (fn at ~490) re-binds /dev,/proc,/sys,/run, mounts the ESP and
  **efivarfs** ("Ensure efivarfs", ~523 — so an in-chroot `efibootmgr` can read/write NVRAM),
  optionally seds `GRUB_CMDLINE_LINUX`, then runs a **3-tier grub-install fallback**
  (`--uefi-secure-boot` → `--no-nvram` → `--removable`, ~548/552/556) and finally `update-grub`
  at the "Updating GRUB config" step (~562). Your new step slots right after that final step,
  still inside the function.
- Because fallback tiers 2/3 use `--no-nvram`/`--removable`, **an NVRAM entry named `ubuntu`
  may not exist at all** — the ordering logic must tolerate an absent ubuntu entry (the USB
  script's compose-then-dedup approach already does: an empty `$ubuntu` fragment just drops
  out).
- The proven reference implementation is `set_boot_order()` in
  `installer-image/nocloud/uaa-usb-bootstrap.sh`: capture `efibootmgr` output (unreadable ⇒
  legacy BIOS ⇒ skip), sed-extract net/ubuntu/rest Boot#### ids, concatenate
  `net,ubuntu,rest`, dedup preserving order with `awk '!seen[$0]++'`, then `efibootmgr -o
  "$order"` — every failure path logs and returns 0.
- Context (not this task's job to fix): `todo.md` records that U1 once booted the USB even
  with a correct BootOrder (`grep -n "still booted the USB" todo.md` — 1 hit); boot-order is
  necessary but may not be sufficient on that host. Ship the step anyway.
- **Re-verify these anchors before editing** — line numbers drift, they are a starting point
  only:

  ```bash
  grep -n "fn configure_grub_in_chroot" src/network/ssh_installer/system_setup.rs   # expect: 1 hit ~line 490
  grep -c "grub-install --target=x86_64-efi" src/network/ssh_installer/system_setup.rs   # expect: count = 3 (~lines 548, 552, 556)
  grep -n "Updating GRUB config" src/network/ssh_installer/system_setup.rs   # expect: 1 hit ~line 562
  grep -n "fn log_and_execute" src/network/ssh_installer/system_setup.rs   # expect: 1 hit ~line 937
  grep -n "runner: &'a mut dyn CommandExecutor" src/network/ssh_installer/system_setup.rs   # expect: 2 hits ~lines 18, 22
  grep -n "sc.configure_grub_in_chroot" src/network/ssh_installer/installer.rs   # expect: 1 hit ~line 542
  grep -n "set_boot_order" installer-image/nocloud/uaa-usb-bootstrap.sh   # expect: 2 hits: line 84 (definition) and line 142 (call)
  grep -rn "efibootmgr" src/   # expect: 2 hits, both apt-install package lists: system_setup.rs:376 and installer.rs:603
  grep -n "Ensure efivarfs" src/network/ssh_installer/system_setup.rs   # expect: 2 hits — "Ensure efivarfs in chroot" (~320) and "Ensure efivarfs" (~523); the ~523 one is inside configure_grub_in_chroot
  ```

  Zero hits on any anchor = STOP and report; do not guess.

- HARD RULES (restated): this task is code-only — NEVER install to, wipe, or touch
  172.16.2.30 ("the server") or len-serv-003; validation is unit tests + the QEMU gate, never
  a live host. Stay in your worktree; NEVER `git push`/`gh pr`/merge — the coordinator owns
  git. Purely additive: do not modify the grub-install tiers, the update-grub step's error
  propagation, or any other existing step.

## Step-by-step

1. Run the ⛔ START HERE block, then the anchor greps above. Confirm 237+ passing tests
   baseline: `cargo test --lib --offline`.
2. In `src/network/ssh_installer/system_setup.rs`, add a **pure command builder** (associated
   fn, no `self` I/O — mirrors the file's existing testable-builder style):

   ```rust
   /// BootOrder script: network entries first, ubuntu second, rest after.
   /// Regexes are copied VERBATIM from set_boot_order() in
   /// installer-image/nocloud/uaa-usb-bootstrap.sh so USB and chroot behave
   /// identically. Every failure path exits 0 (non-fatal by design).
   fn build_boot_order_cmd() -> String
   ```

   It returns one `chroot /mnt/targetos bash -lc '...'` string whose inner script is the USB
   `set_boot_order` body, adapted to a single-quoted `-lc` payload:

   - `command -v efibootmgr >/dev/null 2>&1 || { echo "uaa: efibootmgr not present; skipping boot order"; exit 0; }`
   - `entries="$(efibootmgr 2>/dev/null)" || { echo "uaa: efibootmgr unreadable (legacy BIOS?); skipping boot order"; exit 0; }`
   - net ids: `sed -n "s/^Boot\([0-9A-Fa-f]\{4\}\)\*\{0,1\}[[:space:]].*\(PXE\|[Nn]etwork\|IPv[46]\).*/\1/p"`
   - ubuntu id: `sed -n "s/^Boot\([0-9A-Fa-f]\{4\}\)\*\{0,1\}[[:space:]][Uu]buntu.*/\1/p"`
   - rest: `sed -n "s/^Boot\([0-9A-Fa-f]\{4\}\)\*\{0,1\}[[:space:]].*/\1/p"`
   - compose `net,ubuntu,rest` → `tr "," "\n" | grep -v "^$" | awk "!seen[\$0]++" | paste -sd, -`
   - `[ -n "$order" ] || { echo "uaa: no EFI boot entries found; skipping boot order"; exit 0; }`
   - `efibootmgr -o "$order" && echo "uaa: BootOrder set: $order" || echo "uaa: efibootmgr -o failed (non-fatal)"; exit 0`

   Quoting rules (this is where weak implementations break): the whole inner script sits
   inside the OUTER single quotes of `bash -lc '...'`, so the script may contain NO single
   quotes — quote sed/awk programs with DOUBLE quotes, and escape awk's positional as `\$0`
   (in Rust, write the string as a raw string `r#"..."#` and join lines with `; ` exactly like
   the existing "Ensure efivarfs" command at ~523 — copy its quoting shape verbatim).
3. Add a private step method on `SystemConfigurator`:

   ```rust
   /// Best-effort UEFI boot order (network first, ubuntu second). Non-fatal:
   /// legacy-BIOS / no-efivars hosts log and continue; Phase 5 still completes.
   async fn set_uefi_boot_order(&mut self) -> Result<()>
   ```

   Body: `self.log_and_execute("Set UEFI BootOrder (network first, ubuntu second)",
   &Self::build_boot_order_cmd()).await` — and at the call site use `let _ =` (see step 4) so
   remote non-zero exits are swallowed there too (belt and suspenders: the script itself exits
   0, and the call site ignores errors — an SSH transport hiccup on this step must not fail the
   phase either).
4. Insert the call in `configure_grub_in_chroot` **immediately after** the existing
   "Updating GRUB config" `log_and_execute(...).await?;` statement and before the function's
   `Ok(())`:

   ```rust
   // Best-effort: order NVRAM entries network-first, ubuntu-second. Mirrors
   // set_boot_order() in uaa-usb-bootstrap.sh. MUST stay non-fatal (let _ =):
   // legacy-BIOS hosts have no efivars, and grub-install --no-nvram/--removable
   // fallbacks mean the "ubuntu" entry may not exist.
   let _ = self.set_uefi_boot_order().await;
   ```

   Do NOT touch the update-grub statement itself — it keeps its `?` (a real GRUB config
   failure must still fail Phase 5; only the boot-order step is best-effort).
5. Edge-case semantics (implement exactly — also asserted in Acceptance):
   - **Legacy BIOS / no efivars:** `efibootmgr` output capture fails → echo skip message →
     `exit 0`. Phase 5 continues; a successful grub phase without efivars still completes.
   - **No `ubuntu` NVRAM entry** (no-nvram/removable fallback tiers): the ubuntu sed fragment
     is empty; the composed order is just `net,rest` — still applied. Absence is NOT an error.
   - **No entries at all** (empty `$order`): skip with message, `exit 0`.
   - **`efibootmgr -o` fails** (firmware quirk): log "(non-fatal)", `exit 0`.
6. Add `#[cfg(test)]` unit tests next to the file's existing tests:
   - `test_boot_order_cmd_matches_usb_script_regexes` — `build_boot_order_cmd()` output
     contains the three literal sed fragments above (assert on the distinctive substrings
     `PXE`, `[Nn]etwork`, `IPv[46]`, `[Uu]buntu`, `[0-9A-Fa-f]\{4\}`) and `efibootmgr -o`.
   - `test_boot_order_cmd_is_chrooted_and_nonfatal` — output starts with
     `chroot /mnt/targetos bash -lc`, contains no interior single quote between the `-lc`
     delimiters, and every skip path carries `exit 0`.
   - `test_boot_order_cmd_attempts_order_when_entries_exist` — the script reaches
     `efibootmgr -o "$order"` whenever `$order` is non-empty, i.e. the happy path is not
     gated on the ubuntu entry existing (assert the `-o` invocation is guarded only by
     `[ -n "$order" ]`, not by the ubuntu fragment) — anti-over-suppression.
7. Bump the file header on `src/network/ssh_installer/system_setup.rs` (version + last-edited
   2026-07-09; keep the guid). Commit with the message below.

## How to test

```bash
cargo test --lib --offline
# Expected: >=237 passed (baseline 237 + your new tests); 0 failed
cargo build --offline
# Expected: exit 0
cargo clippy --offline
# Expected: no new warnings
```

## Acceptance criteria

- [ ] `grep -c "set_uefi_boot_order" src/network/ssh_installer/system_setup.rs` returns ≥2
      (definition + call site).
- [ ] The call sits directly after update-grub:
      `grep -n -A8 '"Updating GRUB config"' src/network/ssh_installer/system_setup.rs | grep "set_uefi_boot_order"`
      returns 1 hit, and the call line contains `let _ =` (non-fatal).
- [ ] update-grub error propagation unchanged:
      `grep -n -A2 '"Updating GRUB config"' src/network/ssh_installer/system_setup.rs` still
      shows the statement ending in `.await?;`.
- [ ] Regex parity with the USB script:
      `grep -c 'PXE' src/network/ssh_installer/system_setup.rs` ≥1 and
      `grep -c 'IPv\[46\]' src/network/ssh_installer/system_setup.rs` ≥1.
- [ ] No new package install: `grep -rn "apt install" src/network/ssh_installer/system_setup.rs | grep -c efibootmgr`
      is unchanged from before your edit (efibootmgr was already listed).
- [ ] Anti-over-suppression: `cargo test --lib --offline` shows
      `test_boot_order_cmd_attempts_order_when_entries_exist` passing — the new skip/guard
      paths (legacy-BIOS, empty order) do not suppress the happy path, and absent-ubuntu still
      yields a `net,rest` order.
- [ ] Tests green: `cargo test --lib --offline` ≥237 passed, 0 failed; `cargo build --offline`
      exit 0; `cargo clippy --offline` clean.
- [ ] File header bumped: `grep -n "last-edited: 2026-07-09" src/network/ssh_installer/system_setup.rs`
      returns 1 hit (guid unchanged).

## Commit message

```
feat(installer): set UEFI BootOrder in chroot after update-grub (network first, ubuntu second)

Add SystemConfigurator::set_uefi_boot_order, called non-fatally right after the
"Updating GRUB config" step in configure_grub_in_chroot (efivarfs already
mounted). Reuses the exact set_boot_order regexes from uaa-usb-bootstrap.sh so
USB live-env and chroot ordering behave identically; legacy-BIOS / no-efivars /
absent-ubuntu-entry hosts log and continue.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

Idempotency (additive): `grep -n "set_uefi_boot_order" src/network/ssh_installer/system_setup.rs`
— if this hits, the step is already applied; run the acceptance checks instead of re-applying.
Rollback: revert the single commit — `configure_grub_in_chroot` returns to ending at
update-grub, no NVRAM ordering is attempted, no other behavior or state is touched, siblings
unaffected.
