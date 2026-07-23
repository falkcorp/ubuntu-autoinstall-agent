<!-- file: docs/agent-tasks/profile-system/TASK-03-ps-disk-01.md -->
<!-- version: 1.0.0 -->
<!-- guid: cb3c5106-09b6-4482-ad2b-9d0ca1cf7819 -->
<!-- last-edited: 2026-07-23 -->

# TASK-03 — disk-layout component types + per-variant partials (PS-DISK-01)

**Priority:** P1 · **Effort:** M · **Recommended subagent:** Sonnet-class · a self-contained component with its wiring and tests; bounded, moderate judgment · **Depends on:** none

**Wave:** 1 · **Workstream:** authoring-types · **Role:** rust-component subagent (implement a component + wiring + tests)

> Part of the **Profile-System conversion** ([README](README.md), [design](../../specs/profile-system-design.md), [current-state](../../specs/profile-system-current-state.md)). Universal protocol + wave/collision rules live in the README — read it first.

## ⛔ START HERE (worktree setup — do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent
SLUG=ps-disk-01
git -C "$REPO" fetch origin
# Base on origin/main. If any "Depends on" brief has merged, its changes are already on main.
git -C "$REPO" worktree add "$REPO/.worktrees/ps-$SLUG" -b "agent/ps-$SLUG" origin/main
cd "$REPO/.worktrees/ps-$SLUG"
git rebase origin/main
```

## Goal

Create NEW module crates/uaa-core/src/network/ssh_installer/components/disk_layout.rs plus a NEW components/mod.rs, and append `pub mod components;` to crates/uaa-core/src/network/ssh_installer/mod.rs (append-only, no other brief touches that line this wave). Define a tagged enum `DiskLayout { SingleLuks(SingleLuksSpec), ZfsNativeKeystore(NativeKeystoreSpec) }` with `#[serde(tag="kind", rename_all="kebab-case", deny_unknown_fields)]`, mirroring the ApplicationSpec newtype pattern at config.rs:46. SingleLuksSpec fields are ALL String sgdisk-suffix literals: `esp_size:String` default "512M", `reset_size:String` default "4G", `bpool_size:String` default "2G" (matching the disk_ops.rs sgdisk strings at lines 314/324/334 exactly), `disk_device:Option<String>`, and `reset_enabled:bool` default true. NativeKeystoreSpec carries `disks:Vec<DiskSpec>` reusing DiskSpec/DiskRole verbatim from config.rs. Add per-variant partials SingleLuksSpecPartial and NativeKeystoreSpecPartial modeled literally on CockroachSpecPartial (mod.rs:175): every field Option-wrapped, `#[derive(Debug,Clone,Default,PartialEq,Serialize,Deserialize)]` with `#[serde(deny_unknown_fields, default)]`. These sizes/reset_enabled are authoring-expressibility only and are NOT wire fields — lower() will drop them (they stay inert until PS-INSTALLER-29 wires disk_ops to read them). Do NOT wire anything onto InstallationConfig/InstallationConfigPartial and add no appliers. New files start at version 1.0.0.

## Files (expected touch set)

- `crates/uaa-core/src/network/ssh_installer/components/disk_layout.rs`
- `crates/uaa-core/src/network/ssh_installer/components/mod.rs`
- `crates/uaa-core/src/network/ssh_installer/mod.rs`

## Re-verify anchors before editing (line numbers/paths drift — grep first)

```bash
grep -n . crates/uaa-core/src/network/ssh_installer/components/disk_layout.rs >/dev/null && echo 'ok: crates/uaa-core/src/network/ssh_installer/components/disk_layout.rs' || echo 'MISSING (new or moved): crates/uaa-core/src/network/ssh_installer/components/disk_layout.rs'
grep -n . crates/uaa-core/src/network/ssh_installer/components/mod.rs >/dev/null && echo 'ok: crates/uaa-core/src/network/ssh_installer/components/mod.rs' || echo 'MISSING (new or moved): crates/uaa-core/src/network/ssh_installer/components/mod.rs'
grep -n . crates/uaa-core/src/network/ssh_installer/mod.rs >/dev/null && echo 'ok: crates/uaa-core/src/network/ssh_installer/mod.rs' || echo 'MISSING (new or moved): crates/uaa-core/src/network/ssh_installer/mod.rs'
```
Zero-hit on a file you expected to edit = STOP and report (the code moved).

## Acceptance criteria

- [ ] cargo test -p uaa-core --lib passes new serde round-trip unit tests for both variants and both partials (N new tests visible in the count)
- [ ] DiskLayout is tag="kind" kebab-case deny_unknown_fields; an unknown kind fails to deserialize (test asserts Err)
- [ ] default esp_size/reset_size/bpool_size are the String literals "512M"/"4G"/"2G" matching disk_ops.rs:314/324/334 exactly (asserted in a test)
- [ ] file-version headers present on every new/edited file; new files at 1.0.0
- [ ] cargo clippy -p uaa-core --all-targets returns no warnings
- [ ] `cargo test -p uaa-core -p uaa-control` green for touched crates
- [ ] `cargo clippy --workspace --all-targets` clean
- [ ] File-version headers bumped on **every** changed file (Rust: `// file:/version:/guid:/last-edited:` before `package`/first item; md/yaml/toml comments otherwise)
- [ ] len-serv PlainLuks path stays **byte-identical** unless this brief is an explicit len-serv migration (waves 7–10)

## Commit + PR

Conventional commit; end the body with the repo's Co-Authored-By / Claude-Session trailers.

```
feat(profile): disk-layout component types + per-variant partials (PS-DISK-01)
```

Then `gh pr create` → `gh pr merge <n> --rebase`. Clean up: `git -C "$REPO" worktree remove "$REPO/.worktrees/ps-$SLUG"`.
