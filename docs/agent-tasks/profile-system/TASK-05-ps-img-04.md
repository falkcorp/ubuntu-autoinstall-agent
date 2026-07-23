<!-- file: docs/agent-tasks/profile-system/TASK-05-ps-img-04.md -->
<!-- version: 1.0.0 -->
<!-- guid: 102b18f9-fef5-4fa3-a4f0-ccb354e1c3c7 -->
<!-- last-edited: 2026-07-23 -->

# TASK-05 — base-image authoring sub-struct (BaseImagePartial) (PS-IMG-04)

**Priority:** P1 · **Effort:** S · **Recommended subagent:** Haiku-class · mechanical, additive — a single new types-only module or enum; no cross-cutting logic · **Depends on:** none

**Wave:** 1 · **Workstream:** authoring-types · **Role:** types/enum authoring subagent (scan + one new module file)

> Part of the **Profile-System conversion** ([README](README.md), [design](../../specs/profile-system-design.md), [current-state](../../specs/profile-system-current-state.md)). Universal protocol + wave/collision rules live in the README — read it first.

## ⛔ START HERE (worktree setup — do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent
SLUG=ps-img-04
git -C "$REPO" fetch origin
# Base on origin/main. If any "Depends on" brief has merged, its changes are already on main.
git -C "$REPO" worktree add "$REPO/.worktrees/ps-$SLUG" -b "agent/ps-$SLUG" origin/main
cd "$REPO/.worktrees/ps-$SLUG"
git rebase origin/main
```

## Goal

Create NEW module crates/uaa-core/src/profile/components/base_image.rs defining `pub struct BaseImagePartial { #[serde(default, skip_serializing_if="Option::is_none", deserialize_with="super::super::deserialize_double_option")] release:Option<Option<String>>, #[serde(default, skip_serializing_if="Option::is_none", deserialize_with="super::super::deserialize_double_option")] mirror:Option<Option<String>>, initramfs:Option<InitramfsType>, fallback_mirror:Option<String> }` with `#[derive(Debug,Clone,Default,PartialEq,Serialize,Deserialize)] #[serde(deny_unknown_fields, default)]`. Copy the exact triple serde attribute for the double-Option fields from mod.rs:55-59 (default + skip_serializing_if=Option::is_none + deserialize_with) so absent-vs-null stays distinct. Reuse the existing `InitramfsType` enum from ssh_installer::config (keep its regenerate_cmd(); do NOT define a duplicate). Append `pub mod base_image;` to crates/uaa-core/src/profile/components/mod.rs (create that re-export file if racing PS-UNLOCK-02). fallback_mirror is Option<String> surfacing the hardcoded old-releases URL (http://old-releases.ubuntu.com/ubuntu/) as authoring-expressibility ONLY — it is inert (lower() drops it) until an installer brief reads it, same category as disk sizes. Document mapping for PS-LOWER-12: release->debootstrap_release, mirror->debootstrap_mirror, initramfs->initramfs_type. No wiring onto InstallationConfigPartial, no merge/lower. New file at 1.0.0.

## Files (expected touch set)

- `crates/uaa-core/src/profile/components/base_image.rs`
- `crates/uaa-core/src/profile/components/mod.rs`

## Re-verify anchors before editing (line numbers/paths drift — grep first)

```bash
grep -n . crates/uaa-core/src/profile/components/base_image.rs >/dev/null && echo 'ok: crates/uaa-core/src/profile/components/base_image.rs' || echo 'MISSING (new or moved): crates/uaa-core/src/profile/components/base_image.rs'
grep -n . crates/uaa-core/src/profile/components/mod.rs >/dev/null && echo 'ok: crates/uaa-core/src/profile/components/mod.rs' || echo 'MISSING (new or moved): crates/uaa-core/src/profile/components/mod.rs'
```
Zero-hit on a file you expected to edit = STOP and report (the code moved).

## Acceptance criteria

- [ ] serde round-trip test for a fully-populated BaseImagePartial passes
- [ ] release double-Option distinctness test (absent {} vs {"release":null} vs {"release":"jammy"}) asserts three distinct states
- [ ] reuses existing InitramfsType (grep shows no duplicate enum) and super::super::deserialize_double_option
- [ ] file-version header present; new file at 1.0.0; cargo clippy -p uaa-core --all-targets clean
- [ ] `cargo test -p uaa-core -p uaa-control` green for touched crates
- [ ] `cargo clippy --workspace --all-targets` clean
- [ ] File-version headers bumped on **every** changed file (Rust: `// file:/version:/guid:/last-edited:` before `package`/first item; md/yaml/toml comments otherwise)
- [ ] len-serv PlainLuks path stays **byte-identical** unless this brief is an explicit len-serv migration (waves 7–10)

## Commit + PR

Conventional commit; end the body with the repo's Co-Authored-By / Claude-Session trailers.

```
feat(profile): base-image authoring sub-struct (baseimagepartial) (PS-IMG-04)
```

Then `gh pr create` → `gh pr merge <n> --rebase`. Clean up: `git -C "$REPO" worktree remove "$REPO/.worktrees/ps-$SLUG"`.
