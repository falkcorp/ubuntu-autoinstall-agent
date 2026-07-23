<!-- file: docs/agent-tasks/profile-system/TASK-17-ps-validate-14.md -->
<!-- version: 1.0.0 -->
<!-- guid: 50015f47-fa4a-470f-b954-5219ca1c29d4 -->
<!-- last-edited: 2026-07-23 -->

# TASK-17 — validate_resolved(&InstallationConfig) composition-legality sibling (PS-VALIDATE-14)

**Priority:** P1 · **Effort:** M · **Recommended subagent:** Sonnet-class · a self-contained component with its wiring and tests; bounded, moderate judgment · **Depends on:** PS-WIRE-AXES-10 (TASK-12)

**Wave:** 3 · **Workstream:** seam · **Role:** rust-component subagent (implement a component + wiring + tests)

> Part of the **Profile-System conversion** ([README](README.md), [design](../../specs/profile-system-design.md), [current-state](../../specs/profile-system-current-state.md)). Universal protocol + wave/collision rules live in the README — read it first.

## ⛔ START HERE (worktree setup — do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent
SLUG=ps-validate-14
git -C "$REPO" fetch origin
# Base on origin/main. If any "Depends on" brief has merged, its changes are already on main.
git -C "$REPO" worktree add "$REPO/.worktrees/ps-$SLUG" -b "agent/ps-$SLUG" origin/main
cd "$REPO/.worktrees/ps-$SLUG"
git rebase origin/main
```

## Goal

Add a NEW post-merge entry point `pub fn validate_resolved(cfg:&InstallationConfig) -> Result<()>` in validate.rs, sibling to the existing pre-merge validate(groups,profiles) (do NOT modify that 439-line function beyond adding this one). Fields are read off the wired InstallationConfig from PS-WIRE-AXES-10 (arch:Arch, role:HostRole, firmware_quirks:Vec<FirmwareQuirk>, storage_mode:StorageMode, tang_servers, tang_threshold, enroll_tpm2, disks, applications). Rules, stated concretely: (1) if storage_mode==NativeKeystore then disks non-empty AND arch==Amd64; (2) if tang_servers non-empty then 1<=tang_threshold<=tang_servers.len(); (3) tpm2_clevis_peer legality is expressed as: an authored NativeKeystore is the ONLY config permitted to use the D2-B clevis peer — since tpm2_clevis_peer is not a wire field, enforce the surrogate rule here: storage_mode==NativeKeystore is required for the peer path, and cross-check is handled at authoring merge time (accept this brief validates on the resolved wire form only); (4) if arch==Arm64 then firmware_quirks must NOT contain GrubRemovableFallback; (5) if role==TangServer: permit empty disks + empty unlock (tang_servers may be empty) but require an ApplicationSpec::TangServer in applications; if role==InstallTarget: require a storage_mode disk plan (disk_device or disks non-empty) AND a non-empty unlock (tang_servers non-empty OR enroll_tpm2). Also OPTIONAL/RECOMMENDED per advisor: fail-loud if a resolved config somehow carries non-default disk sizes/reset_enabled (they are unsupported until PS-INSTALLER-29) — but since those are not wire fields this reduces to a no-op today; add a doc comment noting the guard belongs in lower/merge once sizes become wire fields. Return descriptive errors. Bump validate.rs header.

## Files (expected touch set)

- `crates/uaa-core/src/profile/validate.rs`

## Re-verify anchors before editing (line numbers/paths drift — grep first)

```bash
grep -n . crates/uaa-core/src/profile/validate.rs >/dev/null && echo 'ok: crates/uaa-core/src/profile/validate.rs' || echo 'MISSING (new or moved): crates/uaa-core/src/profile/validate.rs'
```
Zero-hit on a file you expected to edit = STOP and report (the code moved).

## Acceptance criteria

- [ ] unit tests cover each of rules 1,2,4,5 with both a passing and a failing case, asserting the error message text
- [ ] rule 5: role=TangServer without a TangServer application fails; role=InstallTarget with empty unlock fails
- [ ] existing validate(groups,profiles) tests untouched and passing
- [ ] file-version header bumped; cargo clippy -p uaa-core --all-targets clean
- [ ] `cargo test -p uaa-core -p uaa-control` green for touched crates
- [ ] `cargo clippy --workspace --all-targets` clean
- [ ] File-version headers bumped on **every** changed file (Rust: `// file:/version:/guid:/last-edited:` before `package`/first item; md/yaml/toml comments otherwise)
- [ ] len-serv PlainLuks path stays **byte-identical** unless this brief is an explicit len-serv migration (waves 7–10)

## Commit + PR

Conventional commit; end the body with the repo's Co-Authored-By / Claude-Session trailers.

```
feat(profile): validate_resolved(&installationconfig) composition-legality sibling (PS-VALIDATE-14)
```

Then `gh pr create` → `gh pr merge <n> --rebase`. Clean up: `git -C "$REPO" worktree remove "$REPO/.worktrees/ps-$SLUG"`.
