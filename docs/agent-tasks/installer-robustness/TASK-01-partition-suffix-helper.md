<!-- file: docs/agent-tasks/installer-robustness/TASK-01-partition-suffix-helper.md -->
<!-- version: 1.0.0 -->
<!-- guid: 82570f64-ac07-4b25-94aa-67e422c81145 -->
<!-- last-edited: 2026-07-09 -->

# TASK-01 — Route all 11 partition-path call sites through one suffix-aware helper (fix /dev/sdapN bug) (todo:partition-suffix)

**Priority:** P1 · **Effort:** L · **Recommended subagent:** Opus-class · rust-systems subagent · **Why:** wide-collision transform across 4 installer files; a wrong suffix targets the wrong device path; blocks the QEMU virtio gate. · ⚠ review-critical · **Depends on:** none

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/installer-robustness-partition-suffix-helper" -b agent/installer-robustness-partition-suffix-helper origin/main
cd "$REPO/.worktrees/installer-robustness-partition-suffix-helper"
git rebase origin/main
```

(Protocol is also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

## Goal

Create ONE suffix-aware helper — `pub fn partition_path(disk: &str, n: u32) -> String` — in a
NEW file `src/network/ssh_installer/partitions.rs` (registered as `pub mod partitions;` in
`src/network/ssh_installer/mod.rs`), and route EVERY production `format!("{}pN", disk)`
partition-path site in `ssh_installer` through it, plus the two `#[cfg(test)]` command builders
in `disk_ops.rs` so tests and production can never diverge.

The Linux naming rule the helper implements — exactly this, nothing more:
**append a `p` separator only when the disk name's LAST character is an ASCII digit.**
`/dev/nvme0n1` → `nvme0n1p3`, `/dev/md126` → `md126p3`, `/dev/md/Volume0_0` → `…Volume0_0p3`
(ends in the digit `0`), but `/dev/sda` → `sda3` and `/dev/vda` → `vda3`. Today the `p` is
hardcoded everywhere, so any `/dev/sdX` or `/dev/vdX` target gets nonexistent `/dev/sdapN`
paths — this is what blocks the QEMU virtio (`/dev/vda`) validation gate.

Reuse-don't-invent: there is NO existing generic helper (verified below — only the ESP-specific
`detect_esp_partition_path`/`choose_esp_partition` GUID detection exists, which stays untouched
except for its two `{}pN` fallback strings). Do NOT add a second helper, a trait, or a config
method; every converted site calls the one free function.

## Background (verify before editing)

- Evidence (scout-verified 2026-07-09): partition paths are built by inline `format!("{}pN", disk)`
  at production call sites in 4 files plus 2 test-helper builders — 12 production `format!`
  lines in total, one of which (`installer.rs` crypttab shell line, ~605) embeds the p4 path
  **twice in one format string**. A naive per-`format!` sweep misses that second occurrence.
- Four existing tests assert the buggy `sdapN` output (`zfs_ops.rs` ~381, `system_setup.rs`
  ~1001/~1026/~1032). They will correctly FAIL after the fix — update them to `sda1`/`sda3`/`sda4`
  in this same task; they become the regression tests for the helper.
- `src/utils/qemu.rs` ~118 appends `p1` to a LOOP device. `/dev/loopN` always ends in a digit,
  so that site is **correct as-is — EXCLUDE it, do not change that file at all**.
- A stale doc comment at `system_setup.rs` ~66 mentions a `${DISK}p1` fallback — update the
  comment text alongside the code or it documents stale behavior.
- `disk_device` enters via YAML (`InstallationConfig::from_yaml_file`) or the
  `for_len_serv_003()` default (`/dev/nvme0n1`); the ESP-only runtime GUID detection
  (`detect_esp_partition_path`) partially masks the bug for phases 4–6, but phases 2–3
  (mkfs/cryptsetup/zpool) hit the wrong paths directly.
