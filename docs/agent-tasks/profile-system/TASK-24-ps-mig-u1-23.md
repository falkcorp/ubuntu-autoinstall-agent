<!-- file: docs/agent-tasks/profile-system/TASK-24-ps-mig-u1-23.md -->
<!-- version: 1.0.0 -->
<!-- guid: 7c90947c-1025-476f-b8cd-932ffaf8207d -->
<!-- last-edited: 2026-07-23 -->

# TASK-24 — migrate unimatrixone (U1) to component authoring (PS-MIG-U1-23)

**Priority:** P2 · **Effort:** M · **Recommended subagent:** Sonnet-class · a self-contained component with its wiring and tests; bounded, moderate judgment · **Depends on:** PS-PIPELINE-21 (TASK-21), PS-GATE-15 (TASK-20), PS-PLACEHOLDER-22 (TASK-22), PS-VALIDATE-14 (TASK-17)

**Wave:** 6 · **Workstream:** host-migration · **Role:** rust-component subagent (implement a component + wiring + tests)

> Part of the **Profile-System conversion** ([README](README.md), [design](../../specs/profile-system-design.md), [current-state](../../specs/profile-system-current-state.md)). Universal protocol + wave/collision rules live in the README — read it first.

## ⛔ START HERE (worktree setup — do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent
SLUG=ps-mig-u1-23
git -C "$REPO" fetch origin
# Base on origin/main. If any "Depends on" brief has merged, its changes are already on main.
git -C "$REPO" worktree add "$REPO/.worktrees/ps-$SLUG" -b "agent/ps-$SLUG" origin/main
cd "$REPO/.worktrees/ps-$SLUG"
git rebase origin/main
```

## Goal

Re-author unimatrixone as a standalone single-host group+profile using components: disk_layout=ZfsNativeKeystore (the existing 4-disk System/Special roster copied from the committed unimatrixone.yaml disks list) + unlock_policy(tang with the committed servers and threshold=2, tpm2_pin.enroll=false, tpm2_clevis_peer=true, fido2_expected per committed). U1 is a standalone group so rollback blast radius is one host and it is EXPLICITLY allowed to change its own placed artifact (unlike len-serv). REFERENCE PATTERN: this brief ESTABLISHES the concrete 'component-in-group-defaults' authoring shape that PS-MIG-LEN-* reuse — put the group defaults (disk_layout+unlock_policy blocks) and the single host override in the fixture, and document the shape in a header comment. Deliverables: (a) update examples/configs/install/unimatrixone.yaml header version 3.1.0->4.0.0 and, IF U1's registry seed is a hand-authored group/profile pair, add it under the same fixture convention as PS-GATE-15 (crates/uaa-core/tests/fixtures/components/unimatrixone.yaml already created there — reuse it as the canonical component authoring); (b) mark its stored row schema_version=1 (the field from PS-SCHEMA-20). Gates: (1) EQUALITY — merge(parse(component-unimatrixone)) == the committed unimatrixone InstallationConfig by STRUCT equality (the component_equality_gate test from PS-GATE-15; run `cargo test -p uaa-core component_equality_gate`); tpm2_clevis_peer is authored but does NOT appear in the lowered config (peer is storage_mode-derived). (2) validate_resolved passes. (3) placeholder-survival helper (PS-PLACEHOLDER-22) run for keystore luks_key + tpm2_pin. (4) D2-B VM GATE — run scripts/vm-validate.sh on the SERVER (172.16.2.30) or another amd64 KVM Linux box (macOS has no KVM): `sudo ./scripts/vm-validate.sh --iso <ssh-ready.iso> --agent <musl-agent> --config examples/configs/install/unimatrixone.yaml`; hardware power-on is NOT in scope. Bump headers.

## Files (expected touch set)

- `examples/configs/install/unimatrixone.yaml`
- `crates/uaa-core/tests/fixtures/components/unimatrixone.yaml`
- `crates/uaa-control/src/profiles/reify.rs`

## Re-verify anchors before editing (line numbers/paths drift — grep first)

```bash
grep -n . examples/configs/install/unimatrixone.yaml >/dev/null && echo 'ok: examples/configs/install/unimatrixone.yaml' || echo 'MISSING (new or moved): examples/configs/install/unimatrixone.yaml'
grep -n . crates/uaa-core/tests/fixtures/components/unimatrixone.yaml >/dev/null && echo 'ok: crates/uaa-core/tests/fixtures/components/unimatrixone.yaml' || echo 'MISSING (new or moved): crates/uaa-core/tests/fixtures/components/unimatrixone.yaml'
grep -n . crates/uaa-control/src/profiles/reify.rs >/dev/null && echo 'ok: crates/uaa-control/src/profiles/reify.rs' || echo 'MISSING (new or moved): crates/uaa-control/src/profiles/reify.rs'
```
Zero-hit on a file you expected to edit = STOP and report (the code moved).

## Acceptance criteria

- [ ] component_equality_gate passes for unimatrixone (merge->lower struct-equals committed); tpm2_clevis_peer absent from lowered config
- [ ] validate_resolved passes for the re-authored profile
- [ ] placeholder-survival passes for keystore luks_key + tpm2_pin
- [ ] D2-B VM gate passes on the server (command + host recorded in the PR; NOT run on macOS)
- [ ] row schema_version=1; file-version headers bumped (unimatrixone.yaml -> 4.0.0); cargo clippy --all-targets clean
- [ ] `cargo test -p uaa-core -p uaa-control` green for touched crates
- [ ] `cargo clippy --workspace --all-targets` clean
- [ ] File-version headers bumped on **every** changed file (Rust: `// file:/version:/guid:/last-edited:` before `package`/first item; md/yaml/toml comments otherwise)
- [ ] len-serv PlainLuks path stays **byte-identical** unless this brief is an explicit len-serv migration (waves 7–10)

## Commit + PR

Conventional commit; end the body with the repo's Co-Authored-By / Claude-Session trailers.

```
feat(profile): migrate unimatrixone (u1) to component authoring (PS-MIG-U1-23)
```

Then `gh pr create` → `gh pr merge <n> --rebase`. Clean up: `git -C "$REPO" worktree remove "$REPO/.worktrees/ps-$SLUG"`.
