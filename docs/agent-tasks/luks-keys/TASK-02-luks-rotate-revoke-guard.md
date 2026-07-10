<!-- file: docs/agent-tasks/luks-keys/TASK-02-luks-rotate-revoke-guard.md -->
<!-- version: 1.0.0 -->
<!-- guid: aaadf216-4f9e-4494-8ba2-10a666162cef -->
<!-- last-edited: 2026-07-10 -->

# TASK-02 — rotate/revoke/rotate-tang with fleet-aware Tang t=2-of-3 cold-start guard (fail-closed, typed-hostname override) (ws7-luks)

**Priority:** P1 · **Effort:** M · **Recommended subagent:** Opus-class · rust-crypto-safety subagent · **Why:** ⚠ keyslot destruction + fleet lockout risk — enroll-new-then-revoke-old ordering is the safety property. · **Depends on:** TASK-01 (wave-3 gated: LK-01 MERGED first — both tasks edit `crates/uaa-core/src/luks_keys.rs`, collision-row serialized; do not start until LK-01 is on `origin/main` and this worktree is rebased)

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/luks-keys-luks-rotate-revoke-guard" -b agent/luks-keys-luks-rotate-revoke-guard origin/main
cd "$REPO/.worktrees/luks-keys-luks-rotate-revoke-guard"
git rebase origin/main
```

(Protocol also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

## Goal

Extend `crates/uaa-core/src/luks_keys.rs` (LK-01 filled enroll/status; this task ADDS, never rewrites, on top of it) with the destructive half of spec Decision 14 / component C8 (`docs/specs/constellation-design.md`): **`rotate`** (enroll-new-then-revoke-old, NEVER reverse), **`revoke --serial`** (keyslot wipe with a last-method guard), and **`rotate-tang`** (fleet-aware re-bind sweep BEFORE old-Tang-key retirement), all behind the **cold-start guard**: any rotation verifies ≥2 of 3 Tang servers remain valid for every affected binding BEFORE removing anything — fail-closed; override requires TYPING THE HOSTNAME.

**YubiKeys are for LUKS disk unlock, NOT auth.** This module manages LUKS2 keyslots and clevis Tang SSS bindings only (spec Decision 14).

Why the ordering is the safety property (spec C8 + Decision 14 scope note, restated so no "simplification" reverses it): if the old credential is revoked before the new one is proven present in the LUKS header, a failure in between leaves the host with FEWER unlock methods than it started with — up to a permanent lockout of an encrypted disk. Similarly, retiring an old Tang server key before every fleet host has re-bound its SSS pin means any host that reboots in that window and can only reach the retired-key Tang fails its t=2-of-3 quorum cold. Both orderings are therefore encoded in code (state machine + guards), not just documented.

REUSE — do not invent parallels:

- **Everything LK-01 built** in this same file: `CredentialRole`, `Fido2Token`, `parse_fido2_tokens`, `build_enroll_command`/`redacted_enroll_command`, `enroll_fido2`, `LuksCredentialRecord`, the atomic tmp+rename state-file write. Rotate CALLS `enroll_fido2` for its enroll leg.
- **`CommandExecutor`** (`src/network/executor.rs` — verify: `grep -n "pub trait CommandExecutor" src/network/executor.rs`) for every `systemd-cryptenroll`/`cryptsetup`/`clevis`/`curl` invocation. No `std::process::Command`.
- **`evaluate_fido2_keyslot`** (`src/autoinstall/verify.rs`) — luksDump parsing stays on the LK-01 parser built over it.
- **`AutoInstallError::ConfigError` / `SystemError`** (`src/error.rs`); the LK-01 `MockExecutor` test idiom with recorded commands.

## Background (verify before editing)

- Ground truth for the unlock topology: `PLAN-zfs-luks-multikey.md` — **clevis Tang SSS t=2 of 3 (172.16.2.45/46/47)**, clevis TPM2+PIN, and 3 FIDO2 credentials per host. The Tang triplet is what the quorum guard probes.
- A Tang server's liveness/validity is checked by fetching its advertisement: `curl -sf --max-time 5 <tang_url>/adv` returns a JWS advertisement JSON on a healthy Tang; non-zero exit / empty body = invalid. The probe runs through the executor (mockable), NEVER via a direct HTTP client in this module.
- Keyslot wipe command: `systemd-cryptenroll --wipe-slot=<n> <dev>` (preferred over `cryptsetup luksKillSlot` — no passphrase prompt when run with sufficient privilege via `sudo -n`).
- Edge semantics (spelled out here AND in acceptance):
  - **Quorum guard:** fewer than 2 of the 3 Tang URLs answering with a valid adv → every rotate/revoke/rotate-tang entry point returns `Err(SystemError)` BEFORE any destructive command; the mock must record zero wipe/unbind commands on that path.
  - **Typed-hostname override:** the override is a function argument `override_confirmation: Option<&str>`; it bypasses the quorum guard ONLY when it is `Some(s)` and `s` equals the target hostname EXACTLY (case-sensitive, no trim beyond trailing newline). `Some("yes")`, `Some("")`, or a wrong hostname do NOT bypass — same `Err` as no override. The CLI layer is what prompts the human to type; this library only compares.
  - **Revoke last-method guard:** if the credential being revoked is the LAST systemd-fido2 token in the header, revoke refuses (`ConfigError` naming the rule) unless the same typed-hostname override is supplied. Revoking one of 3 while 2 remain needs no override (quorum guard still applies).
  - **Rotate is atomic-ordered:** enroll leg first; only after the post-enroll luksDump proves the NEW token present does the revoke leg run. Enroll-leg failure → return its Err, old credential UNTOUCHED, state file unchanged for the old record.
  - **rotate-tang sweep-before-retire:** encoded as a state machine (`TangRotation`, Step 4) whose `retire_old_key()` returns `Err` while ANY fleet host is un-rebound. There is no code path that emits a retire command early.
- This task runs in wave 3, AFTER LK-01 (wave 2) edited this file — line numbers from LK-01's shape WILL exist; the greps are authoritative.

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
  grep -n "t=2\|sss" PLAN-zfs-luks-multikey.md | head -5
  # expect: hits (line 17 today: "clevis Tang SSS (t=2 of 3: 172.16.2.45/46/47)")
  grep -n "pub async fn enroll_fido2" crates/uaa-core/src/luks_keys.rs
  # expect: 1 hit — LK-01 merged (this grep only works post-LK-01; 0 hits = LK-01 not merged, STOP)
  grep -n "pub fn parse_fido2_tokens" crates/uaa-core/src/luks_keys.rs
  # expect: 1 hit (LK-01's parser you build on)
  grep -n "pub trait CommandExecutor" src/network/executor.rs
  # expect: 1 hit (line 11 today; post-CP-01: crates/uaa-core/src/network/executor.rs)
  ```