- HARD RULES (operation-wide, restated): this task is code-only — NEVER run the installer
  against 172.16.2.30 ("the server") or len-serv-003; validation is `cargo test` here and
  QEMU later (testing-gates/TASK-01 depends on this task). `disk_device` is READ from the live
  target, never guessed — on U1, `/dev/sda` is an IMSM RAID *member*; the real volume is
  `/dev/md126`. Workers never push/PR/merge — the coordinator owns all git.

**Re-verify these anchors before editing** — line numbers drift, they are a starting point only.
Zero hits on any of these = STOP and report:

```bash
grep -rn '}p[1-4]' src --include='*.rs'   # expect: 15 code hits + 1 doc-comment hit (system_setup.rs:66); files: system_setup.rs, zfs_ops.rs, installer.rs, disk_ops.rs, utils/qemu.rs
grep -n 'bpool {}p3' src/network/ssh_installer/zfs_ops.rs   # expect: 1 hit ~line 200 (fn build_bpool_create_command at line 194)
grep -n 'ends_with("/dev/sdap3")' src/network/ssh_installer/zfs_ops.rs   # expect: 1 hit ~line 381
grep -rn 'sdap' src --include='*.rs'   # expect: 4 hits: zfs_ops.rs:381 (sdap3), system_setup.rs:1001 (/dev/sdap1), system_setup.rs:1026 and :1032 (luks /dev/sdap4)
grep -n 'mkfs.vfat -F32 -n ESP {}p1\|mkfs.ext4 -F -L RESET {}p2' src/network/ssh_installer/disk_ops.rs   # expect: 4 hits: 320, 325 (production in format_partitions) and 390, 395 (#[cfg(test)] build_mkfs_esp/build_mkfs_reset)
grep -n 'cryptsetup luksFormat --batch-mode {}p4\|cryptsetup open {}p4 luks' src/network/ssh_installer/disk_ops.rs   # expect: 2 hits ~lines 340 and 348 (fn setup_luks_encryption at line 333)
grep -n '}p[1-4]' src/network/ssh_installer/system_setup.rs   # expect: 6 hits: 46, 51, 60, 573, 884 (code) + 66 (doc comment)
grep -n 'fn build_crypttab_entry\|fn choose_esp_partition\|fn setup_luks_key_in_chroot\|fn setup_tpm2_firstboot_enrollment' src/network/ssh_installer/system_setup.rs   # expect: 4 hits: lines 43, 57, 570, 871
grep -n '}p[1-4]' src/network/ssh_installer/installer.rs   # expect: 2 hits: 566 (esp_part = format!("{}p1"...)) and 605 (blkid {d}p4 / DEV="{d}p4"); enclosing fn build_next_commands_after_storage at line 565
grep -n '}p1' src/utils/qemu.rs   # expect: 1 hit ~line 118: let partition = format!("{}p1", loop_device)  — EXCLUDED, do not edit
grep -rn 'fn partition\|partition_path\|part_suffix' src --include='*.rs'   # expect: hits only for detect_esp_partition_path (system_setup.rs:67) and choose_esp_partition (57) plus their tests; no generic helper
```

## Step-by-step

1. Run the anchor greps above in your worktree. Any zero-hit or wildly different count → STOP
   and report (a sibling task may have moved the code).
2. **Create `src/network/ssh_installer/partitions.rs`** with a 4-line Rust header
   (`// file: src/network/ssh_installer/partitions.rs`, `// version: 1.0.0`,
   `// guid: <generate with: uuidgen | tr 'A-Z' 'a-z'>`, `// last-edited: <today>`), then:

   ```rust
   /// Build the path of partition `n` on `disk`, following the kernel naming
   /// rule: insert a `p` separator only when the disk name ends in a digit.
   /// `/dev/nvme0n1` -> `/dev/nvme0n1p3`, `/dev/md126` -> `/dev/md126p3`,
   /// `/dev/sda` -> `/dev/sda3`, `/dev/vda` -> `/dev/vda3`.
   pub fn partition_path(disk: &str, n: u32) -> String {
       if disk.chars().last().is_some_and(|c| c.is_ascii_digit()) {
           format!("{disk}p{n}")
       } else {
           format!("{disk}{n}")
       }
   }
   ```

   Edge-case semantics (do not deviate): the decision is ONLY "is the last character an ASCII
   digit" — no device-name prefix special-casing (no `nvme`/`md`/`loop` matching), and an empty
   `disk` string returns just the number (degenerate, must not panic).
