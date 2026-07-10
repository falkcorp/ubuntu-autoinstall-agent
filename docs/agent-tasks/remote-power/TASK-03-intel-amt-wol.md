<!-- file: docs/agent-tasks/remote-power/TASK-03-intel-amt-wol.md -->
<!-- version: 1.0.0 -->
<!-- guid: 0fcf14ed-ce27-42da-a621-be0cee8a22ad -->
<!-- last-edited: 2026-07-10 -->

# TASK-03 — Intel AMT (wsman) + Wake-on-LAN (server-side wakeonlan via ssh) replacing stubs (ws8-power)

**Priority:** P2 · **Effort:** S · **Recommended subagent:** Sonnet-class · rust-protocol subagent · **Why:** two small protocol paths behind the executor seam · **Depends on:** CP-03 (wave-3 gated: CP-01 workspace transform AND CP-03 fleet-config must both be MERGED to `origin/main` first — CP-01 creates the stub file this task fills). Parallel-safe with RP-02 (disjoint files: `amt_wol.rs` vs `dash.rs`).

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/remote-power-intel-amt-wol" -b agent/remote-power-intel-amt-wol origin/main
cd "$REPO/.worktrees/remote-power-intel-amt-wol"
git rebase origin/main
```

(Protocol also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

## Goal

Fill EXACTLY ONE file — the CP-01-created stub `crates/uaa-core/src/power/amt_wol.rs` — with two small power paths replacing the `IntelAmt` and `WakeOnLan` NotImplemented stubs, per spec Decision 15 (power stays a uaa-core library; remote execution via the server) and Decision 17 (workspace layout): (a) **Intel AMT** via `wsman invoke ... RequestPowerStateChange` against the AMT firmware CIM service (mechanism only — no credentials exist in any file; the runtime password arrives via `UAA_AMT_PASSWORD` / a `--amt-password`-style option that CP-01's CLI pre-wiring routes through), and (b) **Wake-on-LAN** sending magic packets FROM the server (`wakeonlan <MAC>` executed on `172.16.2.30` over the executor seam — a Mac-local magic packet would not reach the fleet VLAN reliably; the server sits on it). Both paths run exclusively through `CommandExecutor`. WoL supports ONLY `on` — `off`/`status` are physically impossible over WoL and return a typed `ConfigError`, never a silent no-op. Purely additive outside `amt_wol.rs`: `mod.rs` already dispatches its `IntelAmt`/`WakeOnLan` arms into this file (CP-01 pre-wiring) — you do NOT edit `mod.rs`, the CLI, or any other file.

REUSE — do not invent parallels for any of these:

- **`CommandExecutor`** trait (`src/network/executor.rs` pre-move; verify: `grep -n "pub trait CommandExecutor" src/network/executor.rs`) — all remote execution. No new SSH wrapper, no `std::process::Command`.
- **Recording `MockExecutor` idiom** (`src/power/mod.rs` `#[cfg(test)]`; verify: `grep -n "struct MockExecutor" src/power/mod.rs`) — mirror in `#[cfg(test)]` in `amt_wol.rs`. No mocking crate.
- **`AutoInstallError::ConfigError` / `AutoInstallError::SystemError`** (`src/error.rs`). No new error enum or variant.
- **`PowerAction`** (`src/power/mod.rs` — exactly `On`/`Off`/`Status`). Do NOT extend it.
- **Single-quote-reject rule** (`src/power/mod.rs` `build_ipmi_command`; verify: `grep -n "must not contain a single-quote" src/power/mod.rs`) — identical rule for the AMT password and for MAC validation (reject, never escape).

