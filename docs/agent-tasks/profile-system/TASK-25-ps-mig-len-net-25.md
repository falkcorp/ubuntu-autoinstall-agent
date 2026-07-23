<!-- file: docs/agent-tasks/profile-system/TASK-25-ps-mig-len-net-25.md -->
<!-- version: 1.0.0 -->
<!-- guid: f386f448-8867-4752-b0af-07d9df4dbdf4 -->
<!-- last-edited: 2026-07-23 -->

# TASK-25 — migrate len-serv group: network component (step 1, lowest risk) (PS-MIG-LEN-NET-25)

**Priority:** P2 · **Effort:** S · **Recommended subagent:** Sonnet-class · a self-contained component with its wiring and tests; bounded, moderate judgment · **Depends on:** PS-MIG-U1-23 (TASK-24)

**Wave:** 7 · **Workstream:** host-migration · **Role:** rust-component subagent (implement a component + wiring + tests)

> Part of the **Profile-System conversion** ([README](README.md), [design](../../specs/profile-system-design.md), [current-state](../../specs/profile-system-current-state.md)). Universal protocol + wave/collision rules live in the README — read it first.

## ⛔ START HERE (worktree setup — do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent
SLUG=ps-mig-len-net-25
git -C "$REPO" fetch origin
# Base on origin/main. If any "Depends on" brief has merged, its changes are already on main.
git -C "$REPO" worktree add "$REPO/.worktrees/ps-$SLUG" -b "agent/ps-$SLUG" origin/main
cd "$REPO/.worktrees/ps-$SLUG"
git rebase origin/main
```

## Goal

Convert ONLY the network axis of the shared len-serv HostGroupProfile to the network component, reusing the concrete component-in-group-defaults shape established by PS-MIG-U1-23 (read that fixture first). The len-serv group defaults are authored at reify time — modify the reify path (crates/uaa-control/src/profiles/reify.rs, where group defaults are built) and/or the committed len-serv group authoring so the group defaults carry a `network:` component block (interface + addressing=static + search + nameservers + renderer as group defaults) and each host profile override carries only its per-host address (Addressing::Static differing by address). Leave all other len-serv fields flat. Merge contract (from PS-MERGE-13): network is a field-component — a host overriding addressing.address inherits interface/search/nameservers/renderer from the group. Bump the len-serv group row schema_version to 1 (field from PS-SCHEMA-20). Add fixtures crates/uaa-core/tests/fixtures/components/len-serv-network.yaml showing the group defaults network block + two host overrides + the expected resolved config. GATE (no secret this step): the component_equality_gate (PS-GATE-15) must stay green for all 3 len-serv nodes — run `cargo test -p uaa-core component_equality_gate`; it exercises len-serv-001/002/003 (parameterization at crates/uaa/src/cli/config.rs:509-520). Only the network axis is componentized (diff-reviewed). Bump headers.

## Files (expected touch set)

- `crates/uaa-control/src/profiles/reify.rs`
- `crates/uaa-core/tests/fixtures/components/len-serv-network.yaml`

## Re-verify anchors before editing (line numbers/paths drift — grep first)

```bash
grep -n . crates/uaa-control/src/profiles/reify.rs >/dev/null && echo 'ok: crates/uaa-control/src/profiles/reify.rs' || echo 'MISSING (new or moved): crates/uaa-control/src/profiles/reify.rs'
grep -n . crates/uaa-core/tests/fixtures/components/len-serv-network.yaml >/dev/null && echo 'ok: crates/uaa-core/tests/fixtures/components/len-serv-network.yaml' || echo 'MISSING (new or moved): crates/uaa-core/tests/fixtures/components/len-serv-network.yaml'
```
Zero-hit on a file you expected to edit = STOP and report (the code moved).

## Acceptance criteria

- [ ] component_equality_gate green for all 3 len-serv nodes with network componentized (merge->lower struct-equals committed)
- [ ] only network fields are componentized; other axes remain flat (diff-reviewed)
- [ ] len-serv group row schema_version=1
- [ ] file-version headers bumped; cargo clippy --all-targets clean
- [ ] `cargo test -p uaa-core -p uaa-control` green for touched crates
- [ ] `cargo clippy --workspace --all-targets` clean
- [ ] File-version headers bumped on **every** changed file (Rust: `// file:/version:/guid:/last-edited:` before `package`/first item; md/yaml/toml comments otherwise)
- [ ] len-serv PlainLuks path stays **byte-identical** unless this brief is an explicit len-serv migration (waves 7–10)

## Commit + PR

Conventional commit; end the body with the repo's Co-Authored-By / Claude-Session trailers.

```
feat(profile): migrate len-serv group: network component (step 1, lowest risk) (PS-MIG-LEN-NET-25)
```

Then `gh pr create` → `gh pr merge <n> --rebase`. Clean up: `git -C "$REPO" worktree remove "$REPO/.worktrees/ps-$SLUG"`.
