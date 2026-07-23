<!-- file: docs/agent-tasks/profile-system/TASK-20-ps-gate-15.md -->
<!-- version: 1.0.0 -->
<!-- guid: 0b1a44e0-8f00-4902-bacf-6da05a0862da -->
<!-- last-edited: 2026-07-23 -->

# TASK-20 — merge-blocking equality gate: parse->merge == committed for 5 hosts (PS-GATE-15)

**Priority:** P1 · **Effort:** M · **Recommended subagent:** Sonnet-class · a self-contained component with its wiring and tests; bounded, moderate judgment · **Depends on:** PS-MERGE-13 (TASK-18)

**Wave:** 5 · **Workstream:** gates · **Role:** rust-component subagent (implement a component + wiring + tests)

> Part of the **Profile-System conversion** ([README](README.md), [design](../../specs/profile-system-design.md), [current-state](../../specs/profile-system-current-state.md)). Universal protocol + wave/collision rules live in the README — read it first.

## ⛔ START HERE (worktree setup — do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent
SLUG=ps-gate-15
git -C "$REPO" fetch origin
# Base on origin/main. If any "Depends on" brief has merged, its changes are already on main.
git -C "$REPO" worktree add "$REPO/.worktrees/ps-$SLUG" -b "agent/ps-$SLUG" origin/main
cd "$REPO/.worktrees/ps-$SLUG"
git rebase origin/main
```

## Goal

Add a merge-blocking test crates/uaa-core/tests/component_equality_gate.rs. For each of the 5 committed hosts (len-serv-001/002/003, unimatrixone, vm-test): hand-author an equivalent component profile (a HostGroupProfile defaults blob + a HostProfile overrides blob) as a YAML fixture under crates/uaa-core/tests/fixtures/components/<host>.yaml; deserialize it (serde_yaml::from_str — this is the "raise" step, NOT a function), run through `uaa_core::profile::merge::merge(&group,&host)` (which internally lowers), and assert the returned InstallationConfig equals the host's committed InstallationConfig via struct equality (the SAME comparison the M2 gate test_resolved_equals_committed_by_struct_equality at crates/uaa/src/cli/config.rs:461 uses — struct-field equality, NOT byte YAML). This is deliberately NOT lower(raise(committed))==committed (tautological). Wire the test into cargo test -p uaa-core (a tests/ integration file runs automatically). IMPORTANT scope note (advisor): the equality gate compares InstallationConfig struct fields ONLY. Disk sizes/reset_enabled are NOT wire fields, so "reset_enabled reproduces today's unconditional RESET staging" is guaranteed by disk_ops.rs being untouched (inert-until-INSTALLER-29), NOT by any assertion here — state this in a comment and do NOT attempt a partition-geometry byte test (it cannot pass without premature installer wiring). For unimatrixone specifically, author tpm2_clevis_peer in the fixture but expect it NOT to appear in the lowered InstallationConfig (peer is storage_mode-derived). New files at 1.0.0.

## Files (expected touch set)

- `crates/uaa-core/tests/component_equality_gate.rs`
- `crates/uaa-core/tests/fixtures/components/len-serv-001.yaml`
- `crates/uaa-core/tests/fixtures/components/len-serv-002.yaml`
- `crates/uaa-core/tests/fixtures/components/len-serv-003.yaml`
- `crates/uaa-core/tests/fixtures/components/unimatrixone.yaml`
- `crates/uaa-core/tests/fixtures/components/vm-test.yaml`

## Re-verify anchors before editing (line numbers/paths drift — grep first)

```bash
grep -n . crates/uaa-core/tests/component_equality_gate.rs >/dev/null && echo 'ok: crates/uaa-core/tests/component_equality_gate.rs' || echo 'MISSING (new or moved): crates/uaa-core/tests/component_equality_gate.rs'
grep -n . crates/uaa-core/tests/fixtures/components/len-serv-001.yaml >/dev/null && echo 'ok: crates/uaa-core/tests/fixtures/components/len-serv-001.yaml' || echo 'MISSING (new or moved): crates/uaa-core/tests/fixtures/components/len-serv-001.yaml'
grep -n . crates/uaa-core/tests/fixtures/components/len-serv-002.yaml >/dev/null && echo 'ok: crates/uaa-core/tests/fixtures/components/len-serv-002.yaml' || echo 'MISSING (new or moved): crates/uaa-core/tests/fixtures/components/len-serv-002.yaml'
grep -n . crates/uaa-core/tests/fixtures/components/len-serv-003.yaml >/dev/null && echo 'ok: crates/uaa-core/tests/fixtures/components/len-serv-003.yaml' || echo 'MISSING (new or moved): crates/uaa-core/tests/fixtures/components/len-serv-003.yaml'
grep -n . crates/uaa-core/tests/fixtures/components/unimatrixone.yaml >/dev/null && echo 'ok: crates/uaa-core/tests/fixtures/components/unimatrixone.yaml' || echo 'MISSING (new or moved): crates/uaa-core/tests/fixtures/components/unimatrixone.yaml'
grep -n . crates/uaa-core/tests/fixtures/components/vm-test.yaml >/dev/null && echo 'ok: crates/uaa-core/tests/fixtures/components/vm-test.yaml' || echo 'MISSING (new or moved): crates/uaa-core/tests/fixtures/components/vm-test.yaml'
```
Zero-hit on a file you expected to edit = STOP and report (the code moved).

## Acceptance criteria

- [ ] test asserts merge(parse(fixture)) == committed InstallationConfig (struct equality) for all 5 hosts
- [ ] a comment documents that geometry/reset_enabled byte-identity comes from disk_ops being untouched, not from this test
- [ ] test runs under cargo test -p uaa-core (CI) and fails loudly if any host drifts
- [ ] file-version headers on new files (1.0.0); cargo clippy -p uaa-core --all-targets clean
- [ ] `cargo test -p uaa-core -p uaa-control` green for touched crates
- [ ] `cargo clippy --workspace --all-targets` clean
- [ ] File-version headers bumped on **every** changed file (Rust: `// file:/version:/guid:/last-edited:` before `package`/first item; md/yaml/toml comments otherwise)
- [ ] len-serv PlainLuks path stays **byte-identical** unless this brief is an explicit len-serv migration (waves 7–10)

## Commit + PR

Conventional commit; end the body with the repo's Co-Authored-By / Claude-Session trailers.

```
feat(profile): merge-blocking equality gate: parse->merge == committed for 5 hosts (PS-GATE-15)
```

Then `gh pr create` → `gh pr merge <n> --rebase`. Clean up: `git -C "$REPO" worktree remove "$REPO/.worktrees/ps-$SLUG"`.