**Path map:** after CP-01 (wave 1) merges, `src/**` lives at `crates/uaa-core/src/**` and the CLI at `crates/uaa/src/**`. The greps below cite pre-move paths (verifiable on today's main); at execution time run them at the old path, then the mapped path. Zero hits at BOTH = STOP and report.

**HARD RULES (non-negotiable):**
- NO hardware actions. Validate ONLY in-repo (`cargo`) and, where a brief says so, the QEMU+swtpm harness (`scripts/vm-validate.sh`). Code that COULD touch hardware is written and unit-tested against mock executors only.
- NEVER wipe, write to, or deploy on 172.16.2.30 ("the server") or len-serv-003.
- `disk_device` is read from the live target at runtime, never guessed or hardcoded.
- ipmitool runs via `ssh 172.16.2.30`, never on macOS.
- NEVER power on unimatrixone (U1).
- No real secret in any file: `REPLACE_AT_PLACE_TIME` placeholders stay placeholders.
- Stay inside your worktree; never `git push`, `gh pr`, or merge — report done and stop.

## Background (verify before editing)

- Intel AMT speaks the same DMTF CIM wsman surface as DASH: `RequestPowerStateChange` on `CIM_PowerManagementService`, port 16992 (HTTP digest), PowerState `2` = On, `8` = Off (hard), `10` = reset (FORBIDDEN — unrepresentable here); status reads `CIM_AssociatedPowerManagementService`. No fleet host is registered as `IntelAmt` today (the M715qs turned out to be AMD DASH — see `docs/agent-tasks/DEFERRED.md`); this path is mechanism-only, ready for the first Intel host added to the registry/fleet config (CP-03). Command shapes carry `VERIFY-ON-HW` comments; NO live validation is possible or permitted.
- Wake-on-LAN: the server (`172.16.2.30`) has L2 adjacency to the fleet; the magic packet is sent by running `wakeonlan <MAC>` ON THE SERVER via the executor. The target's MAC comes from the caller (registry/fleet config per CP-03) — this module validates and uses it, it never guesses one.
- **Edge-case semantics (repeated in Step-by-step and Acceptance):**
  - WoL + `PowerAction::Off` or `Status` → `Err(ConfigError)` whose message contains "Wake-on-LAN" and "only supports 'on'". No command sent. Never a silent success.
  - WoL for hostname `unimatrixone` (any casing) → `Err(ConfigError)` — the U1 power-on deny-list (spec C1: fleet constants incl. "the `unimatrixone` power deny-list") is enforced HERE too, fail-closed, before any executor call. Same guard on the AMT `On` path.
  - MAC not matching `^[0-9a-fA-F]{2}(:[0-9a-fA-F]{2}){5}$` → `Err(ConfigError)`; REJECT, never sanitize (no shell-injection surface). No command sent.
  - AMT password `None`/empty → `ConfigError` naming BOTH supply mechanisms (`UAA_AMT_PASSWORD` and the flag). Password containing `'` → `ConfigError`. No command sent in either case.
  - Nonzero remote exit surfaces as the executor's existing error — do not wrap or retry.
- **Re-verify these anchors before editing** — line numbers drift; zero hits at both old and mapped path = STOP and report:
  ```bash
  grep -n "IntelAmt\|WakeOnLan" src/power/mod.rs
  # expect: 2+ hits (variants + dispatch arms; 4 hits on today's main)
  grep -n "pub trait CommandExecutor" src/network/executor.rs
  # expect: 1 hit
  grep -n "struct MockExecutor" src/power/mod.rs
  # expect: 1 hit (recording mock idiom to mirror)
  grep -n "pub enum PowerAction" src/power/mod.rs
  # expect: 1 hit (On/Off/Status only)
  grep -n "must not contain a single-quote" src/power/mod.rs
  # expect: 1 hit (the reject-don't-escape rule you copy)
  grep -rn "amt_wol" crates/uaa-core/src/power/ 2>/dev/null || grep -rn "amt_wol" src/power/ 2>/dev/null
  # expect: hits — the CP-01 stub file power/amt_wol.rs exists at the mapped path;
  # read its EXACT public fn signatures before writing (mod.rs already calls them)
  grep -rn "wakeonlan\|etherwake" src/ crates/ 2>/dev/null
  # expect: 0 hits outside power/amt_wol.rs before this task (greenfield inside the stub)
  ```

## Step-by-step

1. Run the ⛔ START HERE block, then every anchor grep above (old path, then mapped path). Any anchor with zero hits at both = STOP and report.

2. Open `crates/uaa-core/src/power/amt_wol.rs` and read the CP-01 stub: expected public entry points `run_amt_action(executor, hostname, target_ip, username, password: Option<&str>, action) -> Result<String>` and `run_wol_action(executor, hostname, mac, action) -> Result<String>` (or close — **the stub's actual signatures are authoritative**; `mod.rs` already calls them; adapt names below, never change signatures). Keep the file's existing header, bump `version` minor, `last-edited: 2026-07-10`, keep its guid.

3. AMT half — pure builders + dispatcher (each command string commented `// VERIFY-ON-HW`):

   ```rust
   pub const AMT_PORT: u16 = 16992;

   /// 2 = On, 8 = Off (hard). Status is a read — None. 10/reset unrepresentable.
   pub fn amt_power_state(action: PowerAction) -> Option<u8>;

   pub fn build_amt_power_command(target_ip: &str, username: &str, password: &str,
                                  action: PowerAction) -> Result<String>;
   pub fn redacted_amt_command(target_ip: &str, username: &str,
                               action: PowerAction) -> String;
   ```

   - On/Off shape: `wsman invoke -h <target_ip> -P 16992 -u <username> -p '<password>' -a RequestPowerStateChange http://schemas.dmtf.org/wbem/wscim/1/cim-schema/2/CIM_PowerManagementService -k PowerState=<2|8>`; Status shape: `wsman enumerate -h <target_ip> -P 16992 -u <username> -p '<password>' http://schemas.dmtf.org/wbem/wscim/1/cim-schema/2/CIM_AssociatedPowerManagementService`.
   - Builder rejects empty password and `'`-bearing password with `ConfigError` (copy the mod.rs rule).
   - Dispatcher `run_amt_action` order: (1) deny-list — `hostname.eq_ignore_ascii_case("unimatrixone")` AND `action == PowerAction::On` → `ConfigError` containing "unimatrixone" and "deny"; (2) password present/valid; (3) log ONLY `redacted_amt_command(...)` via `tracing::info!`; (4) `executor.execute_with_output(&cmd).await`. Every `Err` in (1)–(2) returns before any executor call.

4. WoL half:

   ```rust
   /// Strict colon-separated MAC. Reject, never sanitize.
   pub fn validate_mac(mac: &str) -> Result<()>;

   /// Command run ON THE SERVER. No credentials involved.
   pub fn build_wol_command(mac: &str) -> Result<String>;   // "wakeonlan <mac>"
   ```

   - Dispatcher `run_wol_action` order: (1) `action != PowerAction::On` → `ConfigError` containing "Wake-on-LAN" and "only supports 'on'"; (2) deny-list — hostname `unimatrixone` (case-insensitive) → `ConfigError`; (3) `validate_mac`; (4) `tracing::info!` the command (it contains no secret — loggable as-is); (5) `executor.execute_with_output(&cmd).await` and return stdout verbatim. Every `Err` in (1)–(3) returns before any executor call.

5. `#[cfg(test)] mod tests` with the recording mock (HashMap responses + `recorded: Vec<String>`). Passwords in tests are obviously fake (`"test-secret"`); MACs are documentation-style (`"aa:bb:cc:dd:ee:ff"`). Required tests:

   | Test | Asserts |
   |---|---|
   | `test_amt_power_state_mapping` | `On→Some(2)`, `Off→Some(8)`, `Status→None`; no builder output for any action contains `PowerState=10`, `reset`, or `cycle` |
   | `test_build_amt_command_shape` | Off contains `wsman invoke -h 172.16.3.99 -P 16992` + `PowerState=8`; Status uses `enumerate` + `CIM_AssociatedPowerManagementService` |
   | `test_amt_rejects_bad_password` | empty and `"a'b"` → `Err(ConfigError)`; and `run_amt_action` with `None` password records 0 commands |
   | `test_redacted_amt_omits_password` | redacted form contains neither `test-secret` nor `-p 'test-secret'`; still contains the target ip |
   | `test_amt_denies_unimatrixone_on` | `run_amt_action("unimatrixone", ..., PowerAction::On, Some("test-secret"))` → `Err` containing `unimatrixone`; 0 commands recorded; `UNIMATRIXONE` (uppercase) also denied |
   | `test_validate_mac` | `"aa:bb:cc:dd:ee:ff"` ok; `"aabb.ccdd.eeff"`, `"aa:bb:cc:dd:ee"`, `"aa:bb:cc:dd:ee:ff; rm -rf /"` → `Err(ConfigError)` |
   | `test_wol_rejects_off_and_status` | `run_wol_action` with `Off` and with `Status` → `Err` containing `only supports 'on'`; 0 commands recorded |
   | `test_wol_denies_unimatrixone` | hostname `unimatrixone` + `On` → `Err`; 0 commands recorded |
   | `test_wol_on_happy_path` | **anti-over-suppression:** valid host + `On` + valid MAC against a mock returning `"Sending magic packet"` → `Ok`, and the single recorded command equals `build_wol_command("aa:bb:cc:dd:ee:ff").unwrap()` — the guard stack does not block the happy path |
   | `test_amt_status_happy_path` | **anti-over-suppression:** non-denied host + `Status` + `Some("test-secret")` → `Ok` with the mock's stdout; single recorded command equals the builder output |

6. Bump the file header (version + last-edited) on every file you touch; new files get a fresh 4-line header with a new uuid4 (`uuidgen | tr 'A-F' 'a-f'`). (Expected touched set: `crates/uaa-core/src/power/amt_wol.rs` ONLY.)

## How to test

```bash
cargo test --lib --offline && cargo build --offline
# Expected: all tests pass (baseline 311 + your ~10 new amt/wol tests; earlier waves may add more — never fewer), build clean
cargo clippy --offline -- -D warnings
# Expected: no warnings
grep -rn "PowerState=10\|power reset\|power cycle" crates/uaa-core/src/power/
# Expected: 0 hits
grep -rn "std::process::Command" crates/uaa-core/src/power/
# Expected: 0 hits (executor seam only)
git diff origin/main --stat
# Expected: exactly one source file changed: crates/uaa-core/src/power/amt_wol.rs
```

## Acceptance criteria

- [ ] Stubs replaced: `grep -n "pub fn build_amt_power_command\|pub fn build_wol_command\|pub fn validate_mac" crates/uaa-core/src/power/amt_wol.rs` → 3 hits, and a valid AMT/WoL request no longer returns the DEFERRED stub error.
- [ ] WoL on-only proven: `grep -n "test_wol_rejects_off_and_status" crates/uaa-core/src/power/amt_wol.rs` → 1 hit and it passes with ZERO recorded commands.
- [ ] unimatrixone deny-list proven on BOTH paths: `test_amt_denies_unimatrixone_on` and `test_wol_denies_unimatrixone` pass with ZERO recorded commands (case-insensitive).
- [ ] MAC injection surface closed: `test_validate_mac` passes; `grep -n "rm -rf" crates/uaa-core/src/power/amt_wol.rs` hits only inside the test.
- [ ] Reset unrepresentable: `grep -rn "PowerState=10\|chassis power reset\|power cycle" crates/uaa-core/src/power/` → 0 hits.
- [ ] No secret leakage: `test_redacted_amt_omits_password` passes; only the redacted AMT form is logged (`grep -n "tracing\|println" crates/uaa-core/src/power/amt_wol.rs` output reviewed).
- [ ] Single-file scope: `git diff origin/main --stat` shows only `crates/uaa-core/src/power/amt_wol.rs`.
- [ ] Anti-over-suppression: `test_wol_on_happy_path` AND `test_amt_status_happy_path` pass — legitimate requests flow through every guard.
- [ ] `cargo test --lib --offline && cargo build --offline` exits 0; clippy clean.
- [ ] File headers bumped on every changed file (`git diff origin/main --stat` files each show a version bump; guids unchanged).

## Commit message

```
feat(power): implement Intel AMT (wsman) + Wake-on-LAN paths (ws8-power)

Fill the CP-01 stub crates/uaa-core/src/power/amt_wol.rs: Intel AMT power
on/off/status via wsman RequestPowerStateChange / CIM enumerate on :16992
(PowerState 2=on, 8=off; 10/reset unrepresentable), and Wake-on-LAN sending
`wakeonlan <mac>` FROM the server (172.16.2.30) through the CommandExecutor
seam. WoL is on-only (off/status = typed ConfigError, never silent); strict
MAC validation rejects rather than sanitizes; the unimatrixone power-on
deny-list is enforced fail-closed on both paths before any command. AMT
password via UAA_AMT_PASSWORD/--amt-password, quote-rejected, never logged
(redacted twin only). Mock-executor tests only; shapes carry VERIFY-ON-HW
markers — no Intel host is registered yet (mechanism-only per DEFERRED.md).

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

Additive (stub-fill): if `grep -n "pub fn build_wol_command" crates/uaa-core/src/power/amt_wol.rs` hits, the task is already applied — run the Acceptance criteria checks instead of re-applying. Rollback = revert the single commit; `amt_wol.rs` returns to the CP-01 stub (loud DEFERRED error), `mod.rs`, the CLI wiring, the IPMI path, and the sibling `dash.rs` stay untouched, and no server-side, AMT-firmware, or NIC state exists to unwind (nothing live was ever contacted).
