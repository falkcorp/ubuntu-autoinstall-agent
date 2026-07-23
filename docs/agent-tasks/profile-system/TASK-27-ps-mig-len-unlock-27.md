<!-- file: docs/agent-tasks/profile-system/TASK-27-ps-mig-len-unlock-27.md -->
<!-- version: 1.0.0 -->
<!-- guid: bf42c463-f8ed-4240-962b-11fc5c7eb678 -->
<!-- last-edited: 2026-07-23 -->

# TASK-27 — migrate len-serv group: unlock-policy component (step 3, secret-bearing) (PS-MIG-LEN-UNLOCK-27)

**Priority:** P2 · **Effort:** M · **Recommended subagent:** Sonnet-class · a self-contained component with its wiring and tests; bounded, moderate judgment · **Depends on:** PS-MIG-LEN-IMG-26 (TASK-26)

**Wave:** 9 · **Workstream:** host-migration · **Role:** rust-component subagent (implement a component + wiring + tests)

> Part of the **Profile-System conversion** ([README](README.md), [design](../../specs/profile-system-design.md), [current-state](../../specs/profile-system-current-state.md)). Universal protocol + wave/collision rules live in the README — read it first.

## ⛔ START HERE (worktree setup — do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent
SLUG=ps-mig-len-unlock-27
git -C "$REPO" fetch origin
# Base on origin/main. If any "Depends on" brief has merged, its changes are already on main.
git -C "$REPO" worktree add "$REPO/.worktrees/ps-$SLUG" -b "agent/ps-$SLUG" origin/main
cd "$REPO/.worktrees/ps-$SLUG"
git rebase origin/main
```

## Goal

Convert the unlock-policy axis of the shared len-serv group to the unlock_policy component (tang with servers+threshold=2, tpm2_pin with enroll + pin + pcr_ids, fido2_expected). This is secret-bearing (tpm2_pin), so DOUBLE-GATED. Preserve the tpm2_pin double-Option inherit-vs-explicit-none semantics through the migrated authoring: the group carries the pin default; a host that wants no pin authors explicit-null; a host that inherits omits it (Tpm2PinPartial.pin uses super::super::deserialize_double_option, established in PS-UNLOCK-02). Leave disk-layout flat still. Follow the PS-MIG-LEN-IMG-26 pattern. Map (from PS-UNLOCK-02 doc): tang.servers->tang_servers, tang.threshold->tang_threshold, tpm2_pin.pin->tpm2_pin, tpm2_pin.pcr_ids->tpm2_pcr_ids, tpm2_pin.enroll->enroll_tpm2, fido2_expected->expect_fido2. GATES: (1) component_equality_gate green for all 3 nodes with network+base_image+unlock componentized; (2) placeholder-survival helper (PS-PLACEHOLDER-22 assert_placeholder_survives) passes for tpm2_pin on the migrated group; (3) a test asserting tpm2_pin explicit-none on a host does NOT inherit the group pin (mirrors test_tpm2_pin_explicit_none_does_not_inherit). Add fixture len-serv-unlock.yaml. Bump headers.

## Files (expected touch set)

- `crates/uaa-control/src/profiles/reify.rs`
- `crates/uaa-core/tests/fixtures/components/len-serv-unlock.yaml`

## Re-verify anchors before editing (line numbers/paths drift — grep first)

```bash
grep -n . crates/uaa-control/src/profiles/reify.rs >/dev/null && echo 'ok: crates/uaa-control/src/profiles/reify.rs' || echo 'MISSING (new or moved): crates/uaa-control/src/profiles/reify.rs'
grep -n . crates/uaa-core/tests/fixtures/components/len-serv-unlock.yaml >/dev/null && echo 'ok: crates/uaa-core/tests/fixtures/components/len-serv-unlock.yaml' || echo 'MISSING (new or moved): crates/uaa-core/tests/fixtures/components/len-serv-unlock.yaml'
```
Zero-hit on a file you expected to edit = STOP and report (the code moved).

## Acceptance criteria

- [ ] component_equality_gate green for all 3 len-serv nodes with network+base_image+unlock components
- [ ] placeholder-survival passes for tpm2_pin on the migrated len-serv group
- [ ] tpm2_pin explicit-none does not inherit the group pin (test)
- [ ] file-version headers bumped; cargo clippy --all-targets clean
- [ ] `cargo test -p uaa-core -p uaa-control` green for touched crates
- [ ] `cargo clippy --workspace --all-targets` clean
- [ ] File-version headers bumped on **every** changed file (Rust: `// file:/version:/guid:/last-edited:` before `package`/first item; md/yaml/toml comments otherwise)
- [ ] len-serv PlainLuks path stays **byte-identical** unless this brief is an explicit len-serv migration (waves 7–10)

## Commit + PR

Conventional commit; end the body with the repo's Co-Authored-By / Claude-Session trailers.

```
feat(profile): migrate len-serv group: unlock-policy component (step 3, secret-bearing) (PS-MIG-LEN-UNLOCK-27)
```

Then `gh pr create` → `gh pr merge <n> --rebase`. Clean up: `git -C "$REPO" worktree remove "$REPO/.worktrees/ps-$SLUG"`.
