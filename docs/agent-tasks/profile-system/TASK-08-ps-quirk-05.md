<!-- file: docs/agent-tasks/profile-system/TASK-08-ps-quirk-05.md -->
<!-- version: 1.0.0 -->
<!-- guid: 41ad679a-ceb8-49f2-bb25-76cc5eac1e51 -->
<!-- last-edited: 2026-07-23 -->

# TASK-08 — firmware-quirks closed enum + Vec type (PS-QUIRK-05)

**Priority:** P1 · **Effort:** M · **Recommended subagent:** Sonnet-class · a self-contained component with its wiring and tests; bounded, moderate judgment · **Depends on:** none

**Wave:** 1 · **Workstream:** authoring-types · **Role:** rust-component subagent (implement a component + wiring + tests)

> Part of the **Profile-System conversion** ([README](README.md), [design](../../specs/profile-system-design.md), [current-state](../../specs/profile-system-current-state.md)). Universal protocol + wave/collision rules live in the README — read it first.

## ⛔ START HERE (worktree setup — do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent
SLUG=ps-quirk-05
git -C "$REPO" fetch origin
# Base on origin/main. If any "Depends on" brief has merged, its changes are already on main.
git -C "$REPO" worktree add "$REPO/.worktrees/ps-$SLUG" -b "agent/ps-$SLUG" origin/main
cd "$REPO/.worktrees/ps-$SLUG"
git rebase origin/main
```

## Goal

Create NEW module crates/uaa-core/src/network/ssh_installer/components/firmware_quirks.rs defining a closed tagged enum `pub enum FirmwareQuirk { GrubRemovableFallback, ForceNicDriver { driver:String }, WatchdogStaggered { slot:u8, interval_secs:u32 } }` with `#[derive(Debug,Clone,PartialEq,Serialize,Deserialize)] #[serde(tag="kind", rename_all="kebab-case", deny_unknown_fields)]`. Note: with the internally-tagged `tag="kind"` representation, `deny_unknown_fields` is placed at the enum level (it applies to each struct variant's fields plus the tag); a unit variant (GrubRemovableFallback) serializes as `{"kind":"grub-removable-fallback"}`. This is a variant-select (union-by-kind) component carried as `Vec<FirmwareQuirk>`. Do NOT model serial-console (PS-SERIAL-18 makes it an arch-gated installer default, never a quirk) or nvme-cant-boot (stays in DiskRole::System) as quirks. WatchdogStaggered params (slot=which staggered slot, interval_secs=watchdog interval) are greenfield stubs for the rpi Tang watchdog; add a `// TODO(PS-MIG-RPI-24): finalize staggered-watchdog params` and gate NO behavior on them. Append `pub mod firmware_quirks;` to crates/uaa-core/src/network/ssh_installer/components/mod.rs (created by PS-DISK-01; create it if racing). No wiring onto InstallationConfig. New file at 1.0.0. Exercise skip_serializing_if here with a test-local wrapper (`struct Holder { #[serde(default, skip_serializing_if="Vec::is_empty")] quirks:Vec<FirmwareQuirk> }`) since there is no InstallationConfig field yet.

## Files (expected touch set)

- `crates/uaa-core/src/network/ssh_installer/components/firmware_quirks.rs`
- `crates/uaa-core/src/network/ssh_installer/components/mod.rs`

## Re-verify anchors before editing (line numbers/paths drift — grep first)

```bash
grep -n . crates/uaa-core/src/network/ssh_installer/components/firmware_quirks.rs >/dev/null && echo 'ok: crates/uaa-core/src/network/ssh_installer/components/firmware_quirks.rs' || echo 'MISSING (new or moved): crates/uaa-core/src/network/ssh_installer/components/firmware_quirks.rs'
grep -n . crates/uaa-core/src/network/ssh_installer/components/mod.rs >/dev/null && echo 'ok: crates/uaa-core/src/network/ssh_installer/components/mod.rs' || echo 'MISSING (new or moved): crates/uaa-core/src/network/ssh_installer/components/mod.rs'
```
Zero-hit on a file you expected to edit = STOP and report (the code moved).

## Acceptance criteria

- [ ] serde round-trip test for each of the 3 variants passes (unit variant serializes to {"kind":"grub-removable-fallback"})
- [ ] an unknown quirk kind fails to deserialize (asserted Err)
- [ ] test-local Holder with empty Vec serializes with the quirks key omitted (skip_serializing_if verified)
- [ ] cargo test -p uaa-core --lib passes with the new tests visible in the count
- [ ] file-version header present; new file at 1.0.0; cargo clippy -p uaa-core --all-targets clean
- [ ] `cargo test -p uaa-core -p uaa-control` green for touched crates
- [ ] `cargo clippy --workspace --all-targets` clean
- [ ] File-version headers bumped on **every** changed file (Rust: `// file:/version:/guid:/last-edited:` before `package`/first item; md/yaml/toml comments otherwise)
- [ ] len-serv PlainLuks path stays **byte-identical** unless this brief is an explicit len-serv migration (waves 7–10)

## Commit + PR

Conventional commit; end the body with the repo's Co-Authored-By / Claude-Session trailers.

```
feat(profile): firmware-quirks closed enum + vec type (PS-QUIRK-05)
```

Then `gh pr create` → `gh pr merge <n> --rebase`. Clean up: `git -C "$REPO" worktree remove "$REPO/.worktrees/ps-$SLUG"`.
