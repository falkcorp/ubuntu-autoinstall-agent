<!-- file: docs/agent-tasks/profiles/TASK-03-validation.md -->
<!-- version: 1.0.0 -->
<!-- guid: 08b84007-817f-4e31-9e6b-47f49130fd3d -->
<!-- last-edited: 2026-07-16 -->

# TASK-03 — Validation: global hostname uniqueness, immutability, standalone rules (DS-PRF-03)

**Priority:** P1 · **Effort:** M · **Recommended subagent:** Sonnet-class · rust-core subagent · **Why:** prefix uniqueness is necessary-but-not-sufficient; the real invariant is global hostname uniqueness, and getting it wrong lets two machines claim one name. · **Depends on:** DS-PRF-01 (fills its `validate.rs` stub)

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/profiles-validation" -b agent/profiles-validation origin/main
cd "$REPO/.worktrees/profiles-validation"
git rebase origin/main
```

(Protocol also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

**Wave gate:** DS-PRF-01 must be merged. If `grep -n "pub struct HostGroupProfile" crates/uaa-core/src/profile/mod.rs` returns 0 hits, the gate is not met: STOP and report.

## Goal

Fill the DS-PRF-01 stub `crates/uaa-core/src/profile/validate.rs` with pure, fail-closed validation over a set of groups + profiles. Every rule returns a **named** error.

**Own only `validate.rs`.** Do not edit `mod.rs` (DS-PRF-01 owns it) or `merge.rs` (DS-PRF-02 owns it) — sibling tasks, same wave.

REUSE — do not invent parallels:

- **`HostGroupProfile` / `HostProfile`** from `profile/mod.rs` (DS-PRF-01) — import, never redefine.
- **`crate::error::AutoInstallError::ConfigError`** for every rejection. Do NOT add a new error type.
- Pure functions over slices — no store, no file, no async. This mirrors `host_spec.rs`'s pure/impure split (verify: `grep -n "pub fn compute_join" crates/uaa-core/src/autoinstall/host_spec.rs` — a pure fn unit-tested in isolation).

## Background (verify before editing)

- **⚠ Prefix uniqueness is NOT the real invariant.** It is tempting to think "no two groups share a prefix ⇒ no two hostnames collide". **False:** `hostname_pattern` is free-form per group, so a group named `len` with pattern `{name}-serv-{index:03}` and a group named `len-serv` with the default `{name}-{index:03}` **both render `len-serv-001`**. And any `hostname_override` can collide with any generated name. The load-bearing rule is **global hostname uniqueness across every group's materialized hostnames AND every `hostname_override`**. Prefix uniqueness stays as a cheap early check with a better message — not as the guarantee.
- **Group names are immutable** (spec D2). Validation compares a proposed group against the existing set: if a group with the same `id` exists under a different `name`, that is a rename ⇒ error. Renaming is done by creating a new group and `rebind`-ing hosts (DS-REG-03).
- **`standalone` rules** (spec D3): exactly one group has `is_standalone == true`; it cannot be deleted or renamed; a `HostProfile` in it **must** carry `hostname_override`.
  **There is deliberately NO "second host in standalone" warning.** `vm-test` and `unimatrixone` are 2 of 5 machines and both legitimately live there — a warning would fire on the fleet's normal state from day one. If you add one, you are re-introducing something the design explicitly cut.
- Edge semantics (spelled out here AND in acceptance):
  - **Zero groups** → legal (fresh install). NOT an error.
  - **A group with no members** → legal.
  - **`hostname_override` in a NON-standalone group** → legal; it simply wins over the pattern (spec D2 allows an explicit override anywhere). It still participates in global uniqueness.
  - **A hostname that is not a DNS-legal label** (empty, >63 chars, leading/trailing `-`, or characters outside `[a-z0-9-]`) → error naming the offending hostname.
  - **`hostname_pattern` without `{index`** → error, because it would render the same name for every member.
- **Unknown application `kind`** never reaches this function — it fails at deserialization (`#[serde(tag = "kind")]`). Where validation *does* surface it, re-report naming the object rather than bubbling a raw serde message.

**HARD RULES (non-negotiable):**
- NO hardware actions; pure functions, no commands.
- NEVER wipe/write/deploy on 172.16.2.30 or len-serv-003. NEVER power on unimatrixone.
- No real secret in any file; `REPLACE_AT_PLACE_TIME` stays a placeholder.
- Purely additive: own `validate.rs` only. Do NOT edit `mod.rs`, `merge.rs`, or `config.rs`.
- Stay inside your worktree; never `git push`, `gh pr`, or merge — report done and stop.

- **Re-verify these anchors before editing** — line numbers drift; zero hits = STOP and report:
  ```bash
  grep -n "pub struct HostGroupProfile" crates/uaa-core/src/profile/mod.rs
  # expect: 1 hit — DS-PRF-01 merged (0 hits = wave gate not met, STOP)
  grep -n "pub struct HostProfile" crates/uaa-core/src/profile/mod.rs
  # expect: 1 hit — carries hostname_override
  grep -n "pub fn compute_join" crates/uaa-core/src/autoinstall/host_spec.rs
  # expect: 1 hit — the pure-fn + unit-test style to mirror
  ```

## Step-by-step

1. Open `crates/uaa-core/src/profile/validate.rs` (the DS-PRF-01 stub). Keep its guid; bump its version.
2. Implement, as pure functions over slices:
   ```rust
   /// Every rule. Collects ALL violations and returns them together — a weak
   /// operator fixing one error per round-trip is a bad loop.
   pub fn validate(groups: &[HostGroupProfile], profiles: &[HostProfile]) -> Result<()>;

   /// THE load-bearing rule (spec D2). Materializes every group's hostnames and
   /// every hostname_override, and rejects any duplicate — across groups, not
   /// within one.
   pub fn check_global_hostname_uniqueness(groups: &[HostGroupProfile], profiles: &[HostProfile]) -> Result<()>;

   pub fn check_prefix_uniqueness(groups: &[HostGroupProfile]) -> Result<()>;
   pub fn check_standalone_rules(groups: &[HostGroupProfile], profiles: &[HostProfile]) -> Result<()>;
   pub fn check_hostname_pattern(pattern: &str) -> Result<()>;
   pub fn is_dns_legal_label(s: &str) -> bool;
   pub fn check_no_rename(existing: &[HostGroupProfile], proposed: &HostGroupProfile) -> Result<()>;
   ```
3. Collect all violations into one error listing every one.
4. Keep purely additive — no edits outside `validate.rs`.
5. Add tests in `validate.rs`'s `mod tests`:
   - **`test_distinct_prefixes_can_still_collide`** — group `len` with pattern `{name}-serv-{index:03}` and group `len-serv` with the default both render `len-serv-001` ⇒ `Err`. **This is the whole point: prefixes differ, hostnames collide.**
   - `test_hostname_override_collides_with_generated` — an override equal to another group's generated name ⇒ `Err`.
   - `test_duplicate_prefix_rejected` — the cheap early check.
   - `test_group_rename_rejected` — same `id`, different `name` ⇒ `Err`.
   - `test_exactly_one_standalone` — two `is_standalone` groups ⇒ `Err`; zero ⇒ `Err`.
   - `test_standalone_requires_explicit_hostname` — a standalone member with `hostname_override: None` ⇒ `Err`.
   - **`test_second_standalone_host_is_legal`** — two members in `standalone`, both with overrides ⇒ `Ok`, **no error and no warning**. (`vm-test` + `unimatrixone` are the real fleet; a rule that flags this is wrong.)
   - `test_pattern_without_index_rejected` — `{name}-server` ⇒ `Err`.
   - `test_dns_illegal_hostname_rejected` — `Len_Serv!` ⇒ `Err` naming it.
   - **`test_valid_fleet_passes`** — the real fleet shape (a `len-serv` group with 3 members + a `standalone` group with `vm-test` and `unimatrixone`) ⇒ `Ok`.
   - `test_zero_groups_is_legal` — fresh install ⇒ `Ok`.
6. Bump the header on `validate.rs`; keep its guid.

**Anti-over-suppression:** this task is entirely guards, so over-blocking is the primary risk. `test_valid_fleet_passes` and `test_second_standalone_host_is_legal` are the happy-path proofs that the real fleet — three Lenovos plus two standalone machines — validates clean. Without them, an over-strict rule rejects the very configuration this system exists to deploy.

## How to test

```bash
cargo test --lib --offline
# Expected: 639+ passed, 0 failed (baseline + DS-APP-01/DS-PRF-01's tests + your 11).
cargo build --offline
# Expected: exit 0.
cargo clippy --offline -- -D warnings
# Expected: no warnings.
```

## Acceptance criteria

- [ ] `cargo test --lib --offline` exits 0 — verify: `cargo test --lib --offline 2>&1 | grep -E "^test result"`
- [ ] `cargo build --offline` exits 0 — verify: `cargo build --offline && echo BUILD_OK`
- [ ] **Global uniqueness, not just prefixes** — verify: `cargo test --lib --offline test_distinct_prefixes_can_still_collide test_hostname_override_collides_with_generated`
- [ ] Immutability enforced — verify: `cargo test --lib --offline test_group_rename_rejected`
- [ ] Standalone rules — verify: `cargo test --lib --offline test_exactly_one_standalone test_standalone_requires_explicit_hostname`
- [ ] **No standalone-count warning was added** — verify: `grep -ci "warn" crates/uaa-core/src/profile/validate.rs` returns **0**
- [ ] Anti-over-suppression: the real fleet validates clean — verify: `cargo test --lib --offline test_valid_fleet_passes test_second_standalone_host_is_legal`
- [ ] Only `validate.rs` changed — verify: `git diff --stat origin/main -- crates/uaa-core/src/profile/ | grep -c "mod.rs\|merge.rs"` returns **0**
- [ ] `cargo clippy --offline -- -D warnings` clean
- [ ] File header bumped — verify: `grep -n "last-edited: 2026-07" crates/uaa-core/src/profile/validate.rs`

## Commit message

```
feat(core): profile validation with global hostname uniqueness (DS-PRF-03)

Fills the DS-PRF-01 stub. The load-bearing rule is GLOBAL hostname
uniqueness, not prefix uniqueness: hostname_pattern is free-form, so a group
`len` with pattern {name}-serv-{index:03} and a group `len-serv` with the
default both render len-serv-001 despite having distinct prefixes. Every
materialized hostname and every hostname_override is checked across all
groups.

Also: group names are immutable (renaming orphans allocations), exactly one
standalone group exists, and standalone members require an explicit hostname.

Deliberately NO "second standalone host" warning — vm-test and unimatrixone
are 2 of 5 real machines and both belong there, so it would fire on the
fleet's normal state.

Co-Authored-By: Claude <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

**Polarity: additive** (fills an empty stub). If `grep -n "pub fn check_global_hostname_uniqueness" crates/uaa-core/src/profile/validate.rs` hits, this task is already applied — run the acceptance checks instead of re-applying. Rollback = revert the single commit, restoring the empty stub; nothing calls `validate` until DS-OPS-01, so no data or behavior is touched. DS-PRF-02 owns `merge.rs` in the same wave — disjoint file, no rebase needed between them.
