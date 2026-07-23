<!-- file: docs/agent-tasks/profile-system/TASK-16-ps-serial-18.md -->
<!-- version: 1.0.0 -->
<!-- guid: 8a7112f4-2e58-4fc8-93a8-218053de5f89 -->
<!-- last-edited: 2026-07-23 -->

# TASK-16 — serial-console as arch-gated installer default (serialization-safe) (PS-SERIAL-18)

**Priority:** P1 · **Effort:** M · **Recommended subagent:** Sonnet-class · a self-contained component with its wiring and tests; bounded, moderate judgment · **Depends on:** PS-WIRE-AXES-10 (TASK-12)

**Wave:** 3 · **Workstream:** installer-slice · **Role:** rust-component subagent (implement a component + wiring + tests)

> Part of the **Profile-System conversion** ([README](README.md), [design](../../specs/profile-system-design.md), [current-state](../../specs/profile-system-current-state.md)). Universal protocol + wave/collision rules live in the README — read it first.

## ⛔ START HERE (worktree setup — do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent
SLUG=ps-serial-18
git -C "$REPO" fetch origin
# Base on origin/main. If any "Depends on" brief has merged, its changes are already on main.
git -C "$REPO" worktree add "$REPO/.worktrees/ps-$SLUG" -b "agent/ps-$SLUG" origin/main
cd "$REPO/.worktrees/ps-$SLUG"
git rebase origin/main
```

## Goal

CORRECTED per advisor: do NOT use a #[serde(skip)] flag — a skip field does not survive `config place` -> installer-reads-serialized-YAML on the target (it deserializes back to default, so serial-console would silently never apply). Instead gate on the REAL serialized field config.arch (added by PS-WIRE-AXES-10). Today configure_serial_console (system_setup.rs:183) is called unconditionally from configure_grub_in_chroot (system_setup.rs:671). Change: pass config into configure_serial_console (or add the guard at the call site) so it runs ONLY when config.arch==Arch::Amd64, and is skipped for Arm64. Because arch is skip-if-amd64 (WIRE-AXES-10), every committed amd64 host omits the key, deserializes back to Amd64, and still gets serial-console -> placed artifact byte-identical. arm64 hosts serialize arch=arm64 and skip serial-console. This makes serial-console an arch-implied default that NEVER appears in firmware_quirks (so the empty-vec byte-identical claim holds). Bump system_setup.rs header.

## Files (expected touch set)

- `crates/uaa-core/src/network/ssh_installer/system_setup.rs`

## Re-verify anchors before editing (line numbers/paths drift — grep first)

```bash
grep -n . crates/uaa-core/src/network/ssh_installer/system_setup.rs >/dev/null && echo 'ok: crates/uaa-core/src/network/ssh_installer/system_setup.rs' || echo 'MISSING (new or moved): crates/uaa-core/src/network/ssh_installer/system_setup.rs'
```
Zero-hit on a file you expected to edit = STOP and report (the code moved).

## Acceptance criteria

- [ ] test: an amd64 InstallationConfig (arch defaulted) causes configure_serial_console to run; an arm64 config skips it
- [ ] firmware_quirks vec remains empty/omitted for all committed hosts (never used for serial-console)
- [ ] the equality gate test_resolved_equals_committed_by_struct_equality (crates/uaa/src/cli/config.rs:461) still passes — len-serv/unimatrixone placed artifact byte-identical
- [ ] file-version header bumped; cargo clippy -p uaa-core --all-targets clean
- [ ] `cargo test -p uaa-core -p uaa-control` green for touched crates
- [ ] `cargo clippy --workspace --all-targets` clean
- [ ] File-version headers bumped on **every** changed file (Rust: `// file:/version:/guid:/last-edited:` before `package`/first item; md/yaml/toml comments otherwise)
- [ ] len-serv PlainLuks path stays **byte-identical** unless this brief is an explicit len-serv migration (waves 7–10)

## Commit + PR

Conventional commit; end the body with the repo's Co-Authored-By / Claude-Session trailers.

```
feat(profile): serial-console as arch-gated installer default (serialization-safe) (PS-SERIAL-18)
```

Then `gh pr create` → `gh pr merge <n> --rebase`. Clean up: `git -C "$REPO" worktree remove "$REPO/.worktrees/ps-$SLUG"`.
