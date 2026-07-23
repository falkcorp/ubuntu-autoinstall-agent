<!-- file: docs/agent-tasks/profile-system/TASK-23-ps-mig-rpi-24.md -->
<!-- version: 1.0.0 -->
<!-- guid: 7a010ce0-666d-4642-8199-a2745040a621 -->
<!-- last-edited: 2026-07-23 -->

# TASK-23 — author rpi-serv group (expressibility + validation only) (PS-MIG-RPI-24)

**Priority:** P2 · **Effort:** M · **Recommended subagent:** Sonnet-class · a self-contained component with its wiring and tests; bounded, moderate judgment · **Depends on:** PS-VALIDATE-14 (TASK-17), PS-APP-09 (TASK-01), PS-WIRE-AXES-10 (TASK-12), PS-MERGE-13 (TASK-18), PS-MIG-U1-23 (TASK-24)

**Wave:** 6 · **Workstream:** host-migration · **Role:** rust-component subagent (implement a component + wiring + tests)

> Part of the **Profile-System conversion** ([README](README.md), [design](../../specs/profile-system-design.md), [current-state](../../specs/profile-system-current-state.md)). Universal protocol + wave/collision rules live in the README — read it first.

## ⛔ START HERE (worktree setup — do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent
SLUG=ps-mig-rpi-24
git -C "$REPO" fetch origin
# Base on origin/main. If any "Depends on" brief has merged, its changes are already on main.
git -C "$REPO" worktree add "$REPO/.worktrees/ps-$SLUG" -b "agent/ps-$SLUG" origin/main
cd "$REPO/.worktrees/ps-$SLUG"
git rebase origin/main
```

## Goal

Author a net-new rpi-serv group using the component shape established by PS-MIG-U1-23: arch=arm64, role=tang-server, base_image=(ubuntu arm64 mirror + initramfs=initramfs-tools), applications=[{kind:tang-server, port:80, key-directory:/etc/tang/keys}], firmware_quirks=[{kind:watchdog-staggered, slot:0, interval_secs:60}] (the PS-QUIRK-05 stub). EXPLICIT SCOPE: this delivers authoring/validation EXPRESSIBILITY + validate_resolved() coverage ONLY — NOT a bootable arm64 install (the x86-assumption audit of dracut/GRUB/tpm2 chroot phases is out of scope, tracked as an open question). Does NOT touch len-serv or U1. Deliverables: examples/configs/install/rpi-serv-001.yaml (arm64 tang-server group) + a Rust test module. Put the test at crates/uaa-core/tests/rpi_group.rs (integration test): deserialize the group+profile, resolve through merge, and (a) assert validate_resolved passes for the tang-server role (empty disks/unlock permitted because a tang-server application is present), (b) assert validate_resolved REJECTS a variant of the rpi config that adds firmware_quirks=[grub-removable-fallback] (rule 4: arm64 forbids GrubRemovableFallback) with the expected error message. Add a README/doc-comment noting expressibility-only. New files at 1.0.0.

## Files (expected touch set)

- `examples/configs/install/rpi-serv-001.yaml`
- `crates/uaa-core/tests/rpi_group.rs`

## Re-verify anchors before editing (line numbers/paths drift — grep first)

```bash
grep -n . examples/configs/install/rpi-serv-001.yaml >/dev/null && echo 'ok: examples/configs/install/rpi-serv-001.yaml' || echo 'MISSING (new or moved): examples/configs/install/rpi-serv-001.yaml'
grep -n . crates/uaa-core/tests/rpi_group.rs >/dev/null && echo 'ok: crates/uaa-core/tests/rpi_group.rs' || echo 'MISSING (new or moved): crates/uaa-core/tests/rpi_group.rs'
```
Zero-hit on a file you expected to edit = STOP and report (the code moved).

## Acceptance criteria

- [ ] rpi-serv group deserializes and resolves through merge (test)
- [ ] validate_resolved passes for the rpi tang-server group AND rejects the same group with a grub-removable-fallback quirk added (test asserts the error)
- [ ] doc comment / README notes this is expressibility-only, not a bootable arm64 install
- [ ] file-version headers on new files (1.0.0); cargo clippy -p uaa-core --all-targets clean
- [ ] `cargo test -p uaa-core -p uaa-control` green for touched crates
- [ ] `cargo clippy --workspace --all-targets` clean
- [ ] File-version headers bumped on **every** changed file (Rust: `// file:/version:/guid:/last-edited:` before `package`/first item; md/yaml/toml comments otherwise)
- [ ] len-serv PlainLuks path stays **byte-identical** unless this brief is an explicit len-serv migration (waves 7–10)

## Commit + PR

Conventional commit; end the body with the repo's Co-Authored-By / Claude-Session trailers.

```
feat(profile): author rpi-serv group (expressibility + validation only) (PS-MIG-RPI-24)
```

Then `gh pr create` → `gh pr merge <n> --rebase`. Clean up: `git -C "$REPO" worktree remove "$REPO/.worktrees/ps-$SLUG"`.