## Step-by-step

1. Run the ⛔ START HERE block, then the anchor greps. `enroll_fido2` absent from `crates/uaa-core/src/luks_keys.rs` → LK-01 is not merged → STOP and report (wave gate).

2. **Add the Tang quorum guard** to `crates/uaa-core/src/luks_keys.rs`:

   ```rust
   /// The 3 Tang servers backing every SSS t=2-of-3 binding
   /// (PLAN-zfs-luks-multikey.md). Overridable for tests.
   pub const DEFAULT_TANG_URLS: [&str; 3] = [
       "http://172.16.2.45", "http://172.16.2.46", "http://172.16.2.47",
   ];

   /// Probe each Tang adv via `curl -sf --max-time 5 <url>/adv` through the
   /// executor. Ok(valid_count). A probe failure counts as invalid, never Err.
   pub async fn check_tang_quorum(executor: &mut dyn crate::network::CommandExecutor,
                                  tang_urls: &[&str]) -> crate::error::Result<usize>;

   /// Fail-closed gate for every destructive op. Passes when valid >= 2, OR when
   /// override_confirmation == Some(exact target hostname). Wrong/empty/None
   /// override with valid < 2 => Err(SystemError naming the counts and the
   /// typed-hostname rule). NEVER logs the override value.
   pub async fn require_tang_quorum(executor: &mut dyn crate::network::CommandExecutor,
                                    tang_urls: &[&str], target_hostname: &str,
                                    override_confirmation: Option<&str>)
       -> crate::error::Result<()>;
   ```

   Override comparison: strip ONE trailing `\n`/`\r\n` from the supplied string, then byte-equality with `target_hostname`. No lowercase, no substring, no "y/yes".

