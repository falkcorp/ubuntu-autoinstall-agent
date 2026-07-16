<!-- file: docs/agent-tasks/profiles/TASK-01-profile-scaffold-types.md -->
<!-- version: 1.0.0 -->
<!-- guid: a6354b1f-19a6-4011-abcb-cf049c2d2bf6 -->
<!-- last-edited: 2026-07-16 -->

# TASK-01 — `profile/` module scaffold: partial types + `merge.rs`/`validate.rs` stubs (DS-PRF-01)

**Priority:** P1 · **Effort:** M · **Recommended subagent:** Sonnet-class · rust-core subagent · **Why:** defines the partial types (including the `Option<Option<String>>` trap) that both sibling tasks fill against; a wrong shape here silently breaks per-host overrides. · **Depends on:** DS-APP-01 (needs `ApplicationSpec`)

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/profiles-profile-scaffold-types" -b agent/profiles-profile-scaffold-types origin/main
cd "$REPO/.worktrees/profiles-profile-scaffold-types"
git rebase origin/main
```

(Protocol also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

**Wave gate:** DS-APP-01 must be merged. If `grep -n "pub enum ApplicationSpec" crates/uaa-core/src/network/ssh_installer/config.rs` returns 0 hits, the gate is not met: STOP and report.

## Goal

Create `crates/uaa-core/src/profile/mod.rs` holding the `HostGroupProfile` / `HostProfile` partial types, plus **empty stubs** `merge.rs` and `validate.rs` that DS-PRF-02 and DS-PRF-03 fill. `mod.rs` declares both submodules so the siblings never re-edit it — they each own one disjoint file.

**This task ships types and empty stubs only.** No merge logic (DS-PRF-02), no validation logic (DS-PRF-03).

REUSE — do not invent parallels:

- **`ApplicationSpec` / `CockroachSpec` come from `config.rs`** (DS-APP-01) — import them; do NOT redefine. Verify: `grep -n "pub enum ApplicationSpec" crates/uaa-core/src/network/ssh_installer/config.rs`. `ApplicationSpec` lives in `config.rs` because it is part of `InstallationConfig`'s contract; `profile/` depends on it, never the reverse.
- **`InstallationConfig`** is the merge target — verify: `grep -n "pub struct InstallationConfig" crates/uaa-core/src/network/ssh_installer/config.rs`. Every partial field mirrors one of its fields.
- **`crate::error::AutoInstallError`** for error paths. Do NOT define a new error enum.
- Declare `pub mod profile;` in `crates/uaa-core/src/lib.rs`, mirroring the existing module lines.

## Background (verify before editing)

- A **partial** is "the same fields as `InstallationConfig`, all optional". Both tiers use the same partial type: `HostGroupProfile.defaults` and `HostProfile.overrides` are both `InstallationConfigPartial`.
- **⚠ The `Option<Option<String>>` trap — get this wrong and a host silently inherits a PIN it must not have.** `InstallationConfig.tpm2_pin` is already `Option<String>` (`None` = no PIN). In a *partial*, a plain `Option<String>` cannot distinguish **"this host doesn't override the PIN"** from **"this host explicitly has NO PIN"** — so a host meant to have no PIN would inherit the group's. It must be `Option<Option<String>>`:
  - `None` ⇒ inherit from the group
  - `Some(None)` ⇒ explicitly no PIN
  - `Some(Some(p))` ⇒ this PIN
  The same rule applies to **every** field that is `Option<T>` in `InstallationConfig`: `debootstrap_release`, `debootstrap_mirror`, `tpm2_pin`. Non-`Option` fields (e.g. `hostname: String`) are plain `Option<String>` in the partial.
- **`CockroachSpecPartial`** — every field `Option<T>` — is required so a host can override *only* `locality` without restating `seed_ip` (which has no default). Without it, per-application override degrades to whole-application replace, contradicting the locked model (spec D1).
- Edge semantics (spelled out here AND in acceptance):
  - **An all-`None` partial** ⇒ legal; means "inherit everything". NOT an error.
  - **`hostname_override: None` in the `standalone` group** ⇒ legal at *this* task's scope (DS-PRF-03 validates it). Do NOT add validation here.
  - **Unknown YAML key in a partial** ⇒ hard error. Carry `#[serde(deny_unknown_fields)]` on every partial type, mirroring `InstallationConfig`.

