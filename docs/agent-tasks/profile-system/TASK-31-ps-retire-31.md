<!-- file: docs/agent-tasks/profile-system/TASK-31-ps-retire-31.md -->
<!-- version: 1.0.0 -->
<!-- guid: deb22feb-6676-411d-a1b6-f5cc63d69bb9 -->
<!-- last-edited: 2026-07-23 -->

# TASK-31 — remove dead image/deployer + TargetConfig + legacy Architecture enum + cascade (PS-RETIRE-31)

**Priority:** P3 · **Effort:** M · **Recommended subagent:** Sonnet-class · a self-contained component with its wiring and tests; bounded, moderate judgment · **Depends on:** PS-RETIRE-30 (TASK-30)

**Wave:** 12 · **Workstream:** cleanup · **Role:** rust-component subagent (implement a component + wiring + tests)

> Part of the **Profile-System conversion** ([README](README.md), [design](../../specs/profile-system-design.md), [current-state](../../specs/profile-system-current-state.md)). Universal protocol + wave/collision rules live in the README — read it first.

## ⛔ START HERE (worktree setup — do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent
SLUG=ps-retire-31
git -C "$REPO" fetch origin
# Base on origin/main. If any "Depends on" brief has merged, its changes are already on main.
git -C "$REPO" worktree add "$REPO/.worktrees/ps-$SLUG" -b "agent/ps-$SLUG" origin/main
cd "$REPO/.worktrees/ps-$SLUG"
git rebase origin/main
```

## Goal

Remove the dead golden-image build+SSH-deploy pipeline and the legacy Architecture enum it owns. SCOPE DECISION (advisor — removing Architecture forces touching the ImageBuilder/ImageManager/ImageSpec cascade because ListImages/Cleanup/Validate use them): first determine whether the golden-image BUILD pipeline is also dead. Given no committed host uses it (all provision via the component pipeline), retire the WHOLE pipeline: delete image/deployer.rs, image/customizer.rs (no-op stub), config/target.rs (TargetConfig), config/image.rs (ImageSpec) and image/builder/*.rs + image/manager.rs IF ListImages/Cleanup/Validate are being removed too — else keep builder/manager and migrate their Architecture references to ssh_installer::config::Arch. DEFAULT: remove the CreateImage/Deploy CLI subcommands and, if ListImages/Cleanup/Validate have no live users, remove them and their ArchArg->Architecture filter path; otherwise convert ArchArg to map to ssh_installer::config::Arch. Also remove config/loader.rs::load_target_config + its TargetConfig import, and the TargetConfig imports/fixtures in tests/integration_test.rs. Finally delete crate::config::Architecture (config/mod.rs:20). Verify ssh_installer::config::Arch is the only architecture concept left. Bump headers.

## Files (expected touch set)

- `crates/uaa-core/src/image/deployer.rs`
- `crates/uaa-core/src/image/customizer.rs`
- `crates/uaa-core/src/config/target.rs`
- `crates/uaa-core/src/config/image.rs`
- `crates/uaa-core/src/config/loader.rs`
- `crates/uaa-core/src/config/mod.rs`
- `crates/uaa-core/tests/integration_test.rs`
- `crates/uaa/src/cli/args.rs`
- `crates/uaa/src/cli/commands.rs`

## Re-verify anchors before editing (line numbers/paths drift — grep first)

```bash
grep -n . crates/uaa-core/src/image/deployer.rs >/dev/null && echo 'ok: crates/uaa-core/src/image/deployer.rs' || echo 'MISSING (new or moved): crates/uaa-core/src/image/deployer.rs'
grep -n . crates/uaa-core/src/image/customizer.rs >/dev/null && echo 'ok: crates/uaa-core/src/image/customizer.rs' || echo 'MISSING (new or moved): crates/uaa-core/src/image/customizer.rs'
grep -n . crates/uaa-core/src/config/target.rs >/dev/null && echo 'ok: crates/uaa-core/src/config/target.rs' || echo 'MISSING (new or moved): crates/uaa-core/src/config/target.rs'
grep -n . crates/uaa-core/src/config/image.rs >/dev/null && echo 'ok: crates/uaa-core/src/config/image.rs' || echo 'MISSING (new or moved): crates/uaa-core/src/config/image.rs'
grep -n . crates/uaa-core/src/config/loader.rs >/dev/null && echo 'ok: crates/uaa-core/src/config/loader.rs' || echo 'MISSING (new or moved): crates/uaa-core/src/config/loader.rs'
grep -n . crates/uaa-core/src/config/mod.rs >/dev/null && echo 'ok: crates/uaa-core/src/config/mod.rs' || echo 'MISSING (new or moved): crates/uaa-core/src/config/mod.rs'
grep -n . crates/uaa-core/tests/integration_test.rs >/dev/null && echo 'ok: crates/uaa-core/tests/integration_test.rs' || echo 'MISSING (new or moved): crates/uaa-core/tests/integration_test.rs'
grep -n . crates/uaa/src/cli/args.rs >/dev/null && echo 'ok: crates/uaa/src/cli/args.rs' || echo 'MISSING (new or moved): crates/uaa/src/cli/args.rs'
grep -n . crates/uaa/src/cli/commands.rs >/dev/null && echo 'ok: crates/uaa/src/cli/commands.rs' || echo 'MISSING (new or moved): crates/uaa/src/cli/commands.rs'
```
Zero-hit on a file you expected to edit = STOP and report (the code moved).

## Acceptance criteria

- [ ] image/deployer.rs, customizer.rs, TargetConfig, and the legacy crate::config::Architecture removed (grep clean); ImageBuilder/ImageManager fate decided and executed (removed or migrated to Arch) with rationale in the PR
- [ ] CreateImage/Deploy CLI subcommands removed; ListImages/Cleanup/Validate either removed or their arch filter migrated to ssh_installer::config::Arch
- [ ] load_target_config + integration_test.rs TargetConfig fixtures removed
- [ ] cargo build && cargo test --all && cargo clippy --all-targets pass with no dead-code warnings; only ssh_installer::config::Arch remains as the arch concept (grep-verified)
- [ ] file-version headers bumped
- [ ] `cargo test -p uaa-core -p uaa-control` green for touched crates
- [ ] `cargo clippy --workspace --all-targets` clean
- [ ] File-version headers bumped on **every** changed file (Rust: `// file:/version:/guid:/last-edited:` before `package`/first item; md/yaml/toml comments otherwise)
- [ ] len-serv PlainLuks path stays **byte-identical** unless this brief is an explicit len-serv migration (waves 7–10)

## Commit + PR

Conventional commit; end the body with the repo's Co-Authored-By / Claude-Session trailers.

```
feat(profile): remove dead image/deployer + targetconfig + legacy architecture enum + cascade (PS-RETIRE-31)
```

Then `gh pr create` → `gh pr merge <n> --rebase`. Clean up: `git -C "$REPO" worktree remove "$REPO/.worktrees/ps-$SLUG"`.
