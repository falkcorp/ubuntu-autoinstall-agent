<!-- file: docs/agent-tasks/profile-system/TASK-15-ps-schema-20.md -->
<!-- version: 1.0.0 -->
<!-- guid: a744f0f0-1d13-4a8c-be4d-c1e3e1c46886 -->
<!-- last-edited: 2026-07-23 -->

# TASK-15 — schema_version row gate + component-aware control binary (expand step) (PS-SCHEMA-20)

**Priority:** P1 · **Effort:** L · **Recommended subagent:** Opus-class · cross-cutting seam / migration touching merge-provenance, rollback safety, or a multi-module refactor · **Depends on:** PS-WIRE-PARTIAL-11 (TASK-13)

**Wave:** 3 · **Workstream:** control-plane · **Role:** rust-architecture subagent (cross-cutting seam / migration / multi-module refactor)

> Part of the **Profile-System conversion** ([README](README.md), [design](../../specs/profile-system-design.md), [current-state](../../specs/profile-system-current-state.md)). Universal protocol + wave/collision rules live in the README — read it first.

## ⛔ START HERE (worktree setup — do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent
SLUG=ps-schema-20
git -C "$REPO" fetch origin
# Base on origin/main. If any "Depends on" brief has merged, its changes are already on main.
git -C "$REPO" worktree add "$REPO/.worktrees/ps-$SLUG" -b "agent/ps-$SLUG" origin/main
cd "$REPO/.worktrees/ps-$SLUG"
git rebase origin/main
```

## Goal

Implement the expand step of expand-then-migrate. Add `schema_version:i64` as a SEPARATE additive column/field on HostGroupRow and HostProfileRow (store.rs) — NOT nested in the defaults/overrides JSON; on read, a missing field defaults to 0. Define `const SCHEMA_VERSION_MAX:i64 = 1;` and make the control binary refuse (fail-loud anyhow::Error, message pattern "schema version {n} exceeds binary max 1") to serve or roll back any row whose schema_version > MAX, WITHOUT attempting JSON deserialization. New rows written this phase carry schema_version=1; existing rows (version 0) are served normally (0<=1). Ensure convert.rs::group_row_to_profile/profile_row_to_profile deserialize the new component keys (disk_layout/unlock_policy/network/base_image/arch/role/firmware_quirks/hooks) into the deny_unknown_fields InstallationConfigPartial (from PS-WIRE-PARTIAL-11) WITHOUT regressing the group-scoped parse-failure behavior: an unknown key must still produce an error naming the group/host, e.g. `group "prod": stored defaults have unknown field 'x'`. This phase migrates ZERO blobs — it only teaches every binary to recognize the new keys and enforce the version floor, so a shared-group component blob can never be served by an older binary. Document the no-rollback-below-X operational rule in a doc comment. Bump headers.

## Files (expected touch set)

- `crates/uaa-control/src/profiles/store.rs`
- `crates/uaa-control/src/profiles/convert.rs`
- `crates/uaa-control/src/profiles/resolve.rs`
- `crates/uaa-control/src/db/mod.rs`

## Re-verify anchors before editing (line numbers/paths drift — grep first)

```bash
grep -n . crates/uaa-control/src/profiles/store.rs >/dev/null && echo 'ok: crates/uaa-control/src/profiles/store.rs' || echo 'MISSING (new or moved): crates/uaa-control/src/profiles/store.rs'
grep -n . crates/uaa-control/src/profiles/convert.rs >/dev/null && echo 'ok: crates/uaa-control/src/profiles/convert.rs' || echo 'MISSING (new or moved): crates/uaa-control/src/profiles/convert.rs'
grep -n . crates/uaa-control/src/profiles/resolve.rs >/dev/null && echo 'ok: crates/uaa-control/src/profiles/resolve.rs' || echo 'MISSING (new or moved): crates/uaa-control/src/profiles/resolve.rs'
grep -n . crates/uaa-control/src/db/mod.rs >/dev/null && echo 'ok: crates/uaa-control/src/db/mod.rs' || echo 'MISSING (new or moved): crates/uaa-control/src/db/mod.rs'
```
Zero-hit on a file you expected to edit = STOP and report (the code moved).

## Acceptance criteria

- [ ] schema_version:i64 stored + read on both row types; missing->0 (test)
- [ ] control refuses a row with schema_version=2 with the fail-loud message, without deserializing the blob (test)
- [ ] convert deserializes a component-bearing blob into InstallationConfigPartial (test)
- [ ] an unknown component key still yields a group/host-scoped parse error (test); no existing blob is mutated by this brief
- [ ] file-version headers bumped; cargo clippy -p uaa-control --all-targets clean
- [ ] `cargo test -p uaa-core -p uaa-control` green for touched crates
- [ ] `cargo clippy --workspace --all-targets` clean
- [ ] File-version headers bumped on **every** changed file (Rust: `// file:/version:/guid:/last-edited:` before `package`/first item; md/yaml/toml comments otherwise)
- [ ] len-serv PlainLuks path stays **byte-identical** unless this brief is an explicit len-serv migration (waves 7–10)

## Commit + PR

Conventional commit; end the body with the repo's Co-Authored-By / Claude-Session trailers.

```
refactor(profile): schema_version row gate + component-aware control binary (expand step) (PS-SCHEMA-20)
```

Then `gh pr create` → `gh pr merge <n> --rebase`. Clean up: `git -C "$REPO" worktree remove "$REPO/.worktrees/ps-$SLUG"`.
