<!-- file: docs/agent-tasks/profile-system/TASK-30-ps-retire-30.md -->
<!-- version: 1.0.0 -->
<!-- guid: 48fe59cb-ad9a-4eeb-a77c-7021c4998376 -->
<!-- last-edited: 2026-07-23 -->

# TASK-30 — retire legacy autoinstall/render.rs subiquity template path (PS-RETIRE-30)

**Priority:** P3 · **Effort:** M · **Recommended subagent:** Sonnet-class · a self-contained component with its wiring and tests; bounded, moderate judgment · **Depends on:** PS-MIG-LEN-DISK-28 (TASK-28)

**Wave:** 11 · **Workstream:** cleanup · **Role:** rust-component subagent (implement a component + wiring + tests)

> Part of the **Profile-System conversion** ([README](README.md), [design](../../specs/profile-system-design.md), [current-state](../../specs/profile-system-current-state.md)). Universal protocol + wave/collision rules live in the README — read it first.

## ⛔ START HERE (worktree setup — do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent
SLUG=ps-retire-30
git -C "$REPO" fetch origin
# Base on origin/main. If any "Depends on" brief has merged, its changes are already on main.
git -C "$REPO" worktree add "$REPO/.worktrees/ps-$SLUG" -b "agent/ps-$SLUG" origin/main
cd "$REPO/.worktrees/ps-$SLUG"
git rebase origin/main
```

## Goal

PRE-FLIGHT (verify before deleting): confirm PS-MIG-LEN-DISK-28 is merged AND all target hosts (len-serv-001/002/003, unimatrixone, rpi-serv-001) are component-authored group/host profiles; and audit that nothing in production reads the rendered user-data path — `grep -rn 'render_user_data\|len-serv.user-data.tmpl\|render::' scripts/ crates/uaa-control/ ` and check live DNS/DHCP/iPXE configs on 172.16.2.30; record findings in the PR before deletion. Then retire the legacy autoinstall/render.rs template (len-serv.user-data.tmpl) + its golden fixtures under tests/fixtures/golden/, plus the len-serv-only HostSpec::for_lenserv place/render-user-data/verify CLI trio (a live second source of truth: SSH keys, Tang URLs, cockroach params duplicated). DECISION: remove the render-user-data/place/verify CLI subcommands (do not re-route them); the component pipeline (`config place --from-registry`, which resolves HostProfile->InstallationConfig via profile merge) is the single source of truth. Delete render.rs, the template, the golden fixtures, and the CLI subcommands; remove for_lenserv if it becomes dead. Ensure the build is green with no dead-code warnings. Bump headers on edited files.

## Files (expected touch set)

- `crates/uaa-core/src/autoinstall/render.rs`
- `crates/uaa-core/src/autoinstall/templates/len-serv.user-data.tmpl`
- `crates/uaa-core/src/autoinstall/host_spec.rs`
- `crates/uaa/src/cli/commands.rs`
- `crates/uaa/src/cli/args.rs`
- `crates/uaa-core/tests/fixtures/golden/`

## Re-verify anchors before editing (line numbers/paths drift — grep first)

```bash
grep -n . crates/uaa-core/src/autoinstall/render.rs >/dev/null && echo 'ok: crates/uaa-core/src/autoinstall/render.rs' || echo 'MISSING (new or moved): crates/uaa-core/src/autoinstall/render.rs'
grep -n . crates/uaa-core/src/autoinstall/templates/len-serv.user-data.tmpl >/dev/null && echo 'ok: crates/uaa-core/src/autoinstall/templates/len-serv.user-data.tmpl' || echo 'MISSING (new or moved): crates/uaa-core/src/autoinstall/templates/len-serv.user-data.tmpl'
grep -n . crates/uaa-core/src/autoinstall/host_spec.rs >/dev/null && echo 'ok: crates/uaa-core/src/autoinstall/host_spec.rs' || echo 'MISSING (new or moved): crates/uaa-core/src/autoinstall/host_spec.rs'
grep -n . crates/uaa/src/cli/commands.rs >/dev/null && echo 'ok: crates/uaa/src/cli/commands.rs' || echo 'MISSING (new or moved): crates/uaa/src/cli/commands.rs'
grep -n . crates/uaa/src/cli/args.rs >/dev/null && echo 'ok: crates/uaa/src/cli/args.rs' || echo 'MISSING (new or moved): crates/uaa/src/cli/args.rs'
```
Zero-hit on a file you expected to edit = STOP and report (the code moved).

## Acceptance criteria

- [ ] pre-flight audit recorded in the PR (hosts component-authored; no production consumer of render path)
- [ ] render.rs template path + golden fixtures removed (grep clean for render_user_data / len-serv.user-data.tmpl)
- [ ] render-user-data/place/verify CLI subcommands removed; documented that config place --from-registry is the path
- [ ] cargo build + cargo test pass with no dead-code warnings; file-version headers bumped; cargo clippy --all-targets clean
- [ ] `cargo test -p uaa-core -p uaa-control` green for touched crates
- [ ] `cargo clippy --workspace --all-targets` clean
- [ ] File-version headers bumped on **every** changed file (Rust: `// file:/version:/guid:/last-edited:` before `package`/first item; md/yaml/toml comments otherwise)
- [ ] len-serv PlainLuks path stays **byte-identical** unless this brief is an explicit len-serv migration (waves 7–10)

## Commit + PR

Conventional commit; end the body with the repo's Co-Authored-By / Claude-Session trailers.

```
feat(profile): retire legacy autoinstall/render.rs subiquity template path (PS-RETIRE-30)
```

Then `gh pr create` → `gh pr merge <n> --rebase`. Clean up: `git -C "$REPO" worktree remove "$REPO/.worktrees/ps-$SLUG"`.
