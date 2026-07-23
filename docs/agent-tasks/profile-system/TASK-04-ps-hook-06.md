<!-- file: docs/agent-tasks/profile-system/TASK-04-ps-hook-06.md -->
<!-- version: 1.0.0 -->
<!-- guid: db110f3a-d40d-4b10-adfd-15267805c93d -->
<!-- last-edited: 2026-07-23 -->

# TASK-04 — hooks types: new Phase enum + Hooks + HookStep (PS-HOOK-06)

**Priority:** P1 · **Effort:** M · **Recommended subagent:** Sonnet-class · a self-contained component with its wiring and tests; bounded, moderate judgment · **Depends on:** none

**Wave:** 1 · **Workstream:** authoring-types · **Role:** rust-component subagent (implement a component + wiring + tests)

> Part of the **Profile-System conversion** ([README](README.md), [design](../../specs/profile-system-design.md), [current-state](../../specs/profile-system-current-state.md)). Universal protocol + wave/collision rules live in the README — read it first.

## ⛔ START HERE (worktree setup — do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent
SLUG=ps-hook-06
git -C "$REPO" fetch origin
# Base on origin/main. If any "Depends on" brief has merged, its changes are already on main.
git -C "$REPO" worktree add "$REPO/.worktrees/ps-$SLUG" -b "agent/ps-$SLUG" origin/main
cd "$REPO/.worktrees/ps-$SLUG"
git rebase origin/main
```

## Goal

Create NEW module crates/uaa-core/src/network/ssh_installer/components/hooks.rs. Define `pub enum Phase { SetupVariables, PackageInstall, DiskPreparation, ZfsCreation, BaseSystem, SystemConfiguration, FinalSetup }` with `#[derive(Debug,Clone,Copy,Eq,PartialEq,Ord,PartialOrd,Hash,Serialize,Deserialize)] #[serde(rename_all="kebab-case")]`, its 7 variants mapped 1:1 to the seven run_phase! labels at installer.rs:295-342 (put a doc comment, one line per variant, naming the corresponding label). Do NOT reuse PhaseSelection (installer.rs:41) — it is a private-field bool-array struct and cannot key a map. Define `pub struct HookStep { pub run:String, pub chroot:bool }` where `chroot:true` means the command runs inside the target chroot and `false` means it runs on the live ISO/host; and `pub struct Hooks { #[serde(default, skip_serializing_if="BTreeMap::is_empty")] pub pre_phase:BTreeMap<Phase,Vec<HookStep>>, #[serde(default, skip_serializing_if="BTreeMap::is_empty")] pub post_phase:BTreeMap<Phase,Vec<HookStep>> }`. HookStep and Hooks `#[derive(Debug,Clone,Default,PartialEq,Serialize,Deserialize)]`. All three types `pub`. `run` is stored as-is (no validation). Append `pub mod hooks;` to components/mod.rs (created by PS-DISK-01). No installer wiring, no execution — types only. New file at 1.0.0.

## Files (expected touch set)

- `crates/uaa-core/src/network/ssh_installer/components/hooks.rs`
- `crates/uaa-core/src/network/ssh_installer/components/mod.rs`

## Re-verify anchors before editing (line numbers/paths drift — grep first)

```bash
grep -n . crates/uaa-core/src/network/ssh_installer/components/hooks.rs >/dev/null && echo 'ok: crates/uaa-core/src/network/ssh_installer/components/hooks.rs' || echo 'MISSING (new or moved): crates/uaa-core/src/network/ssh_installer/components/hooks.rs'
grep -n . crates/uaa-core/src/network/ssh_installer/components/mod.rs >/dev/null && echo 'ok: crates/uaa-core/src/network/ssh_installer/components/mod.rs' || echo 'MISSING (new or moved): crates/uaa-core/src/network/ssh_installer/components/mod.rs'
```
Zero-hit on a file you expected to edit = STOP and report (the code moved).

## Acceptance criteria

- [ ] Phase has exactly 7 variants aligned to installer.rs:295-342 (documented in a doc comment) and derives Ord+Hash (compiles as a BTreeMap key)
- [ ] test: default Hooks serializes with both maps omitted; a Hooks with only pre_phase populated omits post_phase; a both-populated Hooks round-trips without loss
- [ ] cargo test -p uaa-core --lib passes with the new tests visible in the count
- [ ] file-version header present; new file at 1.0.0; cargo clippy -p uaa-core --all-targets clean
- [ ] `cargo test -p uaa-core -p uaa-control` green for touched crates
- [ ] `cargo clippy --workspace --all-targets` clean
- [ ] File-version headers bumped on **every** changed file (Rust: `// file:/version:/guid:/last-edited:` before `package`/first item; md/yaml/toml comments otherwise)
- [ ] len-serv PlainLuks path stays **byte-identical** unless this brief is an explicit len-serv migration (waves 7–10)

## Commit + PR

Conventional commit; end the body with the repo's Co-Authored-By / Claude-Session trailers.

```
feat(profile): hooks types: new phase enum + hooks + hookstep (PS-HOOK-06)
```

Then `gh pr create` → `gh pr merge <n> --rebase`. Clean up: `git -C "$REPO" worktree remove "$REPO/.worktrees/ps-$SLUG"`.