3. **Add revoke + rotate:**

   ```rust
   /// Wipe the keyslot bound to `yubikey_serial` (looked up in the state file).
   /// Guards, in order, each BEFORE any wipe command:
   ///   1. require_tang_quorum (fail-closed)
   ///   2. last-method guard: if the header would be left with ZERO systemd-fido2
   ///      tokens, refuse (ConfigError) unless override_confirmation matches.
   /// Then: `sudo -n systemd-cryptenroll --wipe-slot=<n> <dev>`, verify via
   /// luksDump the token is GONE, and set revoked_at (RFC3339) on the state
   /// record via the LK-01 atomic tmp+rename write.
   pub async fn revoke_fido2(executor: &mut dyn crate::network::CommandExecutor,
                             luks_dev: &str, yubikey_serial: &str,
                             target_hostname: &str, tang_urls: &[&str],
                             override_confirmation: Option<&str>,
                             state_path: &std::path::Path)
       -> crate::error::Result<()>;

   /// ENROLL-NEW-THEN-REVOKE-OLD — never reverse (spec C8). Calls LK-01's
   /// enroll_fido2 for the new credential; ONLY on its Ok (new token proven in
   /// the header) does the revoke leg run. Enroll failure => old key untouched.
   pub async fn rotate_fido2(executor: &mut dyn crate::network::CommandExecutor,
                             luks_dev: &str, old_serial: &str, new_serial: &str,
                             role: CredentialRole, passphrase: &str,
                             target_hostname: &str, tang_urls: &[&str],
                             override_confirmation: Option<&str>,
                             state_path: &std::path::Path)
       -> crate::error::Result<LuksCredentialRecord>;
   ```

   `rotate_fido2` runs `require_tang_quorum` ONCE at entry (before the enroll leg too — a rotation should not start against a degraded fleet), then enroll, then the revoke leg (which re-checks nothing destructive-relevant beyond the last-method guard — quorum was already proven this call).

4. **Add the rotate-tang sweep state machine** (Decision 14 scope note: Tang SERVER key rotation requires a fleet re-bind sweep BEFORE the old key retires):

   ```rust
   /// Enforces sweep-before-retire in state, not prose. Constructed with the
   /// full fleet host list; retire_old_key() is unreachable until every host
   /// is marked rebound.
   pub struct TangRotation { /* hosts: BTreeMap<String, bool /*rebound*/> */ }
   impl TangRotation {
       pub fn new(fleet_hosts: &[String]) -> crate::error::Result<Self>;  // empty list => ConfigError (a sweep over nobody is a config bug, not a free pass)
       /// Re-bind ONE host's SSS pin: `sudo -n clevis luks regen -d <dev> -s <slot> -q`
       /// via an executor already connected to that host; on Ok marks it rebound.
       pub async fn rebind_host(&mut self, executor: &mut dyn crate::network::CommandExecutor,
                                host: &str, luks_dev: &str, slot: u32)
           -> crate::error::Result<()>;   // unknown host => ConfigError
       pub fn pending_hosts(&self) -> Vec<String>;
       /// Returns the retire plan ONLY when pending_hosts() is empty; otherwise
       /// Err(SystemError listing the un-rebound hosts). This function never
       /// executes the retirement (server-side Tang key rotation is an operator
       /// action outside this binary) — it gates and describes it.
       pub fn retire_old_key(&self) -> crate::error::Result<String>;
   }
   ```

   Each `rebind_host` call is itself gated by `require_tang_quorum` against that host's executor (the NEW Tang key must already be advertised — quorum with the new key present proves re-bind can succeed).