3. Add a `#[cfg(test)] mod tests` in `partitions.rs` covering exactly these cases:
   - `("/dev/nvme0n1", 3)` → `"/dev/nvme0n1p3"` (p-suffix preserved — the proven hardware path)
   - `("/dev/md126", 4)` → `"/dev/md126p4"`
   - `("/dev/sda", 1)` → `"/dev/sda1"` (no p)
   - `("/dev/vda", 4)` → `"/dev/vda4"` (no p — the QEMU virtio case)
   - `("/dev/md/Volume0_0", 3)` → `"/dev/md/Volume0_0p3"` (ends in a digit → p)
   - `("", 1)` → `"1"` (no panic)
4. **Register the module:** in `src/network/ssh_installer/mod.rs`, add `pub mod partitions;`
   alongside the existing `pub mod` lines (after `pub mod packages;`). Bump the file header.
5. **Convert `disk_ops.rs`** (add `use super::partitions::partition_path;`):
   - `format_partitions`: `"mkfs.vfat -F32 -n ESP {}p1"` → `format!("mkfs.vfat -F32 -n ESP {}", partition_path(&config.disk_device, 1))`; same shape for the `mkfs.ext4 -F -L RESET` p2 line.
   - `setup_luks_encryption`: both the `cryptsetup luksFormat --batch-mode {}p4` and
     `cryptsetup open {}p4 luks` format strings — pass `partition_path(&config.disk_device, 4)`
     instead of the raw device (keep the surrounding `echo '{}' |` passphrase piping EXACTLY as
     is — TASK-05 owns changing that).
   - `#[cfg(test)]` builders `build_mkfs_esp` / `build_mkfs_reset`: route through
     `partition_path(disk, 1)` / `partition_path(disk, 2)` too — if only production switches,
     the test builders silently diverge.
6. **Convert `zfs_ops.rs`**: in `build_bpool_create_command`, change the format-string tail
   `bpool {}p3` to `bpool {}` with `partition_path(disk, 3)` as the argument.
7. **Convert `system_setup.rs`** (5 code sites + 1 doc comment):
   - `build_crypttab_entry`: BOTH fallback branches `format!("{}p4", disk_device)` →
     `partition_path(disk_device, 4)`.
   - `choose_esp_partition`: fallback `format!("{}p1", default_disk)` →
     `partition_path(default_disk, 1)`.
   - `setup_luks_key_in_chroot`: `let part = format!("{}p4", config.disk_device);` →
     `let part = partition_path(&config.disk_device, 4);`.
   - `setup_tpm2_firstboot_enrollment`: fallback `format!("{}p4", config.disk_device)` →
     `partition_path(&config.disk_device, 4)`.
   - Doc comment (~line 66, on `detect_esp_partition_path`): replace the stale `${DISK}p1`
     wording with e.g. "fallback to partition 1 of the configured disk (suffix-aware:
     nvme0n1p1 / sda1) if not found".
8. **Convert `installer.rs`** (in `build_next_commands_after_storage`):
   - `let esp_part = format!("{}p1", config.disk_device);` →
     `let esp_part = partition_path(&config.disk_device, 1);`.
   - The crypttab shell line (~605): precompute
     `let p4 = partition_path(&config.disk_device, 4);` and replace **BOTH** `{d}p4`
     occurrences in that single format string (`blkid -s UUID -o value {d}p4` AND
     `DEV="{d}p4"`) with the precomputed `{p4}` — do NOT miss the second occurrence; the rest of
     the shell line stays byte-identical.
9. **Fix the four baked-in-bug tests** (same task, not a follow-up):
   - `zfs_ops.rs` ~381: `ends_with("/dev/sdap3")` → `ends_with("/dev/sda3")`.
   - `system_setup.rs` ~1001: expected `"/dev/sdap1"` → `"/dev/sda1"`.
   - `system_setup.rs` ~1026 and ~1032: expected `"luks /dev/sdap4 none luks,discard,initramfs"`
     → `"luks /dev/sda4 none luks,discard,initramfs"`.
