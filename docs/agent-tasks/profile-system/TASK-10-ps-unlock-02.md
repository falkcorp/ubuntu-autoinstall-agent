<!-- file: docs/agent-tasks/profile-system/TASK-10-ps-unlock-02.md -->
<!-- version: 1.0.0 -->
<!-- guid: e89fa8af-f0ab-4248-83d9-57ea9eb6c508 -->
<!-- last-edited: 2026-07-23 -->

# TASK-10 — unlock-policy authoring sub-struct (UnlockPolicyPartial) + TangServer PartialEq (PS-UNLOCK-02)

**Priority:** P1 · **Effort:** M · **Recommended subagent:** Sonnet-class · a self-contained component with its wiring and tests; bounded, moderate judgment · **Depends on:** none

**Wave:** 1 · **Workstream:** authoring-types · **Role:** rust-component subagent (implement a component + wiring + tests)

> Part of the **Profile-System conversion** ([README](README.md), [design](../../specs/profile-system-design.md), [current-state](../../specs/profile-system-current-state.md)). Universal protocol + wave/collision rules live in the README — read it first.

## ⛔ START HERE (worktree setup — do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent
SLUG=ps-unlock-02
git -C "$REPO" fetch origin
# Base on origin/main. If any "Depends on" brief has merged, its changes are already on main.
git -C "$REPO" worktree add "$REPO/.worktrees/ps-$SLUG" -b "agent/ps-$SLUG" origin/main
cd "$REPO/.worktrees/ps-$SLUG"
git rebase origin/main
```

## Goal

Create NEW module crates/uaa-core/src/profile/components/unlock_policy.rs and NEW crates/uaa-core/src/profile/components/mod.rs (re-export module), and append `pub mod components;` to crates/uaa-core/src/profile/mod.rs (declaration line ONLY — this is NOT the forbidden InstallationConfigPartial field-wiring, which is PS-WIRE-PARTIAL-11). Also add `PartialEq` to the derive on `TangServer` in ssh_installer/config.rs (its sole field is `url:String`, so the derive compiles; this unblocks PartialEq on every struct that holds Vec<TangServer>; do NOT alter the existing hand-written tang_servers comparison in mod.rs:105). Define: `pub struct UnlockPolicyPartial { tang:Option<TangSssPartial>, tpm2_pin:Option<Tpm2PinPartial>, tpm2_clevis_peer:Option<bool>, fido2_expected:Option<bool> }`; `pub struct TangSssPartial { servers:Option<Vec<TangServer>>, threshold:Option<u8> }`; `pub struct Tpm2PinPartial { #[serde(default, skip_serializing_if="Option::is_none", deserialize_with="super::super::deserialize_double_option")] pin:Option<Option<String>>, pcr_ids:Option<String>, enroll:Option<bool> }`. deserialize_double_option is private in mod.rs:95 — reach it via super::super (do NOT add a second helper or bump its visibility). All three structs `#[derive(Debug,Clone,Default,PartialEq,Serialize,Deserialize)]` with `#[serde(deny_unknown_fields, default)]`. Document the authoring->flat-wire mapping in a doc comment so PS-LOWER-12 can consume it verbatim: tang.servers->tang_servers, tang.threshold->tang_threshold, tpm2_pin.pin->tpm2_pin (double-option preserved), tpm2_pin.pcr_ids->tpm2_pcr_ids, tpm2_pin.enroll->enroll_tpm2, fido2_expected->expect_fido2, and tpm2_clevis_peer is authoring/validate-ONLY (D2-B clevis peer is derived by the installer from storage_mode==NativeKeystore at system_setup.rs:722/772 — it is NOT lowered to any wire field). Authoring-types only; no wiring, no merge/lower. New files at 1.0.0. A future brief (PS-WIRE-PARTIAL-11) wires this as `unlock_policy:Option<UnlockPolicyPartial>`.

## Files (expected touch set)

- `crates/uaa-core/src/profile/components/unlock_policy.rs`
- `crates/uaa-core/src/profile/components/mod.rs`
- `crates/uaa-core/src/profile/mod.rs`
- `crates/uaa-core/src/network/ssh_installer/config.rs`

## Re-verify anchors before editing (line numbers/paths drift — grep first)

```bash
grep -n . crates/uaa-core/src/profile/components/unlock_policy.rs >/dev/null && echo 'ok: crates/uaa-core/src/profile/components/unlock_policy.rs' || echo 'MISSING (new or moved): crates/uaa-core/src/profile/components/unlock_policy.rs'
grep -n . crates/uaa-core/src/profile/components/mod.rs >/dev/null && echo 'ok: crates/uaa-core/src/profile/components/mod.rs' || echo 'MISSING (new or moved): crates/uaa-core/src/profile/components/mod.rs'
grep -n . crates/uaa-core/src/profile/mod.rs >/dev/null && echo 'ok: crates/uaa-core/src/profile/mod.rs' || echo 'MISSING (new or moved): crates/uaa-core/src/profile/mod.rs'
grep -n . crates/uaa-core/src/network/ssh_installer/config.rs >/dev/null && echo 'ok: crates/uaa-core/src/network/ssh_installer/config.rs' || echo 'MISSING (new or moved): crates/uaa-core/src/network/ssh_installer/config.rs'
```
Zero-hit on a file you expected to edit = STOP and report (the code moved).

## Acceptance criteria

- [ ] unit test asserts tpm2_pin.pin absent (inherit) vs explicit-null vs present are THREE distinct deserialized states, tested on Tpm2PinPartial via serde_json inputs {}, {"pin":null}, {"pin":"x"}
- [ ] serde round-trip test for a fully-populated UnlockPolicyPartial (all Some, tang with 2 servers) passes
- [ ] reuses super::super::deserialize_double_option (no new double-option helper; grep shows one definition)
- [ ] TangServer now derives PartialEq and existing mod.rs tang_servers comparison is unchanged; cargo test -p uaa-core --lib green
- [ ] file-version headers present; new files at 1.0.0; cargo clippy -p uaa-core --all-targets clean
- [ ] `cargo test -p uaa-core -p uaa-control` green for touched crates
- [ ] `cargo clippy --workspace --all-targets` clean
- [ ] File-version headers bumped on **every** changed file (Rust: `// file:/version:/guid:/last-edited:` before `package`/first item; md/yaml/toml comments otherwise)
- [ ] len-serv PlainLuks path stays **byte-identical** unless this brief is an explicit len-serv migration (waves 7–10)

## Commit + PR

Conventional commit; end the body with the repo's Co-Authored-By / Claude-Session trailers.

```
feat(profile): unlock-policy authoring sub-struct (unlockpolicypartial) + tangserver partialeq (PS-UNLOCK-02)
```

Then `gh pr create` → `gh pr merge <n> --rebase`. Clean up: `git -C "$REPO" worktree remove "$REPO/.worktrees/ps-$SLUG"`.