5. **Unit tests** — extend the existing `#[cfg(test)] mod tests` (LK-01's recording MockExecutor). Required:

   | Test | Asserts |
   |---|---|
   | `test_quorum_counts_valid_advs` | 3 mocked adv responses, one failing → `check_tang_quorum` == 2 |
   | `test_quorum_fail_closed` | 1-of-3 valid, no override → `require_tang_quorum` Err naming counts; downstream `revoke_fido2` records ZERO wipe commands |
   | `test_override_exact_hostname_only` | valid<2: `Some("len-serv-001")` for target `len-serv-001` → Ok; `Some("yes")`, `Some("")`, `Some("LEN-SERV-001")` → Err |
   | `test_revoke_last_method_guard` | header with exactly 1 fido2 token, no override → `ConfigError`, ZERO wipe commands; with exact-hostname override → wipe proceeds |
   | `test_revoke_sets_revoked_at` | 2-of-3 quorum, 2 tokens → wipe command recorded once with the right slot; state record's `revoked_at` is Some; file written via `.tmp`+rename |
   | `test_rotate_order_enroll_then_revoke` | recorded command sequence: luksDump(before), cryptenroll(enroll), luksDump(after), wipe-slot — enroll strictly BEFORE any wipe |
   | `test_rotate_enroll_failure_keeps_old` | enroll leg errors (no new token) → rotate returns Err, ZERO wipe commands recorded, old state record unchanged |
   | `test_tang_rotation_gates_retire` | `TangRotation::new(3 hosts)`; after 2 rebinds `retire_old_key()` → Err listing the 3rd host; after all 3 → Ok(plan string) |
   | `test_tang_rotation_empty_fleet_refuses` | `new(&[])` → ConfigError |
   | `test_rotate_happy_path` | **anti-over-suppression:** full 3-of-3 quorum, 3 tokens, valid passphrase → `rotate_fido2` Ok; new record appended, old record `revoked_at` set — the guard stack does not block a legitimate rotation |

6. Bump the file header (version + last-edited) on every file you touch; new files get a fresh 4-line header with a new uuid4 (`uuidgen | tr 'A-F' 'a-f'`). (Here: `crates/uaa-core/src/luks_keys.rs` version bumped again, guid kept.)

## How to test

```bash
cargo test --lib --offline && cargo build --offline
# Expected: all tests pass (baseline 311 + LK-01's tests + your ~10 new tests), build clean
cargo clippy --offline -- -D warnings
# Expected: no warnings
cargo test --lib --offline luks_keys
# Expected: ~20 passed; 0 failed (LK-01 + LK-02 module tests)
grep -rn "std::process::Command" crates/uaa-core/src/luks_keys.rs
# Expected: 0 hits
grep -c "wipe-slot" crates/uaa-core/src/luks_keys.rs
# Expected: small count, ALL inside functions gated by require_tang_quorum (manual review)
```

## Acceptance criteria

- [ ] Destructive API present: `grep -n "pub async fn revoke_fido2\|pub async fn rotate_fido2\|pub struct TangRotation\|pub async fn require_tang_quorum" crates/uaa-core/src/luks_keys.rs` → 4 hits.
- [ ] Ordering encoded: `test_rotate_order_enroll_then_revoke` asserts the recorded-command SEQUENCE (enroll strictly before any `--wipe-slot`); `test_rotate_enroll_failure_keeps_old` proves zero wipes on enroll failure.
- [ ] Quorum fail-closed: `test_quorum_fail_closed` asserts ZERO destructive commands recorded when <2 Tang advs are valid and no override is given.
- [ ] Override is typed-hostname-exact: `test_override_exact_hostname_only` rejects `"yes"`, empty, and case-mismatched strings.
- [ ] Sweep-before-retire unreachable-early: `test_tang_rotation_gates_retire` proves `retire_old_key()` errs while any host is pending; `grep -n "retire_old_key" crates/uaa-core/src/luks_keys.rs` shows no executor call inside it (it returns a plan string only).
- [ ] Last-method guard: `test_revoke_last_method_guard` passes (refuse without override, proceed with exact hostname).
- [ ] Anti-over-suppression: `test_rotate_happy_path` passes — a healthy 3-of-3 fleet rotation goes through every guard.
- [ ] LK-01 surface untouched: `grep -n "pub async fn enroll_fido2\|pub fn parse_fido2_tokens" crates/uaa-core/src/luks_keys.rs` still → 2 hits with unchanged signatures (`git diff origin/main -- crates/uaa-core/src/luks_keys.rs` shows additions, not rewrites of LK-01 functions).
- [ ] `cargo test --lib --offline && cargo build --offline` exits 0; clippy clean (`cargo clippy --offline -- -D warnings`).
- [ ] File headers bumped on every changed file (`git diff origin/main --stat` files each show a version bump; guids unchanged).

## Commit message

```
feat(luks): rotate/revoke/rotate-tang with t=2-of-3 Tang cold-start guard (ws7-luks)

Extends crates/uaa-core/src/luks_keys.rs: revoke_fido2 (wipe-slot with
last-method guard), rotate_fido2 (enroll-new-THEN-revoke-old, never
reverse — enroll failure leaves the old credential untouched), and the
TangRotation state machine that makes retire-before-sweep unreachable
(retire_old_key errs while any fleet host is un-rebound). Every
destructive path is gated by require_tang_quorum: >=2 of 3 Tang advs
(172.16.2.45/46/47) must validate, fail-closed, override only by typing
the exact target hostname. All cryptsetup/cryptenroll/clevis/curl calls
via CommandExecutor mocks; LUKS disk unlock only, never auth. ~10 tests
incl. command-sequence ordering assertions.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

Additive (extends LK-01's module): if `grep -n "pub async fn rotate_fido2" crates/uaa-core/src/luks_keys.rs` hits, the task is already applied — run the Acceptance checks instead of re-applying. Rollback = revert the single commit; LK-01's enroll/status surface and the state-file contract stay untouched (this commit only appends functions/tests to the same file), and no host, header, or Tang server holds any state to unwind — the destructive paths exist in code but are dormant unless invoked.
