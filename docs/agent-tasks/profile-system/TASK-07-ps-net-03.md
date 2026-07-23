<!-- file: docs/agent-tasks/profile-system/TASK-07-ps-net-03.md -->
<!-- version: 1.0.0 -->
<!-- guid: 1ac69f65-542b-4b08-a354-4c5443cd3fff -->
<!-- last-edited: 2026-07-23 -->

# TASK-07 — network authoring sub-struct + Addressing enum (PS-NET-03)

**Priority:** P1 · **Effort:** S · **Recommended subagent:** Haiku-class · mechanical, additive — a single new types-only module or enum; no cross-cutting logic · **Depends on:** none

**Wave:** 1 · **Workstream:** authoring-types · **Role:** types/enum authoring subagent (scan + one new module file)

> Part of the **Profile-System conversion** ([README](README.md), [design](../../specs/profile-system-design.md), [current-state](../../specs/profile-system-current-state.md)). Universal protocol + wave/collision rules live in the README — read it first.

## ⛔ START HERE (worktree setup — do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent
SLUG=ps-net-03
git -C "$REPO" fetch origin
# Base on origin/main. If any "Depends on" brief has merged, its changes are already on main.
git -C "$REPO" worktree add "$REPO/.worktrees/ps-$SLUG" -b "agent/ps-$SLUG" origin/main
cd "$REPO/.worktrees/ps-$SLUG"
git rebase origin/main
```

## Goal

Create NEW module crates/uaa-core/src/profile/components/network.rs defining BOTH types in this one file: `pub enum Addressing { Dhcp, Static { address:String, gateway:String } }` with `#[derive(Debug,Clone,PartialEq,Serialize,Deserialize)] #[serde(tag="type", rename_all="lowercase")]` (serializes as `{type:"dhcp"}` or `{type:"static", address:"192.0.2.1/24", gateway:"192.0.2.1"}`); implement `Default` returning `Addressing::Dhcp`. And `pub struct NetworkConfigPartial { interface:Option<String>, addressing:Option<Addressing>, search:Option<String>, nameservers:Option<Vec<String>>, renderer:Option<String> }` with `#[derive(Debug,Clone,Default,PartialEq,Serialize,Deserialize)] #[serde(deny_unknown_fields, default)]`. Also append `pub mod network;` to crates/uaa-core/src/profile/components/mod.rs (that mod.rs is created by PS-UNLOCK-02; if racing, create it — a plain `pub mod network;` / `pub mod unlock_policy;` re-export file). This enum replaces the magic network_address=="dhcp" string sentinel: document in a doc comment that PS-LOWER-12 maps Addressing::Dhcp -> network_address="dhcp" (gateway empty), Addressing::Static{address,gateway} -> network_address=address + network_gateway=gateway (the flat wire fields confirmed at mod.rs InstallationConfigPartial). No wiring onto InstallationConfigPartial, no merge/lower. New file at 1.0.0.

## Files (expected touch set)

- `crates/uaa-core/src/profile/components/network.rs`
- `crates/uaa-core/src/profile/components/mod.rs`

## Re-verify anchors before editing (line numbers/paths drift — grep first)

```bash
grep -n . crates/uaa-core/src/profile/components/network.rs >/dev/null && echo 'ok: crates/uaa-core/src/profile/components/network.rs' || echo 'MISSING (new or moved): crates/uaa-core/src/profile/components/network.rs'
grep -n . crates/uaa-core/src/profile/components/mod.rs >/dev/null && echo 'ok: crates/uaa-core/src/profile/components/mod.rs' || echo 'MISSING (new or moved): crates/uaa-core/src/profile/components/mod.rs'
```
Zero-hit on a file you expected to edit = STOP and report (the code moved).

## Acceptance criteria

- [ ] Test 1: serialize Addressing::Dhcp, deserialize back, round-trips; serialized form is `{"type":"dhcp"}`
- [ ] Test 2: Addressing::Static with address+gateway round-trips via serde_json
- [ ] Test 3: deserializing `{"type":"static","address":"x"}` (missing gateway) returns Err
- [ ] NetworkConfigPartial serde round-trip test with addressing set to each variant passes
- [ ] file-version header present; new file at 1.0.0; cargo clippy -p uaa-core --all-targets clean
- [ ] `cargo test -p uaa-core -p uaa-control` green for touched crates
- [ ] `cargo clippy --workspace --all-targets` clean
- [ ] File-version headers bumped on **every** changed file (Rust: `// file:/version:/guid:/last-edited:` before `package`/first item; md/yaml/toml comments otherwise)
- [ ] len-serv PlainLuks path stays **byte-identical** unless this brief is an explicit len-serv migration (waves 7–10)

## Commit + PR

Conventional commit; end the body with the repo's Co-Authored-By / Claude-Session trailers.

```
feat(profile): network authoring sub-struct + addressing enum (PS-NET-03)
```

Then `gh pr create` → `gh pr merge <n> --rebase`. Clean up: `git -C "$REPO" worktree remove "$REPO/.worktrees/ps-$SLUG"`.