10. **Do NOT touch `src/utils/qemu.rs`** — the loop-device `{}p1` there is already correct
    (`/dev/loopN` ends in a digit). "Fixing" it into the helper is acceptable behavior-wise but
    is explicitly OUT OF SCOPE for this task's file set — leave the file with zero diff.
11. Purely-transform scope: besides the substitutions above, do not reorder functions, rename
    variables, change function signatures, or alter any shell command text.
12. Bump the file header (`version` + `last-edited`, keep `guid`) on every touched file:
    `mod.rs`, `disk_ops.rs`, `zfs_ops.rs`, `system_setup.rs`, `installer.rs` (+ the new header
    in `partitions.rs`). Run the gate, then commit.

## How to test

```bash
cargo test --lib --offline    # Expected: 237+ passed; 0 failed (baseline 237 + the new partitions tests)
cargo build --offline         # Expected: exit 0
cargo clippy --offline        # Expected: no new warnings
```

## Acceptance criteria

- [ ] `grep -n 'pub fn partition_path' src/network/ssh_installer/partitions.rs` → exactly 1 hit.
- [ ] `grep -n 'pub mod partitions;' src/network/ssh_installer/mod.rs` → exactly 1 hit.
- [ ] `grep -rn '}p[1-4]' src --include='*.rs'` → exactly 1 remaining hit, in
  `src/utils/qemu.rs` (the intentionally excluded loop-device site); zero hits remain anywhere
  under `src/network/ssh_installer/` (including the rewritten doc comment).
- [ ] `grep -rn 'sdap' src --include='*.rs'` → 0 hits (all four tests now assert `sda1`/`sda3`/`sda4`).
- [ ] `grep -ln 'partition_path' src/network/ssh_installer/*.rs` lists `disk_ops.rs`,
  `installer.rs`, `partitions.rs`, `system_setup.rs`, `zfs_ops.rs`.
- [ ] `git diff --quiet origin/main -- src/utils/qemu.rs` exits 0 (file untouched).
- [ ] The virtio case is unit-proven: `cargo test --lib --offline partition` runs the
  `partitions.rs` tests including `/dev/vda` → `/dev/vda4` (no `p`).
- [ ] Tests green: `cargo test --lib --offline` reports 237+ passed, 0 failed; `cargo build
  --offline` and `cargo clippy --offline` clean.
- [ ] File headers bumped on every touched file (`grep -n 'last-edited:' <file>` shows today's
  date; guids unchanged).
- [ ] Anti-over-suppression: N/A (no filter/guard/skip path is added; the
  `nvme0n1 → nvme0n1p3` helper test proves the existing p-suffix behavior of the proven
  hardware path is preserved).

## Commit message

```
fix(installer): route all partition paths through suffix-aware partition_path helper

/dev/sda- and /dev/vda-style disks got nonexistent /dev/sdapN paths because the
"p" separator was hardcoded at every format! site. The new
ssh_installer::partitions::partition_path(disk, n) appends "p" only when the
disk name ends in a digit (nvme0n1p3, md126p3 vs sda3, vda3). Converts every
production {}pN site in disk_ops/zfs_ops/system_setup/installer plus the two
cfg(test) mkfs builders, fixes the four tests that asserted the buggy sdapN
output, updates the stale ${DISK}p1 doc comment, and leaves utils/qemu.rs
untouched (loop devices always end in a digit). Unblocks the QEMU virtio gate.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

Already-done check (transform polarity — new symbol present at the new location AND the old
pattern absent): if `grep -n 'pub fn partition_path' src/network/ssh_installer/partitions.rs`
hits AND `grep -rn '}p[1-4]' src/network/ssh_installer --include='*.rs'` returns 0 hits, the
task is already applied — run the acceptance checks instead of re-applying. Rollback:
`git revert` the single commit — restores the inline `{}pN` format strings, the `sdapN` test
assertions, and deletes `partitions.rs`; no data or on-disk state is touched, siblings
unaffected.
