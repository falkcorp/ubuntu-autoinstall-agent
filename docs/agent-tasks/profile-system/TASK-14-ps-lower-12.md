<!-- file: docs/agent-tasks/profile-system/TASK-14-ps-lower-12.md -->
<!-- version: 1.0.0 -->
<!-- guid: 3648be8d-785e-4c07-b338-a725b03af7d0 -->
<!-- last-edited: 2026-07-23 -->

# TASK-14 — lower(): pure total authoring->flat-wire bridge (PS-LOWER-12)

**Priority:** P1 · **Effort:** L · **Recommended subagent:** Sonnet-class · a self-contained component with its wiring and tests; bounded, moderate judgment · **Depends on:** PS-WIRE-AXES-10 (TASK-12), PS-WIRE-PARTIAL-11 (TASK-13), PS-APP-09 (TASK-01)

**Wave:** 3 · **Workstream:** seam · **Role:** rust-component subagent (implement a component + wiring + tests)

> Part of the **Profile-System conversion** ([README](README.md), [design](../../specs/profile-system-design.md), [current-state](../../specs/profile-system-current-state.md)). Universal protocol + wave/collision rules live in the README — read it first.

## ⛔ START HERE (worktree setup — do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent
SLUG=ps-lower-12
git -C "$REPO" fetch origin
# Base on origin/main. If any "Depends on" brief has merged, its changes are already on main.
git -C "$REPO" worktree add "$REPO/.worktrees/ps-$SLUG" -b "agent/ps-$SLUG" origin/main
cd "$REPO/.worktrees/ps-$SLUG"
git rebase origin/main
```

## Goal

SEAM CONTRACT (apply verbatim): `pub fn lower(resolved:&InstallationConfigPartial) -> InstallationConfig`, pure and total, in a NEW file crates/uaa-core/src/profile/lower.rs (add `pub mod lower;` to profile/mod.rs). Input is a RESOLVED InstallationConfigPartial — the output of merge() BEFORE flattening (i.e. group-defaults already resolved over host-overrides into one fully-populated partial); this brief does not call merge (PS-MERGE-13 will call lower internally). Model on layout.rs::plan_layout (pure planner->applier split; the installer is the applier). Field map (copy the doc-comment tables authored in PS-UNLOCK-02/PS-NET-03/PS-IMG-04): network component -> network_interface/search/nameservers/renderer, and Addressing::Dhcp -> network_address="dhcp"+network_gateway empty, Static{address,gateway} -> network_address=address+network_gateway=gateway; base_image -> debootstrap_release/debootstrap_mirror/initramfs_type; unlock_policy -> tang_servers/tang_threshold/tpm2_pin(double-Option preserved)/tpm2_pcr_ids/enroll_tpm2/expect_fido2; disk_layout by kind -> SingleLuks sets storage_mode=PlainLuks + disk_device, ZfsNativeKeystore sets storage_mode=NativeKeystore + disks; arch/role/firmware_quirks/hooks copy through. When a nested component is None, fall back to the existing flat field on the resolved partial (so a flat-authored host still lowers correctly). DROP (do NOT lower to any wire field, they are inert until PS-INSTALLER-29): disk sizes (esp/reset/bpool), reset_enabled, base_image.fallback_mirror, and unlock_policy.tpm2_clevis_peer (D2-B peer is storage_mode-derived by the installer; tpm2_clevis_peer is validate-only, enforced in PS-VALIDATE-14). MUST preserve tpm2_pin double-Option inherit-vs-explicit-none and MUST leave REPLACE_AT_PLACE_TIME tokens untouched in the secret-bearing fields (tpm2_pin, luks_key, root_password, install_ca_cert). No I/O, no Result. New file at 1.0.0.

## Files (expected touch set)

- `crates/uaa-core/src/profile/lower.rs`
- `crates/uaa-core/src/profile/mod.rs`

## Re-verify anchors before editing (line numbers/paths drift — grep first)

```bash
grep -n . crates/uaa-core/src/profile/lower.rs >/dev/null && echo 'ok: crates/uaa-core/src/profile/lower.rs' || echo 'MISSING (new or moved): crates/uaa-core/src/profile/lower.rs'
grep -n . crates/uaa-core/src/profile/mod.rs >/dev/null && echo 'ok: crates/uaa-core/src/profile/mod.rs' || echo 'MISSING (new or moved): crates/uaa-core/src/profile/mod.rs'
```
Zero-hit on a file you expected to edit = STOP and report (the code moved).

## Acceptance criteria

- [ ] lower() is total (example tests over: all-fields-set, only-required, NativeKeystore disks, flat-only fallback) — no panics on any well-typed input
- [ ] a resolved partial with unlock_policy.tpm2_pin.pin=Some(None) (explicit-none) lowers to InstallationConfig.tpm2_pin==None distinctly from an inherit case (test)
- [ ] a REPLACE_AT_PLACE_TIME token in luks_key/root_password/tpm2_pin/install_ca_cert survives lower() unchanged (test)
- [ ] a component-authored NativeKeystore partial lowers to storage_mode=NativeKeystore + non-empty disks; disk sizes/reset_enabled/fallback_mirror/tpm2_clevis_peer are absent from the InstallationConfig (test)
- [ ] cargo test -p uaa-core passes; file-version headers bumped; cargo clippy -p uaa-core --all-targets clean
- [ ] `cargo test -p uaa-core -p uaa-control` green for touched crates
- [ ] `cargo clippy --workspace --all-targets` clean
- [ ] File-version headers bumped on **every** changed file (Rust: `// file:/version:/guid:/last-edited:` before `package`/first item; md/yaml/toml comments otherwise)
- [ ] len-serv PlainLuks path stays **byte-identical** unless this brief is an explicit len-serv migration (waves 7–10)

## Commit + PR

Conventional commit; end the body with the repo's Co-Authored-By / Claude-Session trailers.

```
feat(profile): lower(): pure total authoring->flat-wire bridge (PS-LOWER-12)
```

Then `gh pr create` → `gh pr merge <n> --rebase`. Clean up: `git -C "$REPO" worktree remove "$REPO/.worktrees/ps-$SLUG"`.
