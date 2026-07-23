<!-- file: docs/agent-tasks/profile-system/TASK-09-ps-role-08.md -->
<!-- version: 1.0.0 -->
<!-- guid: eaa510d7-c88c-4e48-9011-ad57e01e4acd -->
<!-- last-edited: 2026-07-23 -->

# TASK-09 — HostRole classifier enum (PS-ROLE-08)

**Priority:** P1 · **Effort:** S · **Recommended subagent:** Haiku-class · mechanical, additive — a single new types-only module or enum; no cross-cutting logic · **Depends on:** none

**Wave:** 1 · **Workstream:** authoring-types · **Role:** types/enum authoring subagent (scan + one new module file)

> Part of the **Profile-System conversion** ([README](README.md), [design](../../specs/profile-system-design.md), [current-state](../../specs/profile-system-current-state.md)). Universal protocol + wave/collision rules live in the README — read it first.

## ⛔ START HERE (worktree setup — do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent
SLUG=ps-role-08
git -C "$REPO" fetch origin
# Base on origin/main. If any "Depends on" brief has merged, its changes are already on main.
git -C "$REPO" worktree add "$REPO/.worktrees/ps-$SLUG" -b "agent/ps-$SLUG" origin/main
cd "$REPO/.worktrees/ps-$SLUG"
git rebase origin/main
```

## Goal

Define a NEW enum `HostRole { InstallTarget, TangServer }` in crates/uaa-core/src/network/ssh_installer/config.rs with `#[derive(Debug,Clone,Copy,PartialEq,Eq,Serialize,Deserialize,Default)] #[serde(rename_all="kebab-case")]`, `#[default] InstallTarget`, and `pub fn is_install_target(&self) -> bool { matches!(self, HostRole::InstallTarget) }` for a later skip_serializing_if (mirror StorageMode::is_default at config.rs:192). Note the name collision: `HostRole::TangServer` is a namespaced variant, distinct from the existing `TangServer` struct at config.rs:36 — in tests that `use super::*`, an unqualified `TangServer` resolves to the struct, so match against the role with `HostRole::TangServer`. Do NOT add the field to InstallationConfig yet (PS-WIRE-AXES-10 does that). Type + default + helper + tests only. Bump config.rs header minor.

## Files (expected touch set)

- `crates/uaa-core/src/network/ssh_installer/config.rs`

## Re-verify anchors before editing (line numbers/paths drift — grep first)

```bash
grep -n . crates/uaa-core/src/network/ssh_installer/config.rs >/dev/null && echo 'ok: crates/uaa-core/src/network/ssh_installer/config.rs' || echo 'MISSING (new or moved): crates/uaa-core/src/network/ssh_installer/config.rs'
```
Zero-hit on a file you expected to edit = STOP and report (the code moved).

## Acceptance criteria

- [ ] HostRole::default() == InstallTarget (asserted)
- [ ] is_install_target tested both variants
- [ ] serde round-trip asserts exact wire strings "install-target" and "tang-server" appear in serialized YAML
- [ ] file-version header bumped (minor); cargo clippy -p uaa-core --all-targets clean
- [ ] `cargo test -p uaa-core -p uaa-control` green for touched crates
- [ ] `cargo clippy --workspace --all-targets` clean
- [ ] File-version headers bumped on **every** changed file (Rust: `// file:/version:/guid:/last-edited:` before `package`/first item; md/yaml/toml comments otherwise)
- [ ] len-serv PlainLuks path stays **byte-identical** unless this brief is an explicit len-serv migration (waves 7–10)

## Commit + PR

Conventional commit; end the body with the repo's Co-Authored-By / Claude-Session trailers.

```
feat(profile): hostrole classifier enum (PS-ROLE-08)
```

Then `gh pr create` → `gh pr merge <n> --rebase`. Clean up: `git -C "$REPO" worktree remove "$REPO/.worktrees/ps-$SLUG"`.
