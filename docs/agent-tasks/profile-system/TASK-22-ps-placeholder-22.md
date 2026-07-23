<!-- file: docs/agent-tasks/profile-system/TASK-22-ps-placeholder-22.md -->
<!-- version: 1.0.0 -->
<!-- guid: 15ddff00-30ef-4edd-ba26-8eafe10514ec -->
<!-- last-edited: 2026-07-23 -->

# TASK-22 — placeholder-survival test harness (parse->merge) (PS-PLACEHOLDER-22)

**Priority:** P1 · **Effort:** M · **Recommended subagent:** Sonnet-class · a self-contained component with its wiring and tests; bounded, moderate judgment · **Depends on:** PS-MERGE-13 (TASK-18)

**Wave:** 5 · **Workstream:** gates · **Role:** rust-component subagent (implement a component + wiring + tests)

> Part of the **Profile-System conversion** ([README](README.md), [design](../../specs/profile-system-design.md), [current-state](../../specs/profile-system-current-state.md)). Universal protocol + wave/collision rules live in the README — read it first.

## ⛔ START HERE (worktree setup — do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent
SLUG=ps-placeholder-22
git -C "$REPO" fetch origin
# Base on origin/main. If any "Depends on" brief has merged, its changes are already on main.
git -C "$REPO" worktree add "$REPO/.worktrees/ps-$SLUG" -b "agent/ps-$SLUG" origin/main
cd "$REPO/.worktrees/ps-$SLUG"
git rebase origin/main
```

## Goal

Build a reusable harness asserting that a REPLACE_AT_PLACE_TIME token in each secret-bearing FLAT field survives parse(YAML)->merge() unchanged. The current InstallationConfig is flat, so test the literal field names (no component nesting): tpm2_pin, luks_key, root_password, install_ca_cert. Mirror the assertion style of test_merge_passes_placeholders_through (merge.rs:642), NOT test_tpm2_pin_explicit_none_does_not_inherit (that tests inheritance, a different property). Place the harness in crates/uaa-core/tests/placeholder_survival.rs as an integration test; because merge's fixture helpers are #[cfg(test)]-private to merge.rs, the harness constructs its own minimal HostGroupProfile+HostProfile partials (or deserializes a small inline YAML) carrying each placeholder — do NOT re-export the private merge fixtures. Expose a reusable helper `fn assert_placeholder_survives(field_name:&str, group:&HostGroupProfile, host:&HostProfile, extract:impl Fn(&InstallationConfig)->Option<&str>)` documented with an example, so each future migration brief (UNLOCK-27, DISK-28) can call it for its own secret field. New file at 1.0.0.

## Files (expected touch set)

- `crates/uaa-core/tests/placeholder_survival.rs`

## Re-verify anchors before editing (line numbers/paths drift — grep first)

```bash
grep -n . crates/uaa-core/tests/placeholder_survival.rs >/dev/null && echo 'ok: crates/uaa-core/tests/placeholder_survival.rs' || echo 'MISSING (new or moved): crates/uaa-core/tests/placeholder_survival.rs'
```
Zero-hit on a file you expected to edit = STOP and report (the code moved).

## Acceptance criteria

- [ ] harness asserts REPLACE_AT_PLACE_TIME survives parse->merge for tpm2_pin, luks_key, root_password, install_ca_cert
- [ ] a reusable assert_placeholder_survives helper is exposed and documented
- [ ] test runs under cargo test -p uaa-core
- [ ] file-version header on new file (1.0.0); cargo clippy -p uaa-core --all-targets clean
- [ ] `cargo test -p uaa-core -p uaa-control` green for touched crates
- [ ] `cargo clippy --workspace --all-targets` clean
- [ ] File-version headers bumped on **every** changed file (Rust: `// file:/version:/guid:/last-edited:` before `package`/first item; md/yaml/toml comments otherwise)
- [ ] len-serv PlainLuks path stays **byte-identical** unless this brief is an explicit len-serv migration (waves 7–10)

## Commit + PR

Conventional commit; end the body with the repo's Co-Authored-By / Claude-Session trailers.

```
feat(profile): placeholder-survival test harness (parse->merge) (PS-PLACEHOLDER-22)
```

Then `gh pr create` → `gh pr merge <n> --rebase`. Clean up: `git -C "$REPO" worktree remove "$REPO/.worktrees/ps-$SLUG"`.
