<!-- file: docs/agent-tasks/profile-system/TASK-11-ps-vmgate-19.md -->
<!-- version: 1.0.0 -->
<!-- guid: 34dee77c-ebc2-4c18-a193-af31c6710486 -->
<!-- last-edited: 2026-07-23 -->

# TASK-11 — make VM-gate readiness probe role/application-driven (PS-VMGATE-19)

**Priority:** P1 · **Effort:** M · **Recommended subagent:** Sonnet-class · a self-contained component with its wiring and tests; bounded, moderate judgment · **Depends on:** none

**Wave:** 1 · **Workstream:** gates · **Role:** rust-component subagent (implement a component + wiring + tests)

> Part of the **Profile-System conversion** ([README](README.md), [design](../../specs/profile-system-design.md), [current-state](../../specs/profile-system-current-state.md)). Universal protocol + wave/collision rules live in the README — read it first.

## ⛔ START HERE (worktree setup — do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent
SLUG=ps-vmgate-19
git -C "$REPO" fetch origin
# Base on origin/main. If any "Depends on" brief has merged, its changes are already on main.
git -C "$REPO" worktree add "$REPO/.worktrees/ps-$SLUG" -b "agent/ps-$SLUG" origin/main
cd "$REPO/.worktrees/ps-$SLUG"
git rebase origin/main
```

## Goal

Generalize scripts/vm-validate.sh stage 6 readiness assertion (hardcoded cockroach `SELECT 1` at lines ~534-556) to dispatch on the profile's application kind, extracted from the YAML as applications[0].kind (there is NO host-level role field in the config; dispatch keys off applications[].kind and disks[].role only): kind=cockroach -> the existing SQL probe (behavior unchanged); kind=tang-server -> a Tang readiness check following the in-repo pattern at crates/uaa-core/src/luks_keys.rs (~line 44): `curl -sf --max-time 5 <url>/adv` — note fleet Tang binds port 80, so no explicit port suffix; empty applications -> assert multi-user.target reached only. Create an app-free fixture examples/configs/install/vm-test-app-free.yaml (copy vm-test.yaml, delete the applications: section) so criterion 3 is testable. Keep stages 0-5 generic. Add a code comment flagging that the ZFS/LUKS assertions at lines ~497-513 are storage-mode-specific and will need role/storage-mode gating in a future brief before a pure arm64/tang-only config can fully pass (out of scope here). Bump the script header 1.1.0->1.2.0 (feature). shellcheck clean.

## Files (expected touch set)

- `scripts/vm-validate.sh`
- `examples/configs/install/vm-test-app-free.yaml`

## Re-verify anchors before editing (line numbers/paths drift — grep first)

```bash
grep -n . scripts/vm-validate.sh >/dev/null && echo 'ok: scripts/vm-validate.sh' || echo 'MISSING (new or moved): scripts/vm-validate.sh'
grep -n . examples/configs/install/vm-test-app-free.yaml >/dev/null && echo 'ok: examples/configs/install/vm-test-app-free.yaml' || echo 'MISSING (new or moved): examples/configs/install/vm-test-app-free.yaml'
```
Zero-hit on a file you expected to edit = STOP and report (the code moved).

## Acceptance criteria

- [ ] cockroach profile still selects the SQL readiness probe (unchanged behavior)
- [ ] a tang-server profile selects the curl /adv readiness check (demonstrated via dry-run output in the PR showing the if/else dispatch on applications[0].kind)
- [ ] the app-free fixture selects the multi-user.target-only branch
- [ ] script header bumped to 1.2.0; shellcheck clean (or documented waivers)
- [ ] `cargo test -p uaa-core -p uaa-control` green for touched crates
- [ ] `cargo clippy --workspace --all-targets` clean
- [ ] File-version headers bumped on **every** changed file (Rust: `// file:/version:/guid:/last-edited:` before `package`/first item; md/yaml/toml comments otherwise)
- [ ] len-serv PlainLuks path stays **byte-identical** unless this brief is an explicit len-serv migration (waves 7–10)

## Commit + PR

Conventional commit; end the body with the repo's Co-Authored-By / Claude-Session trailers.

```
feat(profile): make vm-gate readiness probe role/application-driven (PS-VMGATE-19)
```

Then `gh pr create` → `gh pr merge <n> --rebase`. Clean up: `git -C "$REPO" worktree remove "$REPO/.worktrees/ps-$SLUG"`.
