<!-- file: docs/agent-tasks/profile-system/TASK-12-ps-wire-axes-10.md -->
<!-- version: 1.0.0 -->
<!-- guid: d2eb3b3c-1de7-4949-8b12-2b456cb534d3 -->
<!-- last-edited: 2026-07-23 -->

# TASK-12 — wire arch/role/firmware_quirks/hooks onto InstallationConfig (additive, byte-identical) (PS-WIRE-AXES-10)

**Priority:** P1 · **Effort:** M · **Recommended subagent:** Sonnet-class · a self-contained component with its wiring and tests; bounded, moderate judgment · **Depends on:** PS-ARCH-07 (TASK-02), PS-ROLE-08 (TASK-09), PS-QUIRK-05 (TASK-08), PS-HOOK-06 (TASK-04)

**Wave:** 2 · **Workstream:** wire-integration · **Role:** rust-component subagent (implement a component + wiring + tests)

> Part of the **Profile-System conversion** ([README](README.md), [design](../../specs/profile-system-design.md), [current-state](../../specs/profile-system-current-state.md)). Universal protocol + wave/collision rules live in the README — read it first.

## ⛔ START HERE (worktree setup — do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent
SLUG=ps-wire-axes-10
git -C "$REPO" fetch origin
# Base on origin/main. If any "Depends on" brief has merged, its changes are already on main.
git -C "$REPO" worktree add "$REPO/.worktrees/ps-$SLUG" -b "agent/ps-$SLUG" origin/main
cd "$REPO/.worktrees/ps-$SLUG"
git rebase origin/main
```

## Goal

Add four additive fields to the wire InstallationConfig in config.rs, each skip-if-default so an omitting host serializes byte-identically to today (the discipline proven by test_plain_luks_host_omits_storage_keys / StorageMode::is_default): `#[serde(default, skip_serializing_if="Arch::is_amd64")] pub arch:Arch`; `#[serde(default, skip_serializing_if="HostRole::is_install_target")] pub role:HostRole`; `#[serde(default, skip_serializing_if="Vec::is_empty")] pub firmware_quirks:Vec<FirmwareQuirk>`; `#[serde(default, skip_serializing_if="Hooks::is_empty")] pub hooks:Hooks`. Add a `pub fn is_empty(&self)->bool { self.pre_phase.is_empty() && self.post_phase.is_empty() }` to Hooks (in hooks.rs) for that predicate. Import FirmwareQuirk and Hooks from the ssh_installer::components module (PS-QUIRK-05/PS-HOOK-06). Arch/HostRole already exist in this file (PS-ARCH-07/PS-ROLE-08). Do NOT add include_tpm2_peer — the D2-B clevis peer stays derived from storage_mode==NativeKeystore by the installer (system_setup.rs:722/772), unchanged. No behavior consumes these four fields yet. Bump config.rs header minor.

## Files (expected touch set)

- `crates/uaa-core/src/network/ssh_installer/config.rs`
- `crates/uaa-core/src/network/ssh_installer/components/hooks.rs`

## Re-verify anchors before editing (line numbers/paths drift — grep first)

```bash
grep -n . crates/uaa-core/src/network/ssh_installer/config.rs >/dev/null && echo 'ok: crates/uaa-core/src/network/ssh_installer/config.rs' || echo 'MISSING (new or moved): crates/uaa-core/src/network/ssh_installer/config.rs'
grep -n . crates/uaa-core/src/network/ssh_installer/components/hooks.rs >/dev/null && echo 'ok: crates/uaa-core/src/network/ssh_installer/components/hooks.rs' || echo 'MISSING (new or moved): crates/uaa-core/src/network/ssh_installer/components/hooks.rs'
```
Zero-hit on a file you expected to edit = STOP and report (the code moved).

## Acceptance criteria

- [ ] test test_all_new_axes_omit_when_default: a committed len-serv config (arch defaulted amd64, role install-target, empty quirks/hooks) serializes to byte-identical YAML with none of the four keys present
- [ ] test: a config with arch=arm64, role=tang-server, one firmware_quirk, and one hook DOES serialize all four keys (round-trips)
- [ ] InstallationConfig still #[serde(deny_unknown_fields)] holds; cargo test -p uaa-core passes
- [ ] file-version header bumped; cargo clippy -p uaa-core --all-targets clean
- [ ] `cargo test -p uaa-core -p uaa-control` green for touched crates
- [ ] `cargo clippy --workspace --all-targets` clean
- [ ] File-version headers bumped on **every** changed file (Rust: `// file:/version:/guid:/last-edited:` before `package`/first item; md/yaml/toml comments otherwise)
- [ ] len-serv PlainLuks path stays **byte-identical** unless this brief is an explicit len-serv migration (waves 7–10)

## Commit + PR

Conventional commit; end the body with the repo's Co-Authored-By / Claude-Session trailers.

```
feat(profile): wire arch/role/firmware_quirks/hooks onto installationconfig (additive, byte-identical) (PS-WIRE-AXES-10)
```

Then `gh pr create` → `gh pr merge <n> --rebase`. Clean up: `git -C "$REPO" worktree remove "$REPO/.worktrees/ps-$SLUG"`.
