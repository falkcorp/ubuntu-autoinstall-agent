<!-- file: docs/agent-tasks/profiles/TASK-02-merge-provenance.md -->
<!-- version: 1.0.0 -->
<!-- guid: 49c509d4-909e-4683-bb4e-6526b872b0a6 -->
<!-- last-edited: 2026-07-16 -->

# TASK-02 — `merge()` + provenance + 10-required-field fail-closed (DS-PRF-02)

**Priority:** P1 · **Effort:** L · **Recommended subagent:** Sonnet-class · rust-core subagent · **Why:** the fail-closed scope is a correctness trap — "error on any unset field" rejects configs that parse fine today. · **Depends on:** DS-PRF-01 (fills its `merge.rs` stub)

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/profiles-merge-provenance" -b agent/profiles-merge-provenance origin/main
cd "$REPO/.worktrees/profiles-merge-provenance"
git rebase origin/main
```

(Protocol also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

**Wave gate:** DS-PRF-01 must be merged. If `grep -n "pub struct InstallationConfigPartial" crates/uaa-core/src/profile/mod.rs` returns 0 hits, the gate is not met: STOP and report.

## Goal

Fill the DS-PRF-01 stub `crates/uaa-core/src/profile/merge.rs`:

```rust
pub fn merge(group: &HostGroupProfile, host: &HostProfile) -> Result<(InstallationConfig, Provenance)>;
```

Override-wins for scalars; **union** for applications; per-field provenance; fail-closed on the **10 required fields only**.

**Own only `merge.rs`.** Do not edit `mod.rs` (DS-PRF-01 owns it) or `validate.rs` (DS-PRF-03 owns it) — they are sibling tasks in the same wave.

REUSE — do not invent parallels:

- **`InstallationConfigPartial`, `HostGroupProfile`, `HostProfile`, `Provenance`, `Source`** from `profile/mod.rs` (DS-PRF-01) — import, never redefine.
- **`ApplicationSpec` / `CockroachSpec` / `CockroachSpecPartial`** — from `config.rs` and `profile/mod.rs` respectively. Never redefine.
- **`crate::error::AutoInstallError::ConfigError`** for the fail-closed error. Do NOT add a new error type.
- The serde default fns already in `config.rs` (`default_tang_threshold`, `default_true`, `default_tpm2_pcr_ids`, `default_install_ca_cert`, `default_network_renderer`) — call them for defaulted fields. Do NOT duplicate their literal values.

## Background (verify before editing)

- **⚠ Fail-closed applies to exactly 10 fields — not "any unset field".** `InstallationConfig` has nine `#[serde(default)]` fields (`network_renderer`, `tang_threshold`, `enroll_tpm2`, `tpm2_pcr_ids`, `expect_fido2`, `install_ca_cert`, `initramfs_type`, `tang_servers`, `ssh_authorized_keys`) and three implicitly-optional `Option`s (`debootstrap_release`, `debootstrap_mirror`, `tpm2_pin`). **`len-serv-001.yaml` omits `network_renderer` entirely** and relies on the default — so a merge that errors on *any* unset field rejects a config that parses fine today, and the M2 struct-equality gate fails on field one.
  The 10 that DO fail closed: `hostname`, `disk_device`, `timezone`, `luks_key`, `root_password`, `network_interface`, `network_address`, `network_gateway`, `network_search`, `network_nameservers`. Everything else falls back to its serde default with `Source::Default`.
