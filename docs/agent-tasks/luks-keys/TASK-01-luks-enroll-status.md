<!-- file: docs/agent-tasks/luks-keys/TASK-01-luks-enroll-status.md -->
<!-- version: 1.0.0 -->
<!-- guid: ffdba0cd-c181-4639-83e0-78b83df31c84 -->
<!-- last-edited: 2026-07-10 -->

# TASK-01 — `uaa luks` enroll (systemd-cryptenroll FIDO2+PIN wrapper) + status (luksDump fido2 token parse) (ws7-luks)

**Priority:** P1 · **Effort:** M · **Recommended subagent:** Sonnet-class · rust-crypto-cli subagent · **Why:** wraps destructive-adjacent cryptenroll; executor-mocked, roles per 3-credential model. · **Depends on:** CP-01 (wave-2 gated: CP-01 merged — it creates the `crates/uaa-core/src/luks_keys.rs` stub this task fills)

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/luks-keys-luks-enroll-status" -b agent/luks-keys-luks-enroll-status origin/main
cd "$REPO/.worktrees/luks-keys-luks-enroll-status"
git rebase origin/main
```

(Protocol also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

## Goal

Fill the CP-01-created stub `crates/uaa-core/src/luks_keys.rs` with the first half of the `uaa luks` manager from spec Decision 14 and component C8 (`docs/specs/constellation-design.md`): **enroll** (wraps `systemd-cryptenroll --fido2-device=auto --fido2-with-client-pin=yes`, touch at creation unavoidable) and **status** (parses `cryptsetup luksDump` fido2 tokens). Purely additive: the stub becomes a real module; nothing else changes behavior.

**YubiKeys are for LUKS disk unlock, NOT auth.** Nothing in this module authenticates a user or a service — it manages LUKS2 keyslots only (spec Decision 14; restated because a prior design conflated the two).

The credential model is the **3-credential-per-host model** from `PLAN-zfs-luks-multikey.md` (§"YubiKey topology"): each host's LUKS header enrolls exactly three FIDO2 credentials — `primary` (the 5 Nano that stays plugged in), `backup1` (locked up), `backup2` (owner's keychain). FIDO2 non-resident credentials live in the disk's LUKS2 header, so one physical key enrolls on unlimited machines.

REUSE — do not invent parallels for any of these:

- **`CommandExecutor`** trait (`src/network/executor.rs` — verify: `grep -n "pub trait CommandExecutor" src/network/executor.rs`) — every `systemd-cryptenroll` / `cryptsetup` invocation goes through `&mut dyn CommandExecutor`. Do NOT call `std::process::Command` anywhere in this module.
- **`evaluate_fido2_keyslot`** (`src/autoinstall/verify.rs` — verify: `grep -n "pub fn evaluate_fido2_keyslot" src/autoinstall/verify.rs`) for the boolean "a systemd-fido2 token exists" check. Do NOT re-implement the presence check; your new parser ADDS structured token extraction on top of it.
- **`AutoInstallError::ConfigError` / `AutoInstallError::SystemError`** (`src/error.rs`) for all error paths. Do NOT add a new error enum.
- **Mock idiom:** mirror `MockExecutor` (`src/autoinstall/verify.rs` — verify: `grep -n "struct MockExecutor" src/autoinstall/verify.rs`; HashMap command→response) inside `#[cfg(test)]`, PLUS a `Vec<String>` of recorded commands so zero-command assertions work. Do NOT add a mocking crate.

## Background (verify before editing)

- Spec: `docs/specs/constellation-design.md` Decision 14 + §C8 (`uaa luks` ships as subcommands in the agent binary; FIDO2 ops run where the YubiKey is plugged). Plan ground truth for the credential model: `PLAN-zfs-luks-multikey.md`.
- `systemd-cryptenroll --fido2-device=auto --fido2-with-client-pin=yes <dev>` reads the EXISTING LUKS passphrase from the `$PASSWORD` env var (same convention the first-boot TPM2 unit already uses — see `src/network/ssh_installer/system_setup.rs` comment "systemd-cryptenroll reads $PASSWORD"). The passphrase therefore travels as an env-var prefix, NEVER as an argv token, and the full command string is NEVER logged.
- `cryptsetup luksDump <dev>` output contains one `systemd-fido2` token block per enrolled credential; each token block includes a `Keyslot:  <n>` line. Sample shape (from real 26.04 output; your parser must tolerate arbitrary indentation and extra fields):
  ```text
  Tokens:
    0: clevis
          Keyslot:    1
    2: systemd-fido2
          fido2-credential: 4fe0...
          Keyslot:    3
  ```
