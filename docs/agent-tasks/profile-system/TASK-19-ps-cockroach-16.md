<!-- file: docs/agent-tasks/profile-system/TASK-19-ps-cockroach-16.md -->
<!-- version: 1.0.0 -->
<!-- guid: fd361751-51ff-4ad4-80f8-b53d40634334 -->
<!-- last-edited: 2026-07-23 -->

# TASK-19 — derive cockroach advertise/join from group roster; retire LENSERV_MEMBER_IPS (PS-COCKROACH-16)

**Priority:** P1 · **Effort:** M · **Recommended subagent:** Sonnet-class · a self-contained component with its wiring and tests; bounded, moderate judgment · **Depends on:** PS-MERGE-13 (TASK-18)

**Wave:** 5 · **Workstream:** installer-slice · **Role:** rust-component subagent (implement a component + wiring + tests)

> Part of the **Profile-System conversion** ([README](README.md), [design](../../specs/profile-system-design.md), [current-state](../../specs/profile-system-current-state.md)). Universal protocol + wave/collision rules live in the README — read it first.

## ⛔ START HERE (worktree setup — do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent
SLUG=ps-cockroach-16
git -C "$REPO" fetch origin
# Base on origin/main. If any "Depends on" brief has merged, its changes are already on main.
git -C "$REPO" worktree add "$REPO/.worktrees/ps-$SLUG" -b "agent/ps-$SLUG" origin/main
cd "$REPO/.worktrees/ps-$SLUG"
git rebase origin/main
```

## Goal

Replace the hardcoded LENSERV_MEMBER_IPS constant (host_spec.rs) used at applications.rs:190-198 with a `derive_cockroach_endpoints(members:&[String], self_ip:&str) -> (advertise, join)` helper, and thread the member roster in. CROSS-CRATE DESIGN: uaa-control owns allocations (the roster); uaa-core owns the installer. Add `#[serde(default, skip_serializing_if="Vec::is_empty")] pub cockroach_members:Vec<String>` to InstallationConfig (config.rs) — skip-if-empty so non-cockroach hosts stay byte-identical — and have uaa-control's resolve path (profiles/resolve.rs) populate it from the active group allocation before returning the config. applications.rs derives advertise/join from cfg.cockroach_members instead of the constant. Gate the change with an equality assertion: the derived (advertise,join) for the 3 len-serv nodes equals the former constant's output; bake the expected join strings as literals from the documented values (host_spec.rs:96-121: each of .92/.94/.96 sees membership [.92,.94,.96] with self first). Address the for_lenserv cascade: refactor HostSpec::for_lenserv (host_spec.rs:66) to accept a members slice parameter (or call derive_cockroach_endpoints); update its live callers in uaa/cli/commands.rs and the render/place/verify test callsites accordingly. Bump headers on all edited files.

## Files (expected touch set)

- `crates/uaa-core/src/network/ssh_installer/applications.rs`
- `crates/uaa-core/src/network/ssh_installer/config.rs`
- `crates/uaa-core/src/autoinstall/host_spec.rs`
- `crates/uaa-control/src/profiles/resolve.rs`
- `crates/uaa/src/cli/commands.rs`

## Re-verify anchors before editing (line numbers/paths drift — grep first)

```bash
grep -n . crates/uaa-core/src/network/ssh_installer/applications.rs >/dev/null && echo 'ok: crates/uaa-core/src/network/ssh_installer/applications.rs' || echo 'MISSING (new or moved): crates/uaa-core/src/network/ssh_installer/applications.rs'
grep -n . crates/uaa-core/src/network/ssh_installer/config.rs >/dev/null && echo 'ok: crates/uaa-core/src/network/ssh_installer/config.rs' || echo 'MISSING (new or moved): crates/uaa-core/src/network/ssh_installer/config.rs'
grep -n . crates/uaa-core/src/autoinstall/host_spec.rs >/dev/null && echo 'ok: crates/uaa-core/src/autoinstall/host_spec.rs' || echo 'MISSING (new or moved): crates/uaa-core/src/autoinstall/host_spec.rs'
grep -n . crates/uaa-control/src/profiles/resolve.rs >/dev/null && echo 'ok: crates/uaa-control/src/profiles/resolve.rs' || echo 'MISSING (new or moved): crates/uaa-control/src/profiles/resolve.rs'
grep -n . crates/uaa/src/cli/commands.rs >/dev/null && echo 'ok: crates/uaa/src/cli/commands.rs' || echo 'MISSING (new or moved): crates/uaa/src/cli/commands.rs'
```
Zero-hit on a file you expected to edit = STOP and report (the code moved).

## Acceptance criteria

- [ ] LENSERV_MEMBER_IPS constant removed (grep clean)
- [ ] test: derive_cockroach_endpoints for the 3 len-serv nodes == the former constant output (join literals asserted)
- [ ] cockroach_members is skip-if-empty; a non-cockroach host serializes byte-identically (test)
- [ ] for_lenserv refactored to take members; live callers updated; cargo test -p uaa-core and -p uaa pass
- [ ] file-version headers bumped; cargo clippy --all-targets clean
- [ ] `cargo test -p uaa-core -p uaa-control` green for touched crates
- [ ] `cargo clippy --workspace --all-targets` clean
- [ ] File-version headers bumped on **every** changed file (Rust: `// file:/version:/guid:/last-edited:` before `package`/first item; md/yaml/toml comments otherwise)
- [ ] len-serv PlainLuks path stays **byte-identical** unless this brief is an explicit len-serv migration (waves 7–10)

## Commit + PR

Conventional commit; end the body with the repo's Co-Authored-By / Claude-Session trailers.

```
feat(profile): derive cockroach advertise/join from group roster; retire lenserv_member_ips (PS-COCKROACH-16)
```

Then `gh pr create` → `gh pr merge <n> --rebase`. Clean up: `git -C "$REPO" worktree remove "$REPO/.worktrees/ps-$SLUG"`.
