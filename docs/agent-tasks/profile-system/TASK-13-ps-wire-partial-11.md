<!-- file: docs/agent-tasks/profile-system/TASK-13-ps-wire-partial-11.md -->
<!-- version: 1.0.0 -->
<!-- guid: 0eba4d18-984d-405a-a702-327cc3138393 -->
<!-- last-edited: 2026-07-23 -->

# TASK-13 — wire component sub-structs onto InstallationConfigPartial (additive) (PS-WIRE-PARTIAL-11)

**Priority:** P1 · **Effort:** M · **Recommended subagent:** Sonnet-class · a self-contained component with its wiring and tests; bounded, moderate judgment · **Depends on:** PS-DISK-01 (TASK-03), PS-UNLOCK-02 (TASK-10), PS-NET-03 (TASK-07), PS-IMG-04 (TASK-05), PS-QUIRK-05 (TASK-08), PS-HOOK-06 (TASK-04), PS-ARCH-07 (TASK-02), PS-ROLE-08 (TASK-09)

**Wave:** 2 · **Workstream:** wire-integration · **Role:** rust-component subagent (implement a component + wiring + tests)

> Part of the **Profile-System conversion** ([README](README.md), [design](../../specs/profile-system-design.md), [current-state](../../specs/profile-system-current-state.md)). Universal protocol + wave/collision rules live in the README — read it first.

## ⛔ START HERE (worktree setup — do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent
SLUG=ps-wire-partial-11
git -C "$REPO" fetch origin
# Base on origin/main. If any "Depends on" brief has merged, its changes are already on main.
git -C "$REPO" worktree add "$REPO/.worktrees/ps-$SLUG" -b "agent/ps-$SLUG" origin/main
cd "$REPO/.worktrees/ps-$SLUG"
git rebase origin/main
```

## Goal

Add four nested authoring fields to InstallationConfigPartial in profile/mod.rs, ADDITIVELY — the existing flat fields (network_interface/address/gateway/search/nameservers/renderer, disk_device, storage_mode, disks, tang_servers, tang_threshold, enroll_tpm2, tpm2_pin, tpm2_pcr_ids, expect_fido2, debootstrap_release/mirror, initramfs_type, luks_key, applications) are RETAINED; merge/lower reconcile both and are deferred to PS-MERGE-13/PS-LOWER-12. Add: `disk_layout:Option<DiskLayoutPartial>` (variant partials from PS-DISK-01; wrap the two per-variant partials in a small authoring enum `DiskLayoutPartial { SingleLuks(SingleLuksSpecPartial), ZfsNativeKeystore(NativeKeystoreSpecPartial) }` tag="kind" if not already provided by PS-DISK-01 — coordinate: if PS-DISK-01 only shipped the specs, define the partial-select enum here), `unlock_policy:Option<UnlockPolicyPartial>`, `network:Option<NetworkConfigPartial>`, `base_image:Option<BaseImagePartial>`. Keep `#[serde(default, skip_serializing_if="Option::is_none")]` on each. Also expose authoring for the wire axes on the partial: `arch:Option<Arch>`, `role:Option<HostRole>`, `firmware_quirks:Option<Vec<FirmwareQuirk>>`, `hooks:Option<Hooks>`. Keep the struct-level `#[serde(deny_unknown_fields, default)]`. EXTEND the manual `impl PartialEq for InstallationConfigPartial` (mod.rs:103) to compare the new nested fields with `==` (all their inner types now derive PartialEq, since PS-UNLOCK-02 made TangServer derive PartialEq); do NOT change the existing hand-written tang_servers comparison. Update test_partial_all_none_is_legal (mod.rs:205) to include the new Option fields as None — this is a required edit, not a regression. No merge/lower logic here. Bump mod.rs header minor.

## Files (expected touch set)

- `crates/uaa-core/src/profile/mod.rs`

## Re-verify anchors before editing (line numbers/paths drift — grep first)

```bash
grep -n . crates/uaa-core/src/profile/mod.rs >/dev/null && echo 'ok: crates/uaa-core/src/profile/mod.rs' || echo 'MISSING (new or moved): crates/uaa-core/src/profile/mod.rs'
```
Zero-hit on a file you expected to edit = STOP and report (the code moved).

## Acceptance criteria

- [ ] InstallationConfigPartial deserializes a YAML carrying disk_layout/unlock_policy/network/base_image/arch/role/firmware_quirks/hooks nested blocks (test)
- [ ] an unknown top-level key still errors (deny_unknown_fields test, e.g. test_partial_rejects_unknown_field)
- [ ] test_partial_all_none_is_legal updated to include the new fields and passes; test_tpm2_pin_distinguishes_inherit_from_explicit_none and test_partial_roundtrips_applications still pass
- [ ] the manual PartialEq compiles and compares the new nested fields (a differing unlock_policy makes two partials unequal — test)
- [ ] file-version header bumped; cargo clippy -p uaa-core --all-targets clean
- [ ] `cargo test -p uaa-core -p uaa-control` green for touched crates
- [ ] `cargo clippy --workspace --all-targets` clean
- [ ] File-version headers bumped on **every** changed file (Rust: `// file:/version:/guid:/last-edited:` before `package`/first item; md/yaml/toml comments otherwise)
- [ ] len-serv PlainLuks path stays **byte-identical** unless this brief is an explicit len-serv migration (waves 7–10)

## Commit + PR

Conventional commit; end the body with the repo's Co-Authored-By / Claude-Session trailers.

```
feat(profile): wire component sub-structs onto installationconfigpartial (additive) (PS-WIRE-PARTIAL-11)
```

Then `gh pr create` → `gh pr merge <n> --rebase`. Clean up: `git -C "$REPO" worktree remove "$REPO/.worktrees/ps-$SLUG"`.
