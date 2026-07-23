<!-- file: docs/agent-tasks/profile-system/TASK-18-ps-merge-13.md -->
<!-- version: 1.0.0 -->
<!-- guid: 196f889c-c606-49c1-925b-bf0e1a745e57 -->
<!-- last-edited: 2026-07-23 -->

# TASK-18 — merge(): component resolvers + component-path provenance (additive) (PS-MERGE-13)

**Priority:** P1 · **Effort:** L · **Recommended subagent:** Opus-class · cross-cutting seam / migration touching merge-provenance, rollback safety, or a multi-module refactor · **Depends on:** PS-WIRE-PARTIAL-11 (TASK-13), PS-LOWER-12 (TASK-14)

**Wave:** 4 · **Workstream:** seam · **Role:** rust-architecture subagent (cross-cutting seam / migration / multi-module refactor)

> Part of the **Profile-System conversion** ([README](README.md), [design](../../specs/profile-system-design.md), [current-state](../../specs/profile-system-current-state.md)). Universal protocol + wave/collision rules live in the README — read it first.

## ⛔ START HERE (worktree setup — do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent
SLUG=ps-merge-13
git -C "$REPO" fetch origin
# Base on origin/main. If any "Depends on" brief has merged, its changes are already on main.
git -C "$REPO" worktree add "$REPO/.worktrees/ps-$SLUG" -b "agent/ps-$SLUG" origin/main
cd "$REPO/.worktrees/ps-$SLUG"
git rebase origin/main
```

## Goal

SEAM CONTRACT: merge KEEPS its signature `pub fn merge(group:&HostGroupProfile, host:&HostProfile) -> Result<(InstallationConfig, Provenance)>` (merge.rs:228). Internally: (1) resolve group-defaults over host-overrides into a single RESOLVED InstallationConfigPartial (component + flat fields), then (2) return `Ok((lower(&resolved), provenance))` calling PS-LOWER-12's lower(). This preserves the two live callers in resolve.rs:81/98 unchanged. Component resolution rules: variant-select components (disk_layout, applications, firmware_quirks) use whole-replace-by-kind — if the host authors a disk_layout of a different kind than the group's, the host's wholly replaces it; generalize the union_applications precedent. Field-components (unlock_policy, network, base_image) merge each factor independently via the existing resolve_required/resolve_defaulted/resolve_double_option helpers; within a same-kind disk_layout variant, a host may override a single spec field (field-partial), following the merge_cockroach precedent at merge.rs:196. PROVENANCE (resolve the contradiction): keep the existing FLAT keys (disk_device, network_renderer, tpm2_pin, ...) exactly as the current tests at merge.rs:522/546/589/601 assert them — do NOT rewrite those assertions — and ADD component-path keys (e.g. "unlock-policy.tang.threshold") ADDITIVELY alongside them. Do NOT change the 10-field fail-closed set or the two-tier structure. Bump merge.rs/mod.rs headers.

## Files (expected touch set)

- `crates/uaa-core/src/profile/merge.rs`
- `crates/uaa-core/src/profile/mod.rs`

## Re-verify anchors before editing (line numbers/paths drift — grep first)

```bash
grep -n . crates/uaa-core/src/profile/merge.rs >/dev/null && echo 'ok: crates/uaa-core/src/profile/merge.rs' || echo 'MISSING (new or moved): crates/uaa-core/src/profile/merge.rs'
grep -n . crates/uaa-core/src/profile/mod.rs >/dev/null && echo 'ok: crates/uaa-core/src/profile/mod.rs' || echo 'MISSING (new or moved): crates/uaa-core/src/profile/mod.rs'
```
Zero-hit on a file you expected to edit = STOP and report (the code moved).

## Acceptance criteria

- [ ] merge signature unchanged; both resolve.rs callers compile untouched; the M2 gate test_resolved_equals_committed_by_struct_equality still passes for all committed hosts
- [ ] NEW test: host disk_layout of kind ZfsNativeKeystore wholly replaces a group disk_layout of kind SingleLuks
- [ ] NEW test: within same-kind disk_layout, host overrides one spec field (field-partial) and other fields inherit from group
- [ ] provenance contains BOTH the existing flat keys (unchanged assertions) AND at least two new component-path keys (asserted)
- [ ] existing merge tests + the fail-closed set unchanged and passing; headers bumped; cargo clippy -p uaa-core --all-targets clean
- [ ] `cargo test -p uaa-core -p uaa-control` green for touched crates
- [ ] `cargo clippy --workspace --all-targets` clean
- [ ] File-version headers bumped on **every** changed file (Rust: `// file:/version:/guid:/last-edited:` before `package`/first item; md/yaml/toml comments otherwise)
- [ ] len-serv PlainLuks path stays **byte-identical** unless this brief is an explicit len-serv migration (waves 7–10)

## Commit + PR

Conventional commit; end the body with the repo's Co-Authored-By / Claude-Session trailers.

```
refactor(profile): merge(): component resolvers + component-path provenance (additive) (PS-MERGE-13)
```

Then `gh pr create` → `gh pr merge <n> --rebase`. Clean up: `git -C "$REPO" worktree remove "$REPO/.worktrees/ps-$SLUG"`.