- **Precedence per field:** `host.overrides.<f>` if `Some` → `Source::Host`; else `group.defaults.<f>` if `Some` → `Source::Group`; else the serde default → `Source::Default`; else (one of the 10) → error naming the field.
- **⚠ The double-Option fields** (`tpm2_pin`, `debootstrap_release`, `debootstrap_mirror`) are `Option<Option<T>>`: `None` = inherit (fall through to group, then default), `Some(None)` = **explicitly none — stop here, do NOT fall through**, `Some(Some(v))` = this value. Collapsing this to a plain `Option` makes a host meant to have **no** TPM PIN silently inherit the group's.
- **Applications union** (spelled out here AND in acceptance): the effective list is group ∪ host **keyed by variant kind**. A kind present in both merges **field-by-field** — the host's `CockroachSpecPartial` over the group's `CockroachSpec` — never whole-replace. A kind only in one tier is taken as-is. Order the result deterministically (by kind) so the resolved config is reproducible.
- Edge semantics:
  - **Empty group defaults + full host overrides** → legal; every field is `Source::Host`.
  - **A host overriding only `locality`** → the other `CockroachSpec` fields come from the group. This is why `CockroachSpecPartial` exists.
  - **`hostname`** comes from the resolved allocation (caller-supplied via `host.hostname_override` or the group's pattern) — merge does not invent it; if neither tier supplies it, that is one of the 10 fail-closed errors.

**HARD RULES (non-negotiable):**
- NO hardware actions; this is a pure function and runs no command.
- NEVER wipe/write/deploy on 172.16.2.30 or len-serv-003. NEVER power on unimatrixone.
- No real secret in any file; `REPLACE_AT_PLACE_TIME` stays a placeholder (merge must pass placeholders through untouched — injection happens later at place time).
- Purely additive: own `merge.rs` only. Do NOT edit `mod.rs`, `validate.rs`, or `config.rs`.
- Stay inside your worktree; never `git push`, `gh pr`, or merge — report done and stop.

- **Re-verify these anchors before editing** — line numbers drift; zero hits = STOP and report:
  ```bash
  grep -n "pub struct InstallationConfigPartial" crates/uaa-core/src/profile/mod.rs
  # expect: 1 hit — DS-PRF-01 merged (0 hits = wave gate not met, STOP)
  grep -n "Option<Option<String>>" crates/uaa-core/src/profile/mod.rs
  # expect: 3 hits — the inherit / explicitly-none / value fields
  grep -n "pub struct InstallationConfig" crates/uaa-core/src/network/ssh_installer/config.rs
  # expect: 1 hit (~line 43) — the merge target's field list
  grep -n "fn default_tang_threshold\|fn default_true\|fn default_tpm2_pcr_ids\|pub fn default_install_ca_cert\|pub fn default_network_renderer" crates/uaa-core/src/network/ssh_installer/config.rs
  # expect: 5 hits — call these; do not duplicate their literals
  grep -n "network_renderer" examples/configs/install/len-serv-001.yaml
  # expect: 0 hits — PROOF that a real committed config omits a defaulted field
  ```

## Step-by-step

1. Open `crates/uaa-core/src/profile/merge.rs` (the DS-PRF-01 stub). Keep its guid; bump its version.
2. Implement `merge(group, host) -> Result<(InstallationConfig, Provenance)>` per the precedence rules above. Record a `Source` per field name into `Provenance`.
3. Implement the application union keyed by kind, with per-field `CockroachSpecPartial`-over-`CockroachSpec` merging, deterministically ordered.
4. Collect **all** missing required fields and error once, naming every one — not just the first. A weak executor fixing one field at a time is a bad loop.
5. Keep purely additive — no edits outside `merge.rs`.
6. Add tests in `merge.rs`'s `mod tests`:
   - `test_merge_host_overrides_group` — host field wins, `Source::Host`.
   - `test_merge_unset_inherits_group` — `Source::Group`.
   - `test_serde_defaulted_field_is_not_unset` — neither tier sets `network_renderer` ⇒ `"networkd"`, `Source::Default`, **no error**. (This is the trap.)
   - `test_merge_fails_closed_on_defaultless_unset_field` — omit `luks_key` ⇒ `Err` naming `luks_key`.
   - `test_merge_error_names_all_missing_fields` — omit three ⇒ one error naming all three.
   - `test_tpm2_pin_explicit_none_does_not_inherit` — group has a PIN, host sets `Some(None)` ⇒ resolved `tpm2_pin` is `None`. **The trap's guard.**
   - `test_tpm2_pin_unset_inherits_group` — host `None` ⇒ group's PIN.
   - `test_merge_application_lists_union` — group `[cockroach]`, host `[]` ⇒ `[cockroach]`; group `[]`, host `[cockroach]` ⇒ `[cockroach]`.
   - `test_merge_application_partial_overrides_field` — group cockroach with `locality=A`, host partial `locality=B` ⇒ resolved `locality=B` **and `seed_ip` still the group's**. (Proves per-field, not whole-replace.)
   - `test_merge_passes_placeholders_through` — `luks_key: "REPLACE_AT_PLACE_TIME"` survives merge untouched.
7. Bump the header on `merge.rs`; keep its guid.

**Anti-over-suppression:** the 10-field fail-closed check is a guard that can over-block. `test_serde_defaulted_field_is_not_unset` is the happy-path proof that a legitimately-defaulted field still resolves — without it, an over-broad guard rejects `len-serv-001.yaml`, a config that works in production today.

## How to test

```bash
cargo test --lib --offline
# Expected: 639+ passed, 0 failed (baseline + DS-APP-01/DS-PRF-01's tests + your 10).
cargo build --offline
# Expected: exit 0.
cargo clippy --offline -- -D warnings
# Expected: no warnings.
```

## Acceptance criteria

- [ ] `cargo test --lib --offline` exits 0 — verify: `cargo test --lib --offline 2>&1 | grep -E "^test result"`
- [ ] `cargo build --offline` exits 0 — verify: `cargo build --offline && echo BUILD_OK`
- [ ] Anti-over-suppression: a defaulted field resolves, not errors — verify: `cargo test --lib --offline test_serde_defaulted_field_is_not_unset`
- [ ] Fail-closed covers exactly the 10 — verify: `cargo test --lib --offline test_merge_fails_closed_on_defaultless_unset_field test_merge_error_names_all_missing_fields`
- [ ] The double-Option trap is handled — verify: `cargo test --lib --offline test_tpm2_pin_explicit_none_does_not_inherit test_tpm2_pin_unset_inherits_group`
- [ ] Per-field application override, not whole-replace — verify: `cargo test --lib --offline test_merge_application_partial_overrides_field`
- [ ] Only `merge.rs` changed — verify: `git diff --stat origin/main -- crates/uaa-core/src/profile/ | grep -c "mod.rs\|validate.rs"` returns **0**
- [ ] `cargo clippy --offline -- -D warnings` clean
- [ ] File header bumped — verify: `grep -n "last-edited: 2026-07" crates/uaa-core/src/profile/merge.rs`

## Commit message

```
feat(core): implement profile merge with provenance (DS-PRF-02)

Fills the DS-PRF-01 stub: override-wins for scalars, union for applications
(per-field CockroachSpecPartial over the group's spec, not whole-replace), and
a Source per field so "why does this host run Cockroach?" is answerable
without recomputing two blobs by hand.

Fail-closed is scoped to the 10 genuinely required fields. Nine
InstallationConfig fields carry #[serde(default)] and three are Option —
len-serv-001.yaml omits network_renderer entirely, so an "error on any unset
field" merge would reject a config that works in production today.

tpm2_pin/debootstrap_* are Option<Option<T>>: Some(None) means explicitly
none and must NOT fall through to the group, or a host meant to have no TPM
PIN silently inherits one.

Co-Authored-By: Claude <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

**Polarity: additive** (fills an empty stub). If `grep -n "pub fn merge" crates/uaa-core/src/profile/merge.rs` hits, this task is already applied — run the acceptance checks instead of re-applying. Rollback = revert the single commit, restoring the empty stub; nothing calls `merge` until DS-OPS-03, so no data or behavior is touched. DS-PRF-03 owns `validate.rs` in the same wave — disjoint file, no rebase needed between them.
