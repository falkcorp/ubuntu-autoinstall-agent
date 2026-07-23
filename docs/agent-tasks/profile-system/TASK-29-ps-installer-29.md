<!-- file: docs/agent-tasks/profile-system/TASK-29-ps-installer-29.md -->
<!-- version: 1.0.0 -->
<!-- guid: 70218145-84c5-4f9c-84d0-24e3680673c3 -->
<!-- last-edited: 2026-07-23 -->

# TASK-29 — installer modules consume typed resolved fields (per-module, flat fallback) (PS-INSTALLER-29)

**Priority:** P3 · **Effort:** L · **Recommended subagent:** Opus-class · cross-cutting seam / migration touching merge-provenance, rollback safety, or a multi-module refactor · **Depends on:** PS-MIG-LEN-DISK-28 (TASK-28), PS-MIG-U1-23 (TASK-24)

**Wave:** 11 · **Workstream:** installer-refactor · **Role:** rust-architecture subagent (cross-cutting seam / migration / multi-module refactor)

> Part of the **Profile-System conversion** ([README](README.md), [design](../../specs/profile-system-design.md), [current-state](../../specs/profile-system-current-state.md)). Universal protocol + wave/collision rules live in the README — read it first.

## ⛔ START HERE (worktree setup — do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent
SLUG=ps-installer-29
git -C "$REPO" fetch origin
# Base on origin/main. If any "Depends on" brief has merged, its changes are already on main.
git -C "$REPO" worktree add "$REPO/.worktrees/ps-$SLUG" -b "agent/ps-$SLUG" origin/main
cd "$REPO/.worktrees/ps-$SLUG"
git rebase origin/main
```

## Goal

Now that every host is component-authored, make the installer action-modules read the typed axes and finally consume the fields that were inert until now (disk sizes/reset_enabled from disk_layout, tpm2_clevis_peer surrogate, fallback_mirror). Highest risk — do it PER MODULE, keeping the flat InstallationConfig field as a fallback so the wire artifact stays the lowered InstallationConfig and rollback-parse is preserved; flip a module only conceptually after the fleet installer binary is component-aware. Concretely: (a) disk_ops.rs reads esp/reset/bpool sizes + reset_enabled from the resolved disk plan instead of the hardcoded sgdisk literals (defaulting to today's 512M/4G/2G/enabled so byte-identical); (b) system_setup.rs continues to derive the D2-B peer from storage_mode==NativeKeystore (no change needed unless promoting tpm2_clevis_peer to a real field — if so, add it as a serialized skip-if-false wire field FIRST and update lower/gate, NOT a #[serde(skip)] flag per the SERIAL-18 lesson); (c) base-image fallback_mirror consumed where the old-releases URL is hardcoded. The equality gate (struct equality) must stay green after each module; the VM gate (scripts/vm-validate.sh on 172.16.2.30) must pass after the refactor. Any field the installer reads MUST be serialized — never #[serde(skip)]. Bump headers per module.

## Files (expected touch set)

- `crates/uaa-core/src/network/ssh_installer/disk_ops.rs`
- `crates/uaa-core/src/network/ssh_installer/disk_native.rs`
- `crates/uaa-core/src/network/ssh_installer/system_setup.rs`
- `crates/uaa-core/src/network/ssh_installer/config.rs`
- `crates/uaa-core/src/network/ssh_installer/installer.rs`

## Re-verify anchors before editing (line numbers/paths drift — grep first)

```bash
grep -n . crates/uaa-core/src/network/ssh_installer/disk_ops.rs >/dev/null && echo 'ok: crates/uaa-core/src/network/ssh_installer/disk_ops.rs' || echo 'MISSING (new or moved): crates/uaa-core/src/network/ssh_installer/disk_ops.rs'
grep -n . crates/uaa-core/src/network/ssh_installer/disk_native.rs >/dev/null && echo 'ok: crates/uaa-core/src/network/ssh_installer/disk_native.rs' || echo 'MISSING (new or moved): crates/uaa-core/src/network/ssh_installer/disk_native.rs'
grep -n . crates/uaa-core/src/network/ssh_installer/system_setup.rs >/dev/null && echo 'ok: crates/uaa-core/src/network/ssh_installer/system_setup.rs' || echo 'MISSING (new or moved): crates/uaa-core/src/network/ssh_installer/system_setup.rs'
grep -n . crates/uaa-core/src/network/ssh_installer/config.rs >/dev/null && echo 'ok: crates/uaa-core/src/network/ssh_installer/config.rs' || echo 'MISSING (new or moved): crates/uaa-core/src/network/ssh_installer/config.rs'
grep -n . crates/uaa-core/src/network/ssh_installer/installer.rs >/dev/null && echo 'ok: crates/uaa-core/src/network/ssh_installer/installer.rs' || echo 'MISSING (new or moved): crates/uaa-core/src/network/ssh_installer/installer.rs'
```
Zero-hit on a file you expected to edit = STOP and report (the code moved).

## Acceptance criteria

- [ ] each refactored module reads the typed field with the flat field retained as fallback (reviewed); defaults reproduce today's literals
- [ ] no field the installer reads is #[serde(skip)] (grep-verified)
- [ ] component_equality_gate stays green (no change to the lowered InstallationConfig wire artifact)
- [ ] VM gate passes on the server after the refactor (command recorded in PR)
- [ ] file-version headers bumped; cargo clippy -p uaa-core --all-targets clean
- [ ] `cargo test -p uaa-core -p uaa-control` green for touched crates
- [ ] `cargo clippy --workspace --all-targets` clean
- [ ] File-version headers bumped on **every** changed file (Rust: `// file:/version:/guid:/last-edited:` before `package`/first item; md/yaml/toml comments otherwise)
- [ ] len-serv PlainLuks path stays **byte-identical** unless this brief is an explicit len-serv migration (waves 7–10)

## Commit + PR

Conventional commit; end the body with the repo's Co-Authored-By / Claude-Session trailers.

```
refactor(profile): installer modules consume typed resolved fields (per-module, flat fallback) (PS-INSTALLER-29)
```

Then `gh pr create` → `gh pr merge <n> --rebase`. Clean up: `git -C "$REPO" worktree remove "$REPO/.worktrees/ps-$SLUG"`.