**HARD RULES (non-negotiable):**
- NO hardware actions; this task is pure types and runs no command.
- NEVER wipe/write/deploy on 172.16.2.30 or len-serv-003. NEVER power on unimatrixone.
- No real secret in any file; `REPLACE_AT_PLACE_TIME` stays a placeholder.
- Purely additive: do NOT modify `config.rs`, `InstallationConfig`, or any existing module.
- Stay inside your worktree; never `git push`, `gh pr`, or merge — report done and stop.

- **Re-verify these anchors before editing** — line numbers drift; zero hits = STOP and report:
  ```bash
  grep -n "pub enum ApplicationSpec" crates/uaa-core/src/network/ssh_installer/config.rs
  # expect: 1 hit — DS-APP-01 merged (0 hits = wave gate not met, STOP)
  grep -n "pub struct InstallationConfig" crates/uaa-core/src/network/ssh_installer/config.rs
  # expect: 1 hit (~line 43) — the field list every partial mirrors
  grep -n "pub tpm2_pin" crates/uaa-core/src/network/ssh_installer/config.rs
  # expect: 1 hit (~line 84) — already Option<String>; hence Option<Option<String>> in a partial
  grep -n "pub debootstrap_release\|pub debootstrap_mirror" crates/uaa-core/src/network/ssh_installer/config.rs
  # expect: 2 hits — the other two Option fields needing the double-Option treatment
  grep -n "^pub mod \|^mod " crates/uaa-core/src/lib.rs
  # expect: several hits — add `pub mod profile;` alongside, mirroring the style
  ```

## Step-by-step

1. Create `crates/uaa-core/src/profile/mod.rs` with a fresh 4-line header (new uuid4 via `uuidgen | tr '[:upper:]' '[:lower:]'`), declaring `pub mod merge; pub mod validate;`.
2. Define in `mod.rs`:
   ```rust
   /// Every InstallationConfig field, all optional. Used for BOTH a group's
   /// defaults and a host's overrides.
   #[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
   #[serde(deny_unknown_fields, default)]
   pub struct InstallationConfigPartial {
       pub hostname: Option<String>,
       pub disk_device: Option<String>,
       // ... one per InstallationConfig field ...
       /// Double Option: None = inherit, Some(None) = explicitly no PIN,
       /// Some(Some(p)) = this PIN. A plain Option here would make a host
       /// meant to have NO pin silently inherit the group's.
       pub tpm2_pin: Option<Option<String>>,
       pub debootstrap_release: Option<Option<String>>,
       pub debootstrap_mirror: Option<Option<String>>,
       pub applications: Option<Vec<ApplicationSpec>>,
   }

   pub struct HostGroupProfile {
       pub name: String,               // the hostname prefix; immutable
       pub hostname_pattern: String,   // default "{name}-{index:03}"
       pub is_standalone: bool,
       pub defaults: InstallationConfigPartial,
       pub applications: Vec<ApplicationSpec>,
   }

   pub struct HostProfile {
       pub group_name: String,
       pub identity: String,                  // the MAC
       pub hostname_override: Option<String>,
       pub overrides: InstallationConfigPartial,
       pub applications: Vec<ApplicationSpec>,
   }

   /// Every field of CockroachSpec, all optional — so a host can override
   /// only `locality` without restating `seed_ip` (which has no default).
   #[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
   #[serde(deny_unknown_fields, default)]
   pub struct CockroachSpecPartial { /* one Option<T> per CockroachSpec field */ }

   /// Where a resolved field's value came from. Filled by DS-PRF-02.
   #[derive(Debug, Clone, Copy, PartialEq, Eq)]
   pub enum Source { Group, Host, Default }

   pub struct Provenance(pub std::collections::BTreeMap<String, Source>);
   ```
