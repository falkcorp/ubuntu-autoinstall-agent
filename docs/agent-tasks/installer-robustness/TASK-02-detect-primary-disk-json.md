<!-- file: docs/agent-tasks/installer-robustness/TASK-02-detect-primary-disk-json.md -->
<!-- version: 1.0.0 -->
<!-- guid: 070615fe-d5e2-4d17-b0ff-0ebed38465dc -->
<!-- last-edited: 2026-07-09 -->

# TASK-02 — detect_primary_disk: parse lsblk --json (incl. md devices) instead of fragile text matching (todo:detect_primary_disk)

**Priority:** P2 · **Effort:** M · **Recommended subagent:** Sonnet-class · rust-backend subagent · **Why:** single-file parser rewrite with clear fixture tests. · **Depends on:** none

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/installer-robustness-detect-primary-disk-json" -b agent/installer-robustness-detect-primary-disk-json origin/main
cd "$REPO/.worktrees/installer-robustness-detect-primary-disk-json"
git rebase origin/main
```

(Protocol is also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

## Goal

Rewrite `detect_primary_disk` in `src/cli/commands.rs` (the ONLY file this task touches) as a
pure parser over `lsblk --json -b -o NAME,TYPE,SIZE` output, and have its single caller
(`create_local_installation_config`) obtain that JSON from the LIVE target instead of feeding
it the free-text `lsblk -a` blob. Include md arrays (with precedence over their member disks),
include nvme/sd/vd `TYPE=disk` devices, exclude `loop`/`rom`, and choose the largest candidate.

Reuse-don't-invent: `serde_json` is ALREADY a dependency (Cargo.toml — verify below); parse into
`serde_json::Value` (no new dependency, no new deserialize-struct file). Keep the existing error
style `crate::error::AutoInstallError::ValidationError(...)` — do NOT add a new error variant.
The signature change of `detect_primary_disk` IS part of this task; its one and only call site
is listed below.

## Background (verify before editing)

- Current behavior (scout-verified 2026-07-09): `detect_primary_disk(disk_info: &str)` string-
  matches lines containing `"disk"` and accepts only names starting with `nvme`/`sd` — it can
  return a RAID **member** and misses `/dev/md126` and QEMU's `/dev/vda` entirely.
- HARD RULE (restate to yourself before coding): `disk_device` is READ from the live target
  (`lsblk`), NEVER guessed from `/dev/sd*` conventions and NEVER given a hardcoded fallback —
  on U1, `/dev/sda` is an IMSM RAID *member*; the real volume is `/dev/md126`. If enumeration
  fails, the function ERRORS; it must not guess. This is why md arrays take precedence over
  plain disks in the selection rule below.
- HARD RULE: code-only task — never run against 172.16.2.30 ("the server") or len-serv-003;
  validation is unit tests with fixture JSON. Workers never push/PR/merge.
- The caller chain is test-safe: `local_install_command` returns early on the
  `SystemUtils::is_root()` check in test runs, so `create_local_installation_config` (and any
  `lsblk` subprocess you add inside it) is never reached by `cargo test` on a dev machine.
- `system_info.disk_info` (built by `investigation.rs` from `lsblk -a` text) is DISPLAY-ONLY
  after this change — do NOT edit `investigation.rs`; the detector gets its own JSON input.
- lsblk JSON quirk: with `-b`, `"size"` is bytes, but older lsblk versions emit it as a JSON
  **string** while newer ones emit a number — handle both.
- lsblk JSON shape: md arrays appear as `"children"` of EVERY member disk (type `raid0`/
  `raid1`/`raid10`…), so recursion + dedupe-by-name is required.

**Re-verify these anchors before editing** — line numbers drift, they are a starting point only.
Zero hits = STOP and report:

```bash
grep -n 'fn detect_primary_disk' src/cli/commands.rs   # expect: 1 hit ~line 635
grep -n 'detect_primary_disk(&system_info.disk_info)' src/cli/commands.rs   # expect: 1 hit ~line 600 — the ONLY call site, inside create_local_installation_config
grep -n 'fn create_local_installation_config' src/cli/commands.rs   # expect: 1 hit ~line 595
grep -n 'serde_json' Cargo.toml   # expect: 1 hit — already a dependency, add nothing
grep -n 'is_root' src/cli/commands.rs   # expect: >=1 hit — the early-return guard that keeps tests off the lsblk subprocess
```

## Step-by-step

1. Run the anchor greps. Zero hits → STOP and report.
2. Rewrite `detect_primary_disk` as a pure function of the JSON text (keep the name; change the
   parameter meaning to lsblk JSON):
   `fn detect_primary_disk(lsblk_json: &str) -> Result<String>`.
   - Parse with `serde_json::from_str::<serde_json::Value>`; on parse error return
     `Err(ValidationError(...))`.
   - Walk `json["blockdevices"]` AND, recursively, each device's `"children"` array (md arrays
     only appear as children of their member disks).
   - Classify each visited device by its `"type"` and `"name"` — exactly these rules:
     - EXCLUDE types `"loop"`, `"rom"`, and `"part"`.
     - **md candidate**: type starts with `"raid"` OR name starts with `"md"`. Dedupe by name —
       an IMSM array is listed under every member disk and must count once.
     - **disk candidate**: type == `"disk"` (this naturally covers `nvme*`, `sd*`, `vd*` — do
       NOT filter disks by name prefix anymore).
   - Size: accept `"size"` as u64 OR as a numeric string (`as_u64()` first, else
     `as_str().and_then(|s| s.parse().ok())`). Missing/unparsable size → treat as 0 but KEEP the
     candidate (conservative, non-disqualifying — never drop a device just because size didn't
     parse).
   - Selection: if ANY md candidate exists → return the largest md (never install to a RAID
     member); otherwise return the largest disk candidate.
   - Return `format!("/dev/{}", name)` (lsblk `NAME` has no `/dev/` prefix; if a name already
     starts with `/`, return it verbatim — defensive only).
   - No candidates → `Err(ValidationError("could not detect primary disk ..."))` — NEVER a
     hardcoded default device.
   - Delete the old text-matching loop entirely (this is a transform, not a parallel path).
3. Update the single call site in `create_local_installation_config`: run
   `std::process::Command::new("lsblk").args(["--json", "-b", "-o", "NAME,TYPE,SIZE"])` on the
   live system; on spawn error or non-zero exit status return
   `Err(ValidationError("cannot enumerate block devices on the live target — refusing to guess disk_device (use --config)"))`;
   on success pass `String::from_utf8_lossy(&output.stdout)` to `detect_primary_disk`. Do not
   touch anything else in that function (TASK-04 may add one initializer line to its struct
   literal in a later wave — leave the literal otherwise alone).
4. Purely-scoped change: do not modify `detect_network_config` (TASK-03 owns it, next wave), do
   not modify `investigation.rs`, do not reorder or rename anything else in `commands.rs`.
5. Add unit tests in the existing `mod tests` of `src/cli/commands.rs` with inline fixture JSON
   strings (test names are load-bearing for acceptance):
   - `test_detect_primary_disk_nvme` — one nvme0n1 disk → `/dev/nvme0n1`.
   - `test_detect_primary_disk_ignores_loop_and_rom` — sda (type disk) + loop0 + sr0 (rom) →
     `/dev/sda` (anti-over-suppression: the exclusion filter must not block real disks).
   - `test_detect_primary_disk_virtio` — vda, type disk → `/dev/vda`.
   - `test_detect_primary_disk_prefers_md_over_members` — sda and sdb EACH carrying child
     `{"name":"md126","type":"raid1",...}` → `/dev/md126` (dedupe: counted once; md beats the
     larger member disks).
   - `test_detect_primary_disk_picks_largest` — two disks, different `-b` sizes → the larger.
   - `test_detect_primary_disk_size_as_string` — `"size":"500107862016"` (string) parses.
   - `test_detect_primary_disk_empty_errors` — `{"blockdevices": []}` → `Err`.
6. Bump the `src/cli/commands.rs` file header (`version` + `last-edited`, keep `guid`). Run the
   gate, commit.

## How to test

```bash
cargo test --lib --offline    # Expected: 237+ passed; 0 failed (baseline 237 + the new detect tests)
cargo build --offline         # Expected: exit 0
cargo clippy --offline        # Expected: no new warnings
```

## Acceptance criteria

- [ ] `grep -n 'blockdevices' src/cli/commands.rs` → ≥1 hit (JSON parser present).
- [ ] `grep -n 'starts_with("nvme")' src/cli/commands.rs` → 0 hits (old text matcher deleted).
- [ ] `grep -c 'fn test_detect_primary_disk' src/cli/commands.rs` → 7 (all fixtures above).
- [ ] md precedence proven: `cargo test --lib --offline test_detect_primary_disk_prefers_md_over_members` passes.
- [ ] Anti-over-suppression: `cargo test --lib --offline test_detect_primary_disk_ignores_loop_and_rom`
  passes — a real disk still gets selected with the loop/rom exclusion filter active.
- [ ] Failure path never guesses: `test_detect_primary_disk_empty_errors` passes and
  `grep -n '"/dev/sda".to_string()\|"/dev/nvme0n1".to_string()' src/cli/commands.rs` shows no
  hardcoded fallback inside `detect_primary_disk` (the `for_len_serv_003` default in
  `config.rs` is a different, untouched file).
- [ ] Tests green: `cargo test --lib --offline` 237+ passed, 0 failed; `cargo build --offline`
  and `cargo clippy --offline` clean.
- [ ] File headers bumped (`grep -n 'last-edited:' src/cli/commands.rs` shows today's date; guid
  unchanged).

## Commit message

```
fix(cli): detect_primary_disk parses lsblk --json (md/nvme/sd/vd) instead of fragile text matching

The old detector string-matched lsblk text and only accepted nvme*/sd* names,
missing /dev/md126 (U1's real IMSM volume) and /dev/vda (QEMU virtio), and could
return a RAID member. Now parses `lsblk --json -b -o NAME,TYPE,SIZE` from the
live target with serde_json: excludes loop/rom, dedupes md arrays found under
every member disk, prefers md arrays over plain disks, and picks the largest
candidate. Enumeration failure is a hard error — the disk is never guessed.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

Already-done check (transform polarity — new parser present AND old matcher absent): if
`grep -n 'blockdevices' src/cli/commands.rs` hits AND
`grep -n 'starts_with("nvme")' src/cli/commands.rs` returns 0 hits, the task is already
applied — run the acceptance checks instead of re-applying. Rollback: `git revert` the single
commit — restores the text-matching detector and its caller; no data or on-disk state is
touched, siblings unaffected.
