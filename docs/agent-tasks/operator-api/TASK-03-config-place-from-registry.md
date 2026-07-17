<!-- file: docs/agent-tasks/operator-api/TASK-03-config-place-from-registry.md -->
<!-- version: 1.0.0 -->
<!-- guid: 5d287c79-1094-4f05-aca0-c594d84a9427 -->
<!-- last-edited: 2026-07-16 -->

# TASK-03 — `config place --from-registry` (dry-run default, `.bak`) ⚠ review-critical (DS-OPS-03)

**Priority:** P1 · **Effort:** L · **Recommended subagent:** **Opus-class** · rust-core subagent · **Why:** the ONLY behavior-changing task in the package; it mass-overwrites every host's placed config in one loop. Never downgrade this tier, never parallelize it. · **Depends on:** DS-PRF-02 (merge), DS-REG-03 (allocation), **and DS-APP-01** (`PartialEq` for the struct-equality gate + `skip_serializing_if` for cross-version rollback safety)

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/operator-api-config-place-from-registry" -b agent/operator-api-config-place-from-registry origin/main
cd "$REPO/.worktrees/operator-api-config-place-from-registry"
git rebase origin/main
```

(Protocol also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

**Wave gate — BOTH must be merged:**
- `grep -n "pub fn merge" crates/uaa-core/src/profile/merge.rs` → 1 hit (DS-PRF-02)
- `grep -n "pub fn next_index" crates/uaa-control/src/profiles/alloc.rs` → 1 hit (DS-REG-03)

Zero hits on either = gate not met: STOP and report.

## Goal

Add `--from-registry` to `uaa config place`: resolve each host's `InstallationConfig` from the profile store (group + host + allocation) instead of reading a hand-authored `examples/configs/install/<host>.yaml`.

> ## ⚠ This is the only task that changes production behavior, and it overwrites data
>
> `place_configs` writes `/var/www/html/cloud-init/<hexmac>/uaa.yaml` with an **in-place `fs::write`** — no backup, no diff, no confirmation. Flipping `--from-registry` re-authors **every host's** placed config in one loop, with **materially different bytes**: comments stripped, serde defaults materialized.
>
> Three non-negotiable safety properties, all of which the spec requires and none of which exist today:
>
> 1. **`--from-registry` defaults OFF.** `KNOWN_HOSTS` remains the default path.
> 2. **`--dry-run` defaults ON** whenever `--from-registry` is passed. It prints a resolved-vs-committed **diff** and a placed-file **count**, and writes **nothing**. An explicit `--no-dry-run` (or `--commit`) is required to write.
> 3. **`.bak` before every overwrite.** The previous `uaa.yaml` is preserved, so rolling back M4 is an **inverse operation** rather than a re-derivation.

REUSE — do not invent parallels:

- **`place_configs` / `inject_secrets` / `inject_install_ca_cert` / `PLACEHOLDER`** in `crates/uaa-core/src/config_place.rs` — verify: `grep -n "pub const PLACEHOLDER\|fn inject_secrets\|fn inject_install_ca_cert" crates/uaa-core/src/config_place.rs`. **Do not touch the injection or the hard gate** — resolution feeds them, it does not replace them.
- **`profile::merge`** (DS-PRF-02) and **`ProfileStore`** (DS-REG-02/03) for resolution.
- **`crate::error::AutoInstallError`** for errors; no new error enum, no new dependency.

## Background (verify before editing)

- **⚠ The line-based injection matchers still have to match.** `inject_secrets` keys on the literal line shape `<key>: REPLACE_AT_PLACE_TIME`, and `inject_install_ca_cert` does an **exact** `install_ca_cert: REPLACE_AT_PLACE_TIME` string match. A serialized resolved config must still produce those exact lines or injection silently finds nothing. It degrades **fail-closed** via the `PLACEHOLDER` hard gate (a config still containing a placeholder after injection is refused, so a secretless config can never be served) — that gate is the one thing working in your favour here. **State it as a deliberate property in your report; do not rely on it by luck.**
- **⚠ The cross-version rollback trap.** `InstallationConfig` carries `deny_unknown_fields`, and the target's `uaa install` binary does **not** deploy in lockstep with control. If a placed config gains an `applications:` key and control is later rolled back, every PXE-ing machine hits a fail-closed parse on a file the rollback didn't touch. DS-APP-01 mitigated this with `skip_serializing_if = "Vec::is_empty"` — a host with no applications serializes **without** the key. **Verify that still holds** for your serialized output: `grep -n "skip_serializing_if" crates/uaa-core/src/network/ssh_installer/config.rs`. If it does not, STOP and report rather than shipping the trap.
- **The M2 gate is struct equality, not bytes.** Committed YAMLs are comment-rich (`vm-test.yaml` opens with ~29 lines of header) and omit defaulted keys; no serializer reproduces that. Compare parsed `InstallationConfig` values, which requires DS-APP-01's `PartialEq` derive — verify: `grep -n "PartialEq" crates/uaa-core/src/network/ssh_installer/config.rs`.
- Edge semantics (spelled out here AND in acceptance):
  - **`--from-registry` absent** → byte-for-byte today's behavior. `KNOWN_HOSTS` path, untouched.
  - **`--from-registry` + no `--no-dry-run`** → print the diff + count, **write nothing**, exit 0.
  - **A host in the registry but not in `KNOWN_HOSTS`** → legal; that is the point. Resolve and place it.
  - **A host in `KNOWN_HOSTS` but not the registry** → **error naming it**, never a silent skip. A skipped host means a machine PXEs with a stale config nobody noticed.
  - **Resolution fails for any host** (merge fail-closed, unreadable store) → **place nothing at all**. Resolve every host first, then write; a half-placed fleet is worse than an unplaced one.
  - **Store unreadable** → error. Never fall back to `KNOWN_HOSTS` silently — the operator asked for the registry.

**HARD RULES (non-negotiable):**
- **NEVER run this against 172.16.2.30.** `DEFAULT_DEST_BASE` is `/var/www/html/cloud-init` — the live webroot. All tests use a temp dir. If you are about to execute `uaa config place` for real, STOP.
- NO hardware actions. NEVER wipe/write/deploy on 172.16.2.30 or len-serv-003. NEVER power on unimatrixone.
- No real secret in any file; `REPLACE_AT_PLACE_TIME` stays a placeholder. Do NOT weaken the PLACEHOLDER hard gate.
- Do NOT modify `inject_secrets` or `inject_install_ca_cert`.
- Stay inside your worktree; never `git push`, `gh pr`, or merge — report done and stop.

- **Re-verify these anchors before editing** — line numbers drift; zero hits = STOP and report:
  ```bash
  grep -n "pub fn merge" crates/uaa-core/src/profile/merge.rs
  # expect: 1 hit — DS-PRF-02 merged (0 = wave gate not met, STOP)
  grep -n "pub fn next_index" crates/uaa-control/src/profiles/alloc.rs
  # expect: 1 hit — DS-REG-03 merged (0 = wave gate not met, STOP)
  grep -n "pub const PLACEHOLDER" crates/uaa-core/src/config_place.rs
  # expect: 1 hit (~line 48) — the hard gate; do not weaken
  grep -n "pub const KNOWN_HOSTS" crates/uaa-core/src/config_place.rs
  # expect: 1 hit (~line 51) — the default path, kept until the flag is flipped
  grep -n "pub const DEFAULT_DEST_BASE" crates/uaa-core/src/config_place.rs
  # expect: 1 hit (~line 59) — /var/www/html/cloud-init, the LIVE webroot
  grep -n "skip_serializing_if" crates/uaa-core/src/network/ssh_installer/config.rs
  # expect: 1 hit — DS-APP-01's cross-version rollback mitigation. 0 hits = STOP and report.
  grep -n "PartialEq" crates/uaa-core/src/network/ssh_installer/config.rs
  # expect: 1 hit — DS-APP-01's derive, needed for the struct-equality gate
  ```

## Step-by-step

1. Add `--from-registry` (default **false**) and `--no-dry-run` (default **false**, i.e. dry-run on) to the `config place` CLI args, mirroring the existing flag style — verify: `grep -n "Place {" crates/uaa/src/cli/args.rs`.
2. In `config_place.rs`, add a resolution path used **only** when `--from-registry` is set: for each host, load group+profile+allocation, `profile::merge`, serialize to YAML.
3. **Resolve every host before writing any host.** Collect failures and error once, naming each — never a partial placement.
4. Dry-run (the default): print a unified diff of resolved-vs-committed per host plus a total count; write nothing; exit 0.
5. With `--no-dry-run`: for each host, if a `uaa.yaml` exists, copy it to `uaa.yaml.bak` **before** writing; then write, then inject, then the `PLACEHOLDER` hard gate — in that order. Do not reorder around the gate.
6. Keep the non-`--from-registry` path byte-identical to today.
7. Add tests in `config_place.rs`'s test module (all against a **temp dir**, never the real webroot):
   - **`test_default_path_unchanged`** — without the flag, placement is byte-identical to today.
   - **`test_from_registry_dry_run_writes_nothing`** — flag set, no `--no-dry-run` ⇒ the temp dest is **empty** afterwards and a diff was printed.
   - **`test_from_registry_writes_bak_before_overwrite`** — an existing `uaa.yaml` ⇒ `uaa.yaml.bak` holds the **old** bytes.
   - **`test_resolution_failure_places_nothing`** — one host fails to resolve ⇒ **zero** files written, error names that host.
   - `test_known_host_missing_from_registry_errors` — named error, not a skip.
   - `test_resolved_config_still_injectable` — the serialized output still matches `inject_secrets`' line shape and the CA exact-match, and the PLACEHOLDER gate passes after injection.
   - **`test_resolved_equals_committed_by_struct_equality`** — resolved `InstallationConfig` == the parsed committed YAML for each of the four fleet hosts. **The M2 gate.** Struct equality, not bytes.
   - `test_app_free_host_omits_applications_key` — a host with no applications serializes **without** `applications:`, so a rolled-back installer still parses it.
8. Bump headers on every file you touch; keep existing guids.

**Anti-over-suppression:** dry-run-by-default is a guard that could make the command useless. `test_from_registry_writes_bak_before_overwrite` is the happy-path proof that `--no-dry-run` **does** write (and safely) — without it, an over-eager guard would silently no-op every real placement while every negative test passed.

## How to test

```bash
cargo test --lib --offline
# Expected: 634+ passed, 0 failed (baseline + upstream waves' tests + your 8).
cargo build --offline
# Expected: exit 0.
cargo clippy --offline -- -D warnings
# Expected: no warnings.
```

**Do NOT run `uaa config place` against a real destination.** Every test uses a temp dir.

## Acceptance criteria

- [ ] `cargo test --lib --offline` exits 0 — verify: `cargo test --lib --offline 2>&1 | grep -E "^test result"`
- [ ] `cargo build --offline` exits 0 — verify: `cargo build --offline && echo BUILD_OK`
- [ ] **Today's path is unchanged** — verify: `cargo test --lib --offline test_default_path_unchanged`
- [ ] **Dry-run is the default and writes nothing** — verify: `cargo test --lib --offline test_from_registry_dry_run_writes_nothing`
- [ ] **`.bak` precedes every overwrite** — verify: `cargo test --lib --offline test_from_registry_writes_bak_before_overwrite`
- [ ] **No partial placement** — verify: `cargo test --lib --offline test_resolution_failure_places_nothing`
- [ ] The M2 gate passes — verify: `cargo test --lib --offline test_resolved_equals_committed_by_struct_equality`
- [ ] Cross-version safety holds — verify: `cargo test --lib --offline test_app_free_host_omits_applications_key`
- [ ] Injection + hard gate untouched — verify: `git diff origin/main -- crates/uaa-core/src/config_place.rs | grep -c "^-.*inject_secrets\|^-.*PLACEHOLDER"` returns **0**
- [ ] Anti-over-suppression: `--no-dry-run` really writes — covered by the `.bak` test above
- [ ] `cargo clippy --offline -- -D warnings` clean
- [ ] File headers bumped — verify: `git diff origin/main --name-only | xargs -I{} grep -l "last-edited: 2026-07" {}`

## Coordinator review checklist (⚠ review-critical — line-by-line before merge)

- [ ] `--from-registry` defaults **off**; `--dry-run` defaults **on** when it is passed.
- [ ] `.bak` is written **before** the `fs::write`, not after, and not skipped when the file exists.
- [ ] Every host resolves **before** any host is written — no partial placement is possible.
- [ ] `inject_secrets` / `inject_install_ca_cert` / the `PLACEHOLDER` hard gate are unmodified, and run **after** the write in the existing order.
- [ ] A missing/unreadable store errors — it never silently falls back to `KNOWN_HOSTS`.
- [ ] No test writes outside a temp dir; nothing points at `/var/www/html/cloud-init`.

## Commit message

```
feat(place): resolve placed configs from the profile registry (DS-OPS-03)

