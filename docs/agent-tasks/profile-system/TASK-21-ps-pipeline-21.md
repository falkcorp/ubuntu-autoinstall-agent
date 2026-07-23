<!-- file: docs/agent-tasks/profile-system/TASK-21-ps-pipeline-21.md -->
<!-- version: 1.0.0 -->
<!-- guid: 44b2dc94-faed-4dc4-8802-d1ffa37c6942 -->
<!-- last-edited: 2026-07-23 -->

# TASK-21 — wire validate_resolved into resolve path + prove component fixture resolves (PS-PIPELINE-21)

**Priority:** P1 · **Effort:** M · **Recommended subagent:** Sonnet-class · a self-contained component with its wiring and tests; bounded, moderate judgment · **Depends on:** PS-MERGE-13 (TASK-18), PS-SCHEMA-20 (TASK-15), PS-VALIDATE-14 (TASK-17)

**Wave:** 5 · **Workstream:** control-plane · **Role:** rust-component subagent (implement a component + wiring + tests)

> Part of the **Profile-System conversion** ([README](README.md), [design](../../specs/profile-system-design.md), [current-state](../../specs/profile-system-current-state.md)). Universal protocol + wave/collision rules live in the README — read it first.

## ⛔ START HERE (worktree setup — do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent
SLUG=ps-pipeline-21
git -C "$REPO" fetch origin
# Base on origin/main. If any "Depends on" brief has merged, its changes are already on main.
git -C "$REPO" worktree add "$REPO/.worktrees/ps-$SLUG" -b "agent/ps-$SLUG" origin/main
cd "$REPO/.worktrees/ps-$SLUG"
git rebase origin/main
```

## Goal

REFRAMED per advisor: merge() now internally lowers (PS-MERGE-13 keeps the (InstallationConfig, Provenance) signature), so there is NO separate lower() insertion — do NOT add lower() calls (that would double-lower). Instead: (1) insert `uaa_core::profile::validate::validate_resolved(&cfg)?` in resolve_from_registry (resolve.rs) after BOTH merge() call sites (resolve.rs:81 indexed path and resolve.rs:98 hostname_override path) and before returning the config — a single shared helper called on the merged result at both sites is fine; do NOT touch config_place or resolve_all_and_place (the CLI delegates to resolve_from_registry at cli/config.rs:268, so it inherits the change). (2) Add a test proving a component-authored group+host fixture resolves through resolve_from_registry into the expected flat InstallationConfig and passes validate_resolved; construct the fixture by registering a HostGroupProfile whose defaults carry a network/base_image component and a HostProfile override, mirroring the component_equality_gate fixtures from PS-GATE-15. (3) Regression test: an existing flat-authored group still resolves unchanged. Bump headers.

## Files (expected touch set)

- `crates/uaa-control/src/profiles/resolve.rs`

## Re-verify anchors before editing (line numbers/paths drift — grep first)

```bash
grep -n . crates/uaa-control/src/profiles/resolve.rs >/dev/null && echo 'ok: crates/uaa-control/src/profiles/resolve.rs' || echo 'MISSING (new or moved): crates/uaa-control/src/profiles/resolve.rs'
```
Zero-hit on a file you expected to edit = STOP and report (the code moved).

## Acceptance criteria

- [ ] validate_resolved is invoked after merge at both resolve.rs:81 and :98 sites (grep/test)
- [ ] a component-authored fixture resolves through resolve_from_registry to the expected flat InstallationConfig and passes validate_resolved (test)
- [ ] an illegal resolved config (e.g. NativeKeystore with empty disks) makes resolve_from_registry return Err (test)
- [ ] existing flat-authored resolution unchanged (regression test); the M2 gate still passes
- [ ] file-version header bumped; cargo clippy -p uaa-control --all-targets clean
- [ ] `cargo test -p uaa-core -p uaa-control` green for touched crates
- [ ] `cargo clippy --workspace --all-targets` clean
- [ ] File-version headers bumped on **every** changed file (Rust: `// file:/version:/guid:/last-edited:` before `package`/first item; md/yaml/toml comments otherwise)
- [ ] len-serv PlainLuks path stays **byte-identical** unless this brief is an explicit len-serv migration (waves 7–10)

## Commit + PR

Conventional commit; end the body with the repo's Co-Authored-By / Claude-Session trailers.

```
feat(profile): wire validate_resolved into resolve path + prove component fixture resolves (PS-PIPELINE-21)
```

Then `gh pr create` → `gh pr merge <n> --rebase`. Clean up: `git -C "$REPO" worktree remove "$REPO/.worktrees/ps-$SLUG"`.
