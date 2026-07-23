<!-- file: docs/agent-tasks/profile-system/TASK-02-ps-arch-07.md -->
<!-- version: 1.0.0 -->
<!-- guid: f6a5790d-8561-4b32-9bad-c5348cd839d1 -->
<!-- last-edited: 2026-07-23 -->

# TASK-02 — Arch classifier enum (NEW, in ssh_installer::config) (PS-ARCH-07)

**Priority:** P1 · **Effort:** S · **Recommended subagent:** Haiku-class · mechanical, additive — a single new types-only module or enum; no cross-cutting logic · **Depends on:** none

**Wave:** 1 · **Workstream:** authoring-types · **Role:** types/enum authoring subagent (scan + one new module file)

> Part of the **Profile-System conversion** ([README](README.md), [design](../../specs/profile-system-design.md), [current-state](../../specs/profile-system-current-state.md)). Universal protocol + wave/collision rules live in the README — read it first.

## ⛔ START HERE (worktree setup — do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent
SLUG=ps-arch-07
git -C "$REPO" fetch origin
# Base on origin/main. If any "Depends on" brief has merged, its changes are already on main.
git -C "$REPO" worktree add "$REPO/.worktrees/ps-$SLUG" -b "agent/ps-$SLUG" origin/main
cd "$REPO/.worktrees/ps-$SLUG"
git rebase origin/main
```

## Goal

Define a NEW enum `Arch { Amd64, Arm64 }` in crates/uaa-core/src/network/ssh_installer/config.rs with `#[derive(Debug,Clone,Copy,PartialEq,Eq,Serialize,Deserialize,Default)] #[serde(rename_all="kebab-case")]`, `#[default] Amd64`. This is deliberately NOT crate::config::Architecture (that belongs to the retired TargetConfig/image pipeline). Provide `pub fn is_amd64(&self) -> bool { matches!(self, Arch::Amd64) }` for use in a later `#[serde(skip_serializing_if="Arch::is_amd64")]` (mirror the StorageMode::is_default precedent at config.rs:192-194). Do NOT add the field to InstallationConfig yet (that is PS-WIRE-AXES-10). Type + default + helper + tests only. Bump config.rs header minor (adds a new module type).

## Files (expected touch set)

- `crates/uaa-core/src/network/ssh_installer/config.rs`

## Re-verify anchors before editing (line numbers/paths drift — grep first)

```bash
grep -n . crates/uaa-core/src/network/ssh_installer/config.rs >/dev/null && echo 'ok: crates/uaa-core/src/network/ssh_installer/config.rs' || echo 'MISSING (new or moved): crates/uaa-core/src/network/ssh_installer/config.rs'
```
Zero-hit on a file you expected to edit = STOP and report (the code moved).

## Acceptance criteria

- [ ] Arch::default() == Amd64 (asserted)
- [ ] is_amd64 tested true for Amd64, false for Arm64
- [ ] serde round-trip asserts exact wire strings: serde_yaml::to_string(&Arch::Amd64).trim()=="amd64" and Arm64->"arm64"
- [ ] file-version header on config.rs bumped (minor); cargo clippy -p uaa-core --all-targets clean
- [ ] `cargo test -p uaa-core -p uaa-control` green for touched crates
- [ ] `cargo clippy --workspace --all-targets` clean
- [ ] File-version headers bumped on **every** changed file (Rust: `// file:/version:/guid:/last-edited:` before `package`/first item; md/yaml/toml comments otherwise)
- [ ] len-serv PlainLuks path stays **byte-identical** unless this brief is an explicit len-serv migration (waves 7–10)

## Commit + PR

Conventional commit; end the body with the repo's Co-Authored-By / Claude-Session trailers.

```
feat(profile): arch classifier enum (new, in ssh_installer::config) (PS-ARCH-07)
```

Then `gh pr create` → `gh pr merge <n> --rebase`. Clean up: `git -C "$REPO" worktree remove "$REPO/.worktrees/ps-$SLUG"`.
