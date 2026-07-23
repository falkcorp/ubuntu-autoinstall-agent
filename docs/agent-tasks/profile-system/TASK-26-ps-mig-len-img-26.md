<!-- file: docs/agent-tasks/profile-system/TASK-26-ps-mig-len-img-26.md -->
<!-- version: 1.0.0 -->
<!-- guid: 34bfb9c1-f9d8-477d-b4fe-0d0bc779dce6 -->
<!-- last-edited: 2026-07-23 -->

# TASK-26 — migrate len-serv group: base-image component (step 2) (PS-MIG-LEN-IMG-26)

**Priority:** P2 · **Effort:** S · **Recommended subagent:** Sonnet-class · a self-contained component with its wiring and tests; bounded, moderate judgment · **Depends on:** PS-MIG-LEN-NET-25 (TASK-25)

**Wave:** 8 · **Workstream:** host-migration · **Role:** rust-component subagent (implement a component + wiring + tests)

> Part of the **Profile-System conversion** ([README](README.md), [design](../../specs/profile-system-design.md), [current-state](../../specs/profile-system-current-state.md)). Universal protocol + wave/collision rules live in the README — read it first.

## ⛔ START HERE (worktree setup — do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent
SLUG=ps-mig-len-img-26
git -C "$REPO" fetch origin
# Base on origin/main. If any "Depends on" brief has merged, its changes are already on main.
git -C "$REPO" worktree add "$REPO/.worktrees/ps-$SLUG" -b "agent/ps-$SLUG" origin/main
cd "$REPO/.worktrees/ps-$SLUG"
git rebase origin/main
```

## Goal

Convert the base-image axis of the shared len-serv group to the base_image component (release + mirror + initramfs as group defaults; fallback_mirror is authoring-expressibility only and does not affect the lowered config). Leave unlock-policy and disk-layout flat still. Follow the exact pattern PS-MIG-LEN-NET-25 established (read its fixture and reify edit). Map: debootstrap_release->base_image.release, debootstrap_mirror->base_image.mirror, initramfs_type->base_image.initramfs. GATE: component_equality_gate stays green for all 3 nodes with network+base_image now componentized (`cargo test -p uaa-core component_equality_gate`). Only base-image newly componentized this step (diff-reviewed). Bump the len-serv group row schema_version (already 1; keep at 1 unless a prior bump requires increment — schema_version tracks binary-recognized shape, not per-axis). Add fixture crates/uaa-core/tests/fixtures/components/len-serv-base-image.yaml. Bump headers.

## Files (expected touch set)

- `crates/uaa-control/src/profiles/reify.rs`
- `crates/uaa-core/tests/fixtures/components/len-serv-base-image.yaml`

## Re-verify anchors before editing (line numbers/paths drift — grep first)

```bash
grep -n . crates/uaa-control/src/profiles/reify.rs >/dev/null && echo 'ok: crates/uaa-control/src/profiles/reify.rs' || echo 'MISSING (new or moved): crates/uaa-control/src/profiles/reify.rs'
grep -n . crates/uaa-core/tests/fixtures/components/len-serv-base-image.yaml >/dev/null && echo 'ok: crates/uaa-core/tests/fixtures/components/len-serv-base-image.yaml' || echo 'MISSING (new or moved): crates/uaa-core/tests/fixtures/components/len-serv-base-image.yaml'
```
Zero-hit on a file you expected to edit = STOP and report (the code moved).

## Acceptance criteria

- [ ] component_equality_gate green for all 3 len-serv nodes with network+base_image components (merge->lower struct-equals committed)
- [ ] only base-image newly componentized this step (diff-reviewed)
- [ ] file-version headers bumped; cargo clippy --all-targets clean
- [ ] `cargo test -p uaa-core -p uaa-control` green for touched crates
- [ ] `cargo clippy --workspace --all-targets` clean
- [ ] File-version headers bumped on **every** changed file (Rust: `// file:/version:/guid:/last-edited:` before `package`/first item; md/yaml/toml comments otherwise)
- [ ] len-serv PlainLuks path stays **byte-identical** unless this brief is an explicit len-serv migration (waves 7–10)

## Commit + PR

Conventional commit; end the body with the repo's Co-Authored-By / Claude-Session trailers.

```
feat(profile): migrate len-serv group: base-image component (step 2) (PS-MIG-LEN-IMG-26)
```

Then `gh pr create` → `gh pr merge <n> --rebase`. Clean up: `git -C "$REPO" worktree remove "$REPO/.worktrees/ps-$SLUG"`.
