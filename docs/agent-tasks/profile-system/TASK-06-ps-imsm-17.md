<!-- file: docs/agent-tasks/profile-system/TASK-06-ps-imsm-17.md -->
<!-- version: 1.0.0 -->
<!-- guid: b3dcefb0-caa6-4405-8867-6feef4ec8609 -->
<!-- last-edited: 2026-07-23 -->

# TASK-06 — remove /dev/md IMSM sniffing (all call sites + tests) (PS-IMSM-17)

**Priority:** P1 · **Effort:** M · **Recommended subagent:** Sonnet-class · a self-contained component with its wiring and tests; bounded, moderate judgment · **Depends on:** none

**Wave:** 1 · **Workstream:** installer-cleanup · **Role:** rust-component subagent (implement a component + wiring + tests)

> Part of the **Profile-System conversion** ([README](README.md), [design](../../specs/profile-system-design.md), [current-state](../../specs/profile-system-current-state.md)). Universal protocol + wave/collision rules live in the README — read it first.

## ⛔ START HERE (worktree setup — do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent
SLUG=ps-imsm-17
git -C "$REPO" fetch origin
# Base on origin/main. If any "Depends on" brief has merged, its changes are already on main.
git -C "$REPO" worktree add "$REPO/.worktrees/ps-$SLUG" -b "agent/ps-$SLUG" origin/main
cd "$REPO/.worktrees/ps-$SLUG"
git rebase origin/main
```

## Goal

Remove the `disk_device.starts_with("/dev/md")` IMSM/mdraid detection everywhere. Call sites: disk_ops.rs assemble_md_if_needed; system_setup.rs mdadm_pkg selection and the is_md dracut mdraid-module + mdadm.conf write. DESIGN DECISION (hinge): remove the `include_mdraid` parameter from build_dracut_crypt_conf entirely (cleaner than passing constant false) and update its two call sites plus the two tests that call it (test_dracut_crypt_conf_includes_both_subsystems and test_dracut_crypt_conf_omits_clevis_and_mdraid_when_not_needed — rename/rescope them to drop the mdraid axis). Also handle the cascade the verifier found: installer.rs test_default_run_assembles_md_before_wiping (line ~1360) hard-fails at its .expect once assembly is gone — DELETE it; installer.rs test_default_run_skips_md_assemble_for_plain_disk (line ~1394) is vacuous after removal and its assert-negation trips a naive grep — DELETE it too. Update partitions.rs tests (lines ~31/46) and ssh.rs wrap_sudo test (lines ~550-551) that use /dev/md paths to use a plain nvme path. Fix the stale mdadm comment in layout.rs:35. LEAVE the unconditional mdadm apt package in packages.rs (harmless on non-md hosts; removing it is out of scope). Per settled architecture IMSM is dropped for native ZFS; len-serv are plain nvme0n1 and unimatrixone moves to native ZFS, so no committed host relies on md. Bump headers on every edited file.

## Files (expected touch set)

- `crates/uaa-core/src/network/ssh_installer/disk_ops.rs`
- `crates/uaa-core/src/network/ssh_installer/system_setup.rs`
- `crates/uaa-core/src/network/ssh_installer/installer.rs`
- `crates/uaa-core/src/network/ssh_installer/partitions.rs`
- `crates/uaa-core/src/network/ssh_installer/ssh.rs`
- `crates/uaa-core/src/network/ssh_installer/layout.rs`

## Re-verify anchors before editing (line numbers/paths drift — grep first)

```bash
grep -n . crates/uaa-core/src/network/ssh_installer/disk_ops.rs >/dev/null && echo 'ok: crates/uaa-core/src/network/ssh_installer/disk_ops.rs' || echo 'MISSING (new or moved): crates/uaa-core/src/network/ssh_installer/disk_ops.rs'
grep -n . crates/uaa-core/src/network/ssh_installer/system_setup.rs >/dev/null && echo 'ok: crates/uaa-core/src/network/ssh_installer/system_setup.rs' || echo 'MISSING (new or moved): crates/uaa-core/src/network/ssh_installer/system_setup.rs'
grep -n . crates/uaa-core/src/network/ssh_installer/installer.rs >/dev/null && echo 'ok: crates/uaa-core/src/network/ssh_installer/installer.rs' || echo 'MISSING (new or moved): crates/uaa-core/src/network/ssh_installer/installer.rs'
grep -n . crates/uaa-core/src/network/ssh_installer/partitions.rs >/dev/null && echo 'ok: crates/uaa-core/src/network/ssh_installer/partitions.rs' || echo 'MISSING (new or moved): crates/uaa-core/src/network/ssh_installer/partitions.rs'
grep -n . crates/uaa-core/src/network/ssh_installer/ssh.rs >/dev/null && echo 'ok: crates/uaa-core/src/network/ssh_installer/ssh.rs' || echo 'MISSING (new or moved): crates/uaa-core/src/network/ssh_installer/ssh.rs'
grep -n . crates/uaa-core/src/network/ssh_installer/layout.rs >/dev/null && echo 'ok: crates/uaa-core/src/network/ssh_installer/layout.rs' || echo 'MISSING (new or moved): crates/uaa-core/src/network/ssh_installer/layout.rs'
```
Zero-hit on a file you expected to edit = STOP and report (the code moved).

## Acceptance criteria

- [ ] No remaining `starts_with("/dev/md")` sniffing call-site (grep clean for that pattern; the two deleted installer.rs tests are gone)
- [ ] build_dracut_crypt_conf no longer takes include_mdraid; its 2 call sites + 2 tests compile and pass
- [ ] cargo test -p uaa-core passes (md-path tests deleted or repointed to nvme)
- [ ] len-serv PlainLuks path behavior unchanged for non-md devices
- [ ] file-version headers bumped; cargo clippy -p uaa-core --all-targets clean
- [ ] `cargo test -p uaa-core -p uaa-control` green for touched crates
- [ ] `cargo clippy --workspace --all-targets` clean
- [ ] File-version headers bumped on **every** changed file (Rust: `// file:/version:/guid:/last-edited:` before `package`/first item; md/yaml/toml comments otherwise)
- [ ] len-serv PlainLuks path stays **byte-identical** unless this brief is an explicit len-serv migration (waves 7–10)

## Commit + PR

Conventional commit; end the body with the repo's Co-Authored-By / Claude-Session trailers.

```
feat(profile): remove /dev/md imsm sniffing (all call sites + tests) (PS-IMSM-17)
```

Then `gh pr create` → `gh pr merge <n> --rebase`. Clean up: `git -C "$REPO" worktree remove "$REPO/.worktrees/ps-$SLUG"`.