Adds `uaa config place --from-registry`, resolving each host's
InstallationConfig from group + host profile + allocation instead of a
hand-authored per-host YAML.

This is the only behavior-changing step in the package, and it overwrites
data: place_configs does an in-place fs::write of every host's
/var/www/html/cloud-init/<hexmac>/uaa.yaml with no backup. So: --from-registry
defaults off; --dry-run defaults on when it is passed, printing a
resolved-vs-committed diff and a count while writing nothing; and the previous
uaa.yaml is copied to .bak before any overwrite, making an M4 rollback an
inverse operation rather than a re-derivation.

Resolution is all-or-nothing: every host resolves before any host is written,
because a half-placed fleet is worse than an unplaced one. A KNOWN_HOSTS host
missing from the registry is a named error, never a silent skip.

Secret injection and the REPLACE_AT_PLACE_TIME hard gate are untouched —
resolution feeds them.

Co-Authored-By: Claude <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge. This task is ⚠ review-critical: expect line-by-line review against the checklist above. **Flipping `--from-registry` in production is Bucket 3 — an operator action after a human reviews the dry-run diff. Do not flip it, and do not run this against the real webroot.**

## Idempotency / Rollback

**Polarity: additive** (a new flag; the default path is unchanged). If `grep -n "from_registry\|from-registry" crates/uaa/src/cli/args.rs` hits, this task is already applied — run the acceptance checks instead of re-applying. Rollback = revert the single commit; because `--from-registry` defaults **off**, no placed config changes unless an operator explicitly flipped it. **If it WAS flipped**, reverting the code does not rewrite the webroot: restore from the `.bak` files, or re-run `config place` without the flag — the inverse operation this task exists to provide. No sibling shares `config_place.rs`.
