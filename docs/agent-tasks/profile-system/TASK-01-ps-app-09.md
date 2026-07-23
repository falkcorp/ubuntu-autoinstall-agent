<!-- file: docs/agent-tasks/profile-system/TASK-01-ps-app-09.md -->
<!-- version: 1.0.0 -->
<!-- guid: 11798769-20c6-42d9-b88c-58bd68d9ee83 -->
<!-- last-edited: 2026-07-23 -->

# TASK-01 — add TangServer variant to ApplicationSpec (PS-APP-09)

**Priority:** P1 · **Effort:** M · **Recommended subagent:** Sonnet-class · a self-contained component with its wiring and tests; bounded, moderate judgment · **Depends on:** none

**Wave:** 1 · **Workstream:** authoring-types · **Role:** rust-component subagent (implement a component + wiring + tests)

> Part of the **Profile-System conversion** ([README](README.md), [design](../../specs/profile-system-design.md), [current-state](../../specs/profile-system-current-state.md)). Universal protocol + wave/collision rules live in the README — read it first.

## ⛔ START HERE (worktree setup — do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent
SLUG=ps-app-09
git -C "$REPO" fetch origin
# Base on origin/main. If any "Depends on" brief has merged, its changes are already on main.
git -C "$REPO" worktree add "$REPO/.worktrees/ps-$SLUG" -b "agent/ps-$SLUG" origin/main
cd "$REPO/.worktrees/ps-$SLUG"
git rebase origin/main
```

## Goal

Extend the closed ApplicationSpec enum in config.rs (currently only Cockroach, at config.rs:46) with a `TangServer(TangServerSpec)` variant, preserving `tag="kind", rename_all="kebab-case", deny_unknown_fields`. Define `pub struct TangServerSpec { #[serde(default="default_tang_port")] port:u16, key_directory:String }` with `#[derive(Debug,Clone,PartialEq,Serialize,Deserialize)] #[serde(deny_unknown_fields)]` and a `fn default_tang_port()->u16 { 80 }` (fleet Tang binds port 80; key_directory is required, e.g. /etc/tang/keys). In applications.rs, the ApplicationInstaller dispatch must handle the new variant by returning `Ok(())` after `tracing::warn!("TangServer application authored but installer not implemented (host={}) — skipping", hostname)` — a no-op skip, NOT an error, NOT a panic (rpi is expressibility-only; no applier this brief). Duplicate policy: a config with two TangServer apps is a config error — extend reject_duplicates (applications.rs:268) to forbid duplicate `tang-server` keys just as it does for cockroach. Do not disturb existing Cockroach handling. New/edited files bump minor.

## Files (expected touch set)

- `crates/uaa-core/src/network/ssh_installer/config.rs`
- `crates/uaa-core/src/network/ssh_installer/applications.rs`

## Re-verify anchors before editing (line numbers/paths drift — grep first)

```bash
grep -n . crates/uaa-core/src/network/ssh_installer/config.rs >/dev/null && echo 'ok: crates/uaa-core/src/network/ssh_installer/config.rs' || echo 'MISSING (new or moved): crates/uaa-core/src/network/ssh_installer/config.rs'
grep -n . crates/uaa-core/src/network/ssh_installer/applications.rs >/dev/null && echo 'ok: crates/uaa-core/src/network/ssh_installer/applications.rs' || echo 'MISSING (new or moved): crates/uaa-core/src/network/ssh_installer/applications.rs'
```
Zero-hit on a file you expected to edit = STOP and report (the code moved).

## Acceptance criteria

- [ ] ApplicationSpec::TangServer round-trips from YAML `applications: [{kind: tang-server, port: 80, key-directory: /etc/tang/keys}]` (test)
- [ ] existing test_app_free_host_omits_applications_key still passes
- [ ] ApplicationInstaller dispatch returns Ok(()) with a warn log for TangServer (tested, no panic)
- [ ] reject_duplicates forbids two tang-server apps (test asserts Err); two cockroach still forbidden
- [ ] file-version headers bumped; cargo clippy -p uaa-core --all-targets clean
- [ ] `cargo test -p uaa-core -p uaa-control` green for touched crates
- [ ] `cargo clippy --workspace --all-targets` clean
- [ ] File-version headers bumped on **every** changed file (Rust: `// file:/version:/guid:/last-edited:` before `package`/first item; md/yaml/toml comments otherwise)
- [ ] len-serv PlainLuks path stays **byte-identical** unless this brief is an explicit len-serv migration (waves 7–10)

## Commit + PR

Conventional commit; end the body with the repo's Co-Authored-By / Claude-Session trailers.

```
feat(profile): add tangserver variant to applicationspec (PS-APP-09)
```

Then `gh pr create` → `gh pr merge <n> --rebase`. Clean up: `git -C "$REPO" worktree remove "$REPO/.worktrees/ps-$SLUG"`.