- Edge semantics (spelled out here AND in acceptance): a luksDump with ZERO `systemd-fido2` tokens is NOT an error for `status` — it returns an empty credential list (matches `evaluate_fido2_keyslot`'s fail-is-informative posture). A missing/garbled `Keyslot:` line inside a fido2 token block yields `keyslot: None`, never a parse abort. An unknown `--role` string is a hard `ConfigError` (only `primary|backup1|backup2` exist).
- LK-03 (`luks_sync.rs`, wave 5) reads the local state file this task writes — the JSON shape defined in Step 5 is a cross-task contract; do not rename its fields.

**HARD RULES (non-negotiable):**
- NO hardware actions. Validate ONLY in-repo (`cargo`) and, where a brief says so,
  the QEMU+swtpm harness (`scripts/vm-validate.sh`). Code that COULD touch hardware
  is written and unit-tested against mock executors only.
- NEVER wipe, write to, or deploy on 172.16.2.30 ("the server") or len-serv-003.
- `disk_device` is read from the live target at runtime, never guessed or hardcoded.
- ipmitool runs via `ssh 172.16.2.30`, never on macOS.
- NEVER power on unimatrixone (U1).
- No real secret in any file: `REPLACE_AT_PLACE_TIME` placeholders stay placeholders.
- Stay inside your worktree; never `git push`, `gh pr`, or merge — report done and stop.

**Path map:** after CP-01 (wave 1) merges, `src/**` lives at `crates/uaa-core/src/**` and the CLI at `crates/uaa/src/**`. The greps below cite pre-move paths (verifiable on today's main); at execution time run them at the old path, then the mapped path. Zero hits at BOTH = STOP and report.

- **Re-verify these anchors before editing** — line numbers drift; zero hits at both old and mapped path = STOP and report:
  ```bash
  grep -n "fido2-with-client-pin" PLAN-zfs-luks-multikey.md
  # expect: 1+ hits (3 today: lines 19, 34, 108 — the 3-credential model + exact cryptenroll flag)
  grep -n "pub fn evaluate_fido2_keyslot" src/autoinstall/verify.rs
  # expect: 1 hit (line 211 today; post-CP-01: crates/uaa-core/src/autoinstall/verify.rs)
  grep -n "pub trait CommandExecutor" src/network/executor.rs
  # expect: 1 hit (line 11 today; post-CP-01: crates/uaa-core/src/network/executor.rs)
  grep -n "struct MockExecutor" src/autoinstall/verify.rs
  # expect: 1 hit (line 527 today; the test-mock idiom to mirror)
  grep -n "luks_keys" crates/uaa-core/src/lib.rs
  # expect: 1 hit — CP-01 declared the stub module (this grep only works post-CP-01)
  ```

## Step-by-step

1. Run the ⛔ START HERE block, then the anchor greps above. Any zero-hit grep (at both old and mapped paths) → STOP and report.

2. **Replace the stub body of `crates/uaa-core/src/luks_keys.rs`** (keep the CP-01 file header; bump its version + `last-edited: <today>`, keep its guid) with the role model:

   ```rust
   /// Which of the 3 per-host FIDO2 credentials a keyslot belongs to
   /// (PLAN-zfs-luks-multikey.md 3-credential model). LUKS disk unlock
   /// ONLY — never used for auth.
   #[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
   #[serde(rename_all = "lowercase")]
   pub enum CredentialRole { Primary, Backup1, Backup2 }

   impl std::str::FromStr for CredentialRole { /* "primary"|"backup1"|"backup2",
       anything else => Err(AutoInstallError::ConfigError(..listing the three)) */ }
   ```

3. **Add the enroll command builder + its redacted twin** (mirror the `build_ipmi_command`/`redacted_ipmi_command` split in `src/power/mod.rs` — that redaction pattern is a locked idiom):

   ```rust
   /// Full command executed via the CommandExecutor. NEVER log this string —
   /// it embeds the existing LUKS passphrase. Log redacted_enroll_command().
   pub fn build_enroll_command(luks_dev: &str, passphrase: &str) -> crate::error::Result<String>;
   /// Passphrase-free form, safe for logs and errors.
   pub fn redacted_enroll_command(luks_dev: &str) -> String;
   ```

   - Built shape (normative): `PASSWORD='<passphrase>' systemd-cryptenroll --fido2-device=auto --fido2-with-client-pin=yes <luks_dev>`.
   - Fail-closed validation, all `ConfigError`, all BEFORE any executor call: `luks_dev` must start with `/dev/` (never guessed — the caller reads it from the live target); empty passphrase rejected; passphrase containing a single quote (`'`) REJECTED, not escaped (no shell-injection surface — same rule as the power module).
   - Redacted shape: `systemd-cryptenroll --fido2-device=auto --fido2-with-client-pin=yes <luks_dev>` — no `PASSWORD=` prefix, no passphrase characters.

4. **Add the luksDump token parser + status:**

   ```rust
   #[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
   pub struct Fido2Token { pub token_id: u32, pub keyslot: Option<u32> }

   /// Parse every `systemd-fido2` token block out of `cryptsetup luksDump` output.
   /// Zero fido2 tokens => Ok(vec![]) — NOT an error. A fido2 block with a
   /// missing/garbled `Keyslot:` line => keyslot: None — NOT a parse abort.
   pub fn parse_fido2_tokens(luksdump_output: &str) -> Vec<Fido2Token>;

   /// Run `sudo -n cryptsetup luksDump <luks_dev>` through the executor and
   /// return (CheckResult from evaluate_fido2_keyslot, parsed tokens).
   pub async fn luks_status(executor: &mut dyn crate::network::CommandExecutor,
                            luks_dev: &str)
       -> crate::error::Result<(crate::autoinstall::verify::CheckResult, Vec<Fido2Token>)>;
   ```

   `luks_status` calls the existing `evaluate_fido2_keyslot` for the presence verdict (adjust the import path to wherever it is re-exported post-CP-01) and `parse_fido2_tokens` for structure. `luks_dev` gets the same `/dev/`-prefix `ConfigError` guard, checked before any executor call.

5. **Add the enroll driver + local state record** (the LK-03 contract):

   ```rust
   #[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
   pub struct LuksCredentialRecord {
       pub yubikey_serial: String,
       pub role: CredentialRole,
       pub luks_keyslot: Option<u32>,
       pub enrolled_at: String,            // RFC3339 via chrono::Utc::now().to_rfc3339()
       pub revoked_at: Option<String>,     // set by LK-02's revoke, never here
   }

   pub async fn enroll_fido2(executor: &mut dyn crate::network::CommandExecutor,
                             luks_dev: &str, role: CredentialRole,
                             yubikey_serial: &str, passphrase: &str,
                             state_path: &std::path::Path)
       -> crate::error::Result<LuksCredentialRecord>;
   ```

   Semantics, in order (every failure fail-closed — Err BEFORE the cryptenroll call where possible; the mock must record zero commands on validation failures):
   1. Validate: `luks_dev` `/dev/`-prefix; non-empty `yubikey_serial`; passphrase via `build_enroll_command`'s guards.
   2. Snapshot BEFORE: run `sudo -n cryptsetup luksDump <luks_dev>`, `parse_fido2_tokens`, remember the token-id set.
   3. Run the built enroll command via `executor.execute_with_output(..)` — log ONLY `redacted_enroll_command(..)` (tracing macros), never the built string.
   4. Snapshot AFTER: re-run luksDump + parse. Exactly one NEW fido2 token must have appeared; zero new tokens → `SystemError` ("enrollment ran but no new systemd-fido2 token appeared"); the new token's `keyslot` becomes `luks_keyslot`.
   5. Append the `LuksCredentialRecord` to the JSON array at `state_path` (default the CLI will pass: `/var/lib/uaa/luks-credentials.json`): read-or-`vec![]`, push, then ATOMIC write — serialize to `<state_path>.tmp`, `std::fs::rename` over the target. Never truncate-then-write in place.

6. **Unit tests** — `#[cfg(test)] mod tests` at the bottom of `crates/uaa-core/src/luks_keys.rs`, MockExecutor with recorded-commands `Vec<String>`, tempdir (`std::env::temp_dir()` + unique subdir, or the `tempfile` crate ONLY if it is already in `Cargo.toml` — check first) for state files. Passphrases in tests are obviously fake (`"test-passphrase"`). Required tests:

   | Test | Asserts |
   |---|---|
   | `test_role_parse` | `"primary"/"backup1"/"backup2"` parse; `"backup3"` → Err listing the three roles |
   | `test_build_enroll_command_shape` | contains `systemd-cryptenroll --fido2-device=auto --fido2-with-client-pin=yes /dev/nvme0n1p4` and starts with `PASSWORD=` |
   | `test_build_enroll_command_rejects` | empty passphrase → Err; passphrase `"a'b"` → Err; dev `"nvme0n1p4"` (no `/dev/`) → Err |
   | `test_redacted_omits_passphrase` | for `"test-passphrase"`, redacted form contains neither `test-passphrase` nor `PASSWORD` |
   | `test_parse_fido2_tokens_multi` | fixture with clevis + 2 systemd-fido2 tokens → exactly 2 tokens with the right ids/keyslots |
   | `test_parse_fido2_tokens_empty` | fixture with no fido2 token → `vec![]` (no error) |
   | `test_parse_fido2_tokens_missing_keyslot` | fido2 block without `Keyslot:` → one token, `keyslot: None` |
   | `test_enroll_validation_no_command` | bad dev / empty serial / quote passphrase → Err AND mock recorded 0 commands |
   | `test_enroll_no_new_token_errors` | before/after luksDump identical → `SystemError`; state file NOT written |
   | `test_enroll_happy_path_appends_state` | **anti-over-suppression:** mock returns before-dump (1 fido2 token), enroll-ok, after-dump (2 fido2 tokens) → `Ok(record)` with the new keyslot; state file contains 1 record with `role:"primary"`, `revoked_at:null`; recorded enroll command equals `build_enroll_command(..)` output (guards do not block the happy path) |

7. Bump the file header (version + last-edited) on every file you touch; new files get a fresh 4-line header with a new uuid4 (`uuidgen | tr 'A-F' 'a-f'`). (Here: `crates/uaa-core/src/luks_keys.rs` keeps its CP-01 guid, version bumped.)

## How to test

```bash
cargo test --lib --offline && cargo build --offline
# Expected: all tests pass (baseline 311 + your ~10 new tests), build clean
cargo clippy --offline -- -D warnings
# Expected: no warnings
cargo test --lib --offline luks_keys
# Expected: 10 passed; 0 failed (the module's tests in isolation)
grep -rn "std::process::Command" crates/uaa-core/src/luks_keys.rs
# Expected: 0 hits (executor seam only)
grep -n "PASSWORD" crates/uaa-core/src/luks_keys.rs | grep -v "redacted\|test\|//"
# Expected: only the builder's env-prefix construction — never inside a log/println call
```

## Acceptance criteria

- [ ] Module filled: `grep -n "pub async fn enroll_fido2\|pub async fn luks_status\|pub fn parse_fido2_tokens" crates/uaa-core/src/luks_keys.rs` → 3 hits; `grep -n "todo!" crates/uaa-core/src/luks_keys.rs` → 0 hits.
- [ ] Reuse honored: `grep -n "evaluate_fido2_keyslot" crates/uaa-core/src/luks_keys.rs` → ≥1 hit (status path calls it, not a re-implementation).
- [ ] Exact cryptenroll flags: `grep -n -- "--fido2-device=auto --fido2-with-client-pin=yes" crates/uaa-core/src/luks_keys.rs` → ≥1 hit (spec C8 wording, PLAN line 19/108 semantics).
- [ ] Fail-closed proven: `test_enroll_validation_no_command` asserts the mock recorded ZERO commands on every validation failure; `test_enroll_no_new_token_errors` proves no state write on a silent enroll failure.
- [ ] Redaction proven: `grep -n "test_redacted_omits_passphrase" crates/uaa-core/src/luks_keys.rs` → 1 hit and the test passes.
- [ ] Atomic state write: `grep -n "\.tmp" crates/uaa-core/src/luks_keys.rs` → ≥1 hit and `grep -n "rename" crates/uaa-core/src/luks_keys.rs` → ≥1 hit (tmp+rename, never in-place truncate).
- [ ] Anti-over-suppression: `test_enroll_happy_path_appends_state` passes — the guard stack does not block a legitimate enroll; the recorded command equals the builder output.
- [ ] `cargo test --lib --offline && cargo build --offline` exits 0; clippy clean (`cargo clippy --offline -- -D warnings`).
- [ ] File headers bumped on every changed file (`git diff origin/main --stat` files each show a version bump; guids unchanged).

## Commit message

```
feat(luks): add uaa luks enroll + status core (FIDO2+PIN cryptenroll wrapper, luksDump parser) (ws7-luks)

Fills the CP-01 stub crates/uaa-core/src/luks_keys.rs: CredentialRole
(3-credential-per-host model per PLAN-zfs-luks-multikey.md), enroll via
systemd-cryptenroll --fido2-device=auto --fido2-with-client-pin=yes with
PASSWORD env prefix (never argv, never logged — redacted twin only),
before/after luksDump diff to capture the new keyslot, atomic tmp+rename
local state JSON for LK-03 sync, and parse_fido2_tokens on top of the
existing evaluate_fido2_keyslot. All calls via CommandExecutor mocks;
LUKS disk unlock only, never auth. ~10 unit tests.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

Additive (stub → fill): if `grep -n "pub async fn enroll_fido2" crates/uaa-core/src/luks_keys.rs` hits, the task is already applied — run the Acceptance checks instead of re-applying. Rollback = revert the single commit; the CP-01 stub file returns (module declaration in `lib.rs` untouched either way), no other file, no state, and no host is affected — the feature is dormant unless invoked.
