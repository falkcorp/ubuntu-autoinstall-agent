<!-- file: docs/agent-tasks/profile-system/TASK-28-ps-mig-len-disk-28.md -->
<!-- version: 1.0.0 -->
<!-- guid: e733adcd-6a16-4bd6-9019-a56264228ffe -->
<!-- last-edited: 2026-07-23 -->

# TASK-28 — migrate len-serv group: disk-layout component (step 4, riskiest, last) (PS-MIG-LEN-DISK-28)

**Priority:** P2 · **Effort:** M · **Recommended subagent:** Sonnet-class · a self-contained component with its wiring and tests; bounded, moderate judgment · **Depends on:** PS-MIG-LEN-UNLOCK-27 (TASK-27)

**Wave:** 10 · **Workstream:** host-migration · **Role:** rust-component subagent (implement a component + wiring + tests)

> Part of the **Profile-System conversion** ([README](README.md), [design](../../specs/profile-system-design.md), [current-state](../../specs/profile-system-current-state.md)). Universal protocol + wave/collision rules live in the README — read it first.

## ⛔ START HERE (worktree setup — do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent
SLUG=ps-mig-len-disk-28
git -C "$REPO" fetch origin
# Base on origin/main. If any "Depends on" brief has merged, its changes are already on main.
git -C "$REPO" worktree add "$REPO/.worktrees/ps-$SLUG" -b "agent/ps-$SLUG" origin/main
cd "$REPO/.worktrees/ps-$SLUG"
git rebase origin/main
```

## Goal

Convert the disk-layout axis of the shared len-serv group to disk_layout=SingleLuks with disk_device + esp_size="512M"/reset_size="4G"/bpool_size="2G" (the disk_ops.rs literals) + reset_enabled=true. CRITICAL (advisor): these sizes and reset_enabled are authoring-expressibility ONLY and are NOT wire fields — lower() drops them, so the lowered InstallationConfig is byte-identical BECAUSE disk_ops.rs is untouched (inert-until-PS-INSTALLER-29), NOT because a partition-geometry test proves it. Do NOT add a partition-geometry byte test (it cannot pass without premature installer wiring). The equality gate remains struct-equality of InstallationConfig; SingleLuks lowers to storage_mode=PlainLuks + disk_device. After this step the whole len-serv group is component-authored. DOUBLE-GATED: (1) component_equality_gate green for all 3 nodes with ALL axes (network+base_image+unlock+disk_layout) componentized; (2) placeholder-survival for luks_key via PS-PLACEHOLDER-22's helper. Add fixture len-serv-disk.yaml. Bump headers.

## Files (expected touch set)

- `crates/uaa-control/src/profiles/reify.rs`
- `crates/uaa-core/tests/fixtures/components/len-serv-disk.yaml`

## Re-verify anchors before editing (line numbers/paths drift — grep first)

```bash
grep -n . crates/uaa-control/src/profiles/reify.rs >/dev/null && echo 'ok: crates/uaa-control/src/profiles/reify.rs' || echo 'MISSING (new or moved): crates/uaa-control/src/profiles/reify.rs'
grep -n . crates/uaa-core/tests/fixtures/components/len-serv-disk.yaml >/dev/null && echo 'ok: crates/uaa-core/tests/fixtures/components/len-serv-disk.yaml' || echo 'MISSING (new or moved): crates/uaa-core/tests/fixtures/components/len-serv-disk.yaml'
```
Zero-hit on a file you expected to edit = STOP and report (the code moved).

## Acceptance criteria

- [ ] component_equality_gate green for all 3 fully-component-authored len-serv nodes (merge->lower struct-equals committed)
- [ ] placeholder-survival passes for luks_key
- [ ] a comment documents that geometry byte-identity comes from disk_ops being untouched (inert sizes), not from a geometry test; sizes/reset_enabled absent from the lowered config (asserted)
- [ ] file-version headers bumped; cargo clippy --all-targets clean
- [ ] `cargo test -p uaa-core -p uaa-control` green for touched crates
- [ ] `cargo clippy --workspace --all-targets` clean
- [ ] File-version headers bumped on **every** changed file (Rust: `// file:/version:/guid:/last-edited:` before `package`/first item; md/yaml/toml comments otherwise)
- [ ] len-serv PlainLuks path stays **byte-identical** unless this brief is an explicit len-serv migration (waves 7–10)

## Commit + PR

Conventional commit; end the body with the repo's Co-Authored-By / Claude-Session trailers.

```
feat(profile): migrate len-serv group: disk-layout component (step 4, riskiest, last) (PS-MIG-LEN-DISK-28)
```

Then `gh pr create` → `gh pr merge <n> --rebase`. Clean up: `git -C "$REPO" worktree remove "$REPO/.worktrees/ps-$SLUG"`.