3. Create `merge.rs` and `validate.rs` as stubs — each with its own fresh header and a `// Filled by DS-PRF-0N.` comment. **Empty, no logic.** They must compile.
4. Declare `pub mod profile;` in `crates/uaa-core/src/lib.rs`.
5. Keep purely additive — do not touch `config.rs`.
6. Add tests in `mod.rs`'s `mod tests`:
   - `test_partial_all_none_is_legal` — `InstallationConfigPartial::default()` constructs and every field is `None`.
   - `test_tpm2_pin_distinguishes_inherit_from_explicit_none` — deserializing `{}` gives `tpm2_pin: None` (inherit); deserializing `{"tpm2_pin": null}` gives `Some(None)` (explicitly no PIN). **These must not be equal.** This is the trap; the test is the guard.
   - `test_partial_rejects_unknown_field` — a typo'd key is a parse error naming the key.
   - `test_partial_roundtrips_applications` — a partial carrying a `Cockroach` application round-trips.
7. Bump headers on every file you touch; keep existing guids.

**Anti-over-suppression: N/A** — this task adds no filter, guard, veto, skip, or dedupe path; it defines data types and empty stubs.

## How to test

```bash
cargo test --lib --offline
# Expected: 639+ passed, 0 failed (634 baseline + DS-APP-01's 5 + your 4).
cargo build --offline
# Expected: exit 0.
cargo clippy --offline -- -D warnings
# Expected: no warnings.
```

## Acceptance criteria

- [ ] `cargo test --lib --offline` exits 0 — verify: `cargo test --lib --offline 2>&1 | grep -E "^test result"`
- [ ] `cargo build --offline` exits 0 — verify: `cargo build --offline && echo BUILD_OK`
- [ ] The double-Option trap is handled — verify: `grep -c "Option<Option<String>>" crates/uaa-core/src/profile/mod.rs` returns **3** (`tpm2_pin`, `debootstrap_release`, `debootstrap_mirror`)
- [ ] Inherit ≠ explicitly-none — verify: `cargo test --lib --offline test_tpm2_pin_distinguishes_inherit_from_explicit_none`
- [ ] `ApplicationSpec` is imported, not redefined — verify: `grep -c "pub enum ApplicationSpec" crates/uaa-core/src/profile/mod.rs` returns **0**
- [ ] `config.rs` untouched — verify: `git diff origin/main --name-only | grep -c "ssh_installer/config.rs"` returns **0**
- [ ] Stubs compile and are empty — verify: `wc -l crates/uaa-core/src/profile/merge.rs` returns < 15
- [ ] Module declared — verify: `grep -c "pub mod profile" crates/uaa-core/src/lib.rs` returns 1
- [ ] Anti-over-suppression: N/A (stated above)
- [ ] `cargo clippy --offline -- -D warnings` clean
- [ ] File headers bumped — verify: `git diff origin/main --name-only | xargs -I{} grep -l "last-edited: 2026-07" {}`

## Commit message

```
feat(core): scaffold profile module with partial types (DS-PRF-01)

Adds crates/uaa-core/src/profile/ — InstallationConfigPartial (every
InstallationConfig field, all optional), HostGroupProfile, HostProfile,
CockroachSpecPartial, and the Provenance types — plus empty merge.rs and
validate.rs stubs so DS-PRF-02/03 each fill one disjoint file and never
collide on mod.rs.

tpm2_pin, debootstrap_release and debootstrap_mirror are Option<Option<T>>:
a plain Option cannot distinguish "not overridden" from "explicitly none", so
a host meant to have NO tpm2 PIN would silently inherit the group's.

CockroachSpecPartial exists so a host can override only `locality` without
restating `seed_ip`, which has no default — without it, per-application
override degrades to whole-application replace (spec D1).

Co-Authored-By: Claude <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

**Polarity: additive.** If `grep -n "pub struct InstallationConfigPartial" crates/uaa-core/src/profile/mod.rs` hits, this task is already applied — run the acceptance checks instead of re-applying. Rollback = revert the single commit; nothing reads the types yet and no data, schema, or existing module is touched. DS-PRF-02/03 fill stubs created here and must be dispatched only after this merges — see the collision table in `../BREAKDOWN-2026-07-16.md`.
