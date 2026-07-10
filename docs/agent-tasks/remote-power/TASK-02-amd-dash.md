<!-- file: docs/agent-tasks/remote-power/TASK-02-amd-dash.md -->
<!-- version: 1.0.0 -->
<!-- guid: c454e365-e8ce-45dc-bb57-61cdc9f54cf7 -->
<!-- last-edited: 2026-07-10 -->

# TASK-02 — AMD DASH power path: dashcli-deb-first with wsman fallback, executor-mocked, replacing the stub (ws8-power)

**Priority:** P2 · **Effort:** M · **Recommended subagent:** Sonnet-class · rust-protocol subagent · **Why:** protocol/CLI fallback logic; NO hardware — mock-tested only · **Depends on:** CP-03 (wave-3 gated: CP-01 workspace transform AND CP-03 fleet-config must both be MERGED to `origin/main` first — CP-01 creates the stub file this task fills; CP-03 finalizes the registry/deny-list shape it plugs into)

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/remote-power-amd-dash" -b agent/remote-power-amd-dash origin/main
cd "$REPO/.worktrees/remote-power-amd-dash"
git rebase origin/main
```

(Protocol also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

## Goal

Fill EXACTLY ONE file — the CP-01-created stub `crates/uaa-core/src/power/dash.rs` — with a real AMD DASH (Lenovo M715q, Realtek RTL8111EPP NIC firmware) power path that replaces the `AmdDash` NotImplemented stub error for `len-serv-001/002/003`. Per spec Decision 15 (power stays a uaa-core library + `uaa power` CLI) and Decision 17 (workspace layout: `crates/uaa-core`), every remote command executes ON THE SERVER (`172.16.2.30`) over SSH through the existing `CommandExecutor` seam — never locally. The DASH path tries the AMD `dashcli` tool first (installed from the AMD .deb on the server, if present) and falls back to `wsman invoke` (DMTF CIM `RequestPowerStateChange`) when `dashcli` is absent. Purely additive to everything outside `dash.rs`: `crates/uaa-core/src/power/mod.rs` already dispatches its `AmdDash` arm into this file (CP-01 pre-wiring) — you do NOT edit `mod.rs`, the CLI, or any other file.

REUSE — do not invent parallels for any of these:

- **`CommandExecutor`** trait (`src/network/executor.rs` pre-move — `pub trait CommandExecutor`, methods `execute_with_output` returning captured stdout and `check_silent` returning bool; verify: `grep -n "pub trait CommandExecutor" src/network/executor.rs`). All remote execution goes through `&mut dyn CommandExecutor`. Do NOT write a new SSH wrapper and do NOT use `std::process::Command`.
- **Recording `MockExecutor` idiom** (`src/power/mod.rs` `#[cfg(test)]` — a HashMap command→response mock with a `recorded: Vec<String>`; verify: `grep -n "struct MockExecutor" src/power/mod.rs`). Mirror it inside `#[cfg(test)]` in `dash.rs` (or `use super::` it if CP-01 made it shareable). Do NOT add a mocking crate.
- **`stub_error` / fail-closed validation pattern** (`src/power/mod.rs` — `fn stub_error`, `validate_ipmi_request`; verify: `grep -n "fn stub_error\|validate_ipmi_request" src/power/mod.rs`). Your DASH validation follows the same shape: every failure returns `Err` BEFORE any executor call.
- **`AutoInstallError::ConfigError` / `AutoInstallError::SystemError`** (`src/error.rs`) for all error paths. Do NOT add a new error enum or variant.
- **`PowerAction`** (`src/power/mod.rs` — exactly `On`/`Off`/`Status`; reset/cycle are unrepresentable). Do NOT extend it.

**Path map:** after CP-01 (wave 1) merges, `src/**` lives at `crates/uaa-core/src/**` and the CLI at `crates/uaa/src/**`. The greps below cite pre-move paths (verifiable on today's main); at execution time run them at the old path, then the mapped path (`crates/uaa-core/src/power/mod.rs`, `crates/uaa-core/src/network/executor.rs`, ...). Zero hits at BOTH = STOP and report.

**HARD RULES (non-negotiable):**
- NO hardware actions. Validate ONLY in-repo (`cargo`) and, where a brief says so, the QEMU+swtpm harness (`scripts/vm-validate.sh`). Code that COULD touch hardware is written and unit-tested against mock executors only.
- NEVER wipe, write to, or deploy on 172.16.2.30 ("the server") or len-serv-003.
- `disk_device` is read from the live target at runtime, never guessed or hardcoded.
- ipmitool runs via `ssh 172.16.2.30`, never on macOS.
- NEVER power on unimatrixone (U1).
- No real secret in any file: `REPLACE_AT_PLACE_TIME` placeholders stay placeholders.
- Stay inside your worktree; never `git push`, `gh pr`, or merge — report done and stop.

## Background (verify before editing)

- Ground truth on DASH (from the M715q investigation, summarized in `docs/agent-tasks/DEFERRED.md` row "AMD DASH / Intel AMT driver + credential setup"): M715q is AMD Ryzen Pro — DASH lives in the Realtek NIC firmware on TCP port 16992, credentials are set by `DASHConfigRT` (user `Administrator` by default), and control from Linux is `wsman invoke ... RequestPowerStateChange` against `CIM_PowerManagementService` with DMTF PowerState values: `2` = On, `8` = Power Off (hard), `10` = reset (FORBIDDEN here). Status reads `PowerState` from `CIM_AssociatedPowerManagementService`.
- **len-serv-002 (and every other M715q) does NOT yet run the Linux DASH ClientTool/WSMAN service** — the driver + credentials are a deferred hardware task. Therefore: NO live validation is possible or permitted. All tests are mock-executor tests; the normative command strings below carry `VERIFY-ON-HW` comments so the first live session can confirm them. Do not "test it for real".
- The DASH mechanism's remote credential arrives at runtime via `UAA_DASH_PASSWORD` / a `--dash-password`-style option that CP-01's CLI pre-wiring already routes into the power dispatch (grep below). It is NEVER hardcoded and the built command string is NEVER logged (redacted twin only) — same locked posture as the IPMI path.
- Fallback semantics (the whole point of this task): probe the server for `dashcli` with `check_silent("command -v dashcli")`; if true, run the dashcli-form command; if false, run the wsman-form command. Exactly one power command is ever sent per invocation. Probe failure (executor error) propagates as `Err` — no blind fallback.
- Edge-case semantics (spelled again in Step-by-step and Acceptance): empty password → `ConfigError`, no command sent; password containing `'` → `ConfigError` (REJECT, never escape), no command sent; `PowerAction::Status` never maps to a `RequestPowerStateChange` (it is a read, not a state change); nonzero remote exit surfaces as the executor's existing error — do not wrap or retry.
- **Re-verify these anchors before editing** — line numbers drift; zero hits at both old and mapped path = STOP and report:
  ```bash
  grep -n "AmdDash" src/power/mod.rs
  # expect: 2+ hits (variant + dispatch arm; 7 hits on today's main)
  grep -n "pub trait CommandExecutor" src/network/executor.rs
  # expect: 1 hit
  grep -n "async fn execute_with_output\|async fn check_silent" src/network/executor.rs
  # expect: 2+ hits (trait methods)
  grep -n "struct MockExecutor" src/power/mod.rs
  # expect: 1 hit (recording mock idiom to mirror)
  grep -n "fn stub_error" src/power/mod.rs
  # expect: 1 hit
  grep -n "pub enum PowerAction" src/power/mod.rs
  # expect: 1 hit (On/Off/Status only)
  grep -rn "dash" crates/uaa-core/src/power/ 2>/dev/null || grep -rn "dash" src/power/ 2>/dev/null
  # expect: hits — the CP-01 stub file power/dash.rs exists at the mapped path with a
  # headered stub fn; read its EXACT signature before writing (mod.rs already calls it)
  grep -n "UAA_DASH_PASSWORD\|dash_password" crates/uaa/src/ -r 2>/dev/null; grep -rn "UAA_DASH_PASSWORD\|dash_password" src/cli/ 2>/dev/null
  # expect: hits after CP-01's CLI pre-wiring; if 0 hits at both paths, the password
  # reaches you as an Option<&str> parameter on the stub fn — use that and note it
  ```

## Step-by-step

1. Run the ⛔ START HERE block, then every anchor grep above (old path, then mapped path). Any anchor with zero hits at both = STOP and report.

2. Open `crates/uaa-core/src/power/dash.rs` and read the stub CP-01 left there: its public function (expected shape `pub async fn run_dash_action(executor: &mut dyn CommandExecutor, hostname: &str, target_ip: &str, username: &str, password: Option<&str>, action: PowerAction) -> Result<String>` or close to it — **the stub's actual signature is authoritative** because `mod.rs` already calls it; adapt the names below to it, never change the signature). Keep the file's existing header, bump `version` to the next minor and `last-edited: 2026-07-10`, keep its guid.

3. Add the constants and pure builders (all `pub` so tests hit them directly):

   ```rust
   /// DASH WSMAN service port (Realtek NIC firmware). VERIFY-ON-HW.
   pub const DASH_PORT: u16 = 16992;

   /// DMTF CIM PowerState for a state CHANGE. Status is a read — returns None.
   /// 2 = On, 8 = Power Off (hard). 10 (reset) is intentionally unrepresentable.
   pub fn dash_power_state(action: PowerAction) -> Option<u8>;

   /// Probe command run on the server to detect the AMD dashcli .deb install.
   pub fn dash_probe_command() -> String;         // "command -v dashcli"

   /// dashcli-form command (preferred when the probe hits). VERIFY-ON-HW.
   pub fn build_dashcli_command(target_ip: &str, username: &str, password: &str,
                                action: PowerAction) -> Result<String>;

   /// wsman-form fallback command. VERIFY-ON-HW.
   pub fn build_wsman_dash_command(target_ip: &str, username: &str, password: &str,
                                   action: PowerAction) -> Result<String>;

   /// Password-free twin of whichever command was built — the ONLY loggable form.
   pub fn redacted_dash_command(target_ip: &str, username: &str,
                                action: PowerAction, via_dashcli: bool) -> String;
   ```

   Normative built shapes (each with a `// VERIFY-ON-HW` comment in code):
   - wsman On/Off: `wsman invoke -h <target_ip> -P 16992 -u <username> -p '<password>' -a RequestPowerStateChange http://schemas.dmtf.org/wbem/wscim/1/cim-schema/2/CIM_PowerManagementService -k PowerState=<2|8>`
   - wsman Status: `wsman enumerate -h <target_ip> -P 16992 -u <username> -p '<password>' http://schemas.dmtf.org/wbem/wscim/1/cim-schema/2/CIM_AssociatedPowerManagementService`
   - dashcli On/Off/Status: `dashcli -h <target_ip>:16992 -u <username> -p '<password>' power <on|off|status>`
   - Both builders: empty password → `Err(AutoInstallError::ConfigError(..))`; password containing `'` → `Err(ConfigError)` — REJECT, do not escape (fail-closed, no shell-injection surface; identical rule to `build_ipmi_command`, verify: `grep -n "must not contain a single-quote" src/power/mod.rs`).
   - Redacted shape: same command with the `-p '<password>'` token replaced by `-p '<redacted>'` — output must contain neither the password nor any password characters.

4. Add the status parser as a pure function so it is trivially unit-testable:

   ```rust
   /// Map raw remote stdout to "on"/"off". wsman XML: find "PowerState>2<" → on,
   /// "PowerState>8<" (or 6) → off; dashcli: pass through a line containing
   /// "on"/"off". Anything unrecognizable → Err(SystemError) naming the raw output
   /// length, NEVER echoing credentials.
   pub fn parse_dash_status(raw: &str) -> Result<&'static str>;
   ```

5. Implement the dispatcher body (replacing the stub error), fail-closed — every `Err` below returns BEFORE any power command is sent (the probe is the only command allowed before validation completes, so validate FIRST, probe second):
   1. `password` `None` or empty → `ConfigError` naming BOTH supply mechanisms (`UAA_DASH_PASSWORD` and the `--dash-password` flag). No executor call.
   2. Password contains `'` → the builder's `ConfigError` propagates. No power command sent.
   3. Probe: `executor.check_silent(&dash_probe_command()).await?` → `true` = build via `build_dashcli_command`, `false` = build via `build_wsman_dash_command`. A probe transport error propagates — do NOT silently assume wsman.
   4. Log ONLY `redacted_dash_command(...)` via `tracing::info!` (mirror `run_power_action`'s comment discipline: `grep -n "NEVER log" src/power/mod.rs`). The built string must never reach any log/println/error.
   5. `executor.execute_with_output(&cmd).await?`; for `Status` return `parse_dash_status(&raw)?.to_string()`, for On/Off return the raw stdout verbatim. Nonzero remote exit = executor's existing error, unwrapped, no retry.

6. `#[cfg(test)] mod tests` in `dash.rs` with a recording mock (mirror the `mod.rs` idiom — HashMap responses + `recorded: Vec<String>`; `check_silent` returns true iff the preloaded response is non-empty). Passwords in tests are obviously fake (`"test-secret"`). Required tests:

   | Test | Asserts |
   |---|---|
   | `test_dash_power_state_mapping` | `On→Some(2)`, `Off→Some(8)`, `Status→None`; and for every action neither builder output contains `PowerState=10`, `reset`, nor `cycle` |
   | `test_build_wsman_command_shape` | Off for `172.16.3.92`/`Administrator`/`test-secret` contains `wsman invoke -h 172.16.3.92 -P 16992`, `RequestPowerStateChange`, `PowerState=8`; Status uses `enumerate` + `CIM_AssociatedPowerManagementService` and contains NO `RequestPowerStateChange` |
   | `test_build_dashcli_command_shape` | On contains `dashcli -h 172.16.3.92:16992` and `power on` |
   | `test_builders_reject_bad_password` | empty and `"a'b"` passwords → `Err(ConfigError)` from BOTH builders |
   | `test_redacted_omits_password` | redacted forms (both `via_dashcli` values) contain neither `test-secret` nor `'` -wrapped secret; still contain the target ip |
   | `test_run_dash_missing_password` | `None` password → `Err` naming `UAA_DASH_PASSWORD`; mock recorded 0 commands |
   | `test_run_dash_fallback_to_wsman` | probe response empty (dashcli absent) → the single recorded POWER command equals `build_wsman_dash_command(...)` output (recorded = [probe, wsman cmd]) |
   | `test_run_dash_prefers_dashcli` | probe response non-empty → recorded power command equals `build_dashcli_command(...)` output |
   | `test_parse_dash_status` | fixture wsman XML containing `<p:PowerState>2</p:PowerState>` → `"on"`; `>8<` → `"off"`; garbage → `Err` whose message does NOT contain `test-secret` |
   | `test_run_dash_status_happy_path` | **anti-over-suppression:** `Status` + `Some("test-secret")` against a mock preloaded with the probe (empty → wsman) and the wsman status command returning the `PowerState>2<` fixture → `Ok("on")` — the guard stack does not block the happy path |

7. Bump the file header (version + last-edited) on every file you touch; new files get a fresh 4-line header with a new uuid4 (`uuidgen | tr 'A-F' 'a-f'`). (Expected touched set: `crates/uaa-core/src/power/dash.rs` ONLY.)

## How to test

```bash
cargo test --lib --offline && cargo build --offline
# Expected: all tests pass (baseline 311 + your ~10 new dash tests; earlier waves may add more — never fewer), build clean
cargo clippy --offline -- -D warnings
# Expected: no warnings
grep -rn "PowerState=10\|power reset\|power cycle" crates/uaa-core/src/power/
# Expected: 0 hits
grep -rn "std::process::Command" crates/uaa-core/src/power/
# Expected: 0 hits (executor seam only)
git diff origin/main --stat
# Expected: exactly one source file changed: crates/uaa-core/src/power/dash.rs
```

## Acceptance criteria

- [ ] Stub replaced: `grep -n "DEFERRED.md" crates/uaa-core/src/power/dash.rs` → 0 hits in the non-test body's error path for a valid request, AND `grep -n "pub fn build_wsman_dash_command\|pub fn build_dashcli_command\|pub fn parse_dash_status" crates/uaa-core/src/power/dash.rs` → 3 hits.
- [ ] Fallback order proven: `grep -n "test_run_dash_prefers_dashcli\|test_run_dash_fallback_to_wsman" crates/uaa-core/src/power/dash.rs` → 2 hits and both pass in the suite.
- [ ] Fail-closed proven: `test_run_dash_missing_password` asserts the mock recorded ZERO commands; both builders reject empty/quote passwords (`test_builders_reject_bad_password` passes).
- [ ] Reset unrepresentable: `grep -rn "PowerState=10\|chassis power reset\|power cycle" crates/uaa-core/src/power/` → 0 hits.
- [ ] No secret leakage: `grep -n "test_redacted_omits_password" crates/uaa-core/src/power/dash.rs` → 1 hit and it passes; the built (password-bearing) string is never passed to `tracing`/`println!` (`grep -n "tracing\|println" crates/uaa-core/src/power/dash.rs` output reviewed — only the redacted form is logged).
- [ ] Single-file scope: `git diff origin/main --stat` shows only `crates/uaa-core/src/power/dash.rs` (mod.rs, CLI, and all other files untouched).
- [ ] Anti-over-suppression: `test_run_dash_status_happy_path` passes — a legitimate Status request flows through every guard and returns `Ok("on")`.
- [ ] `cargo test --lib --offline && cargo build --offline` exits 0; clippy clean.
- [ ] File headers bumped on every changed file (`git diff origin/main --stat` files each show a version bump; guids unchanged).

## Commit message

```
feat(power): implement AMD DASH path — dashcli-first with wsman fallback (ws8-power)

Fill the CP-01 stub crates/uaa-core/src/power/dash.rs: probe the server for
the AMD dashcli .deb (command -v via CommandExecutor.check_silent) and prefer
it; otherwise fall back to wsman invoke RequestPowerStateChange (PowerState
2=on, 8=off; 10/reset unrepresentable) against the Realtek DASH firmware on
:16992. All commands execute ON THE SERVER (172.16.2.30) through the
CommandExecutor seam; password via UAA_DASH_PASSWORD/--dash-password, quote-
rejected fail-closed, never logged (redacted twin only). Status parsed by a
pure parse_dash_status fixture-tested function. Mock-executor tests only —
len-serv-002 lacks the Linux WSMAN service, so command shapes carry
VERIFY-ON-HW markers per docs/agent-tasks/DEFERRED.md.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

Additive (stub-fill): if `grep -n "pub fn build_wsman_dash_command" crates/uaa-core/src/power/dash.rs` hits, the task is already applied — run the Acceptance criteria checks instead of re-applying. Rollback = revert the single commit; `dash.rs` returns to the CP-01 stub (loud DEFERRED error), `mod.rs`, the CLI wiring, the IPMI path, and all sibling stub files (`amt_wol.rs`) stay untouched, and no server-side or NIC-firmware state exists to unwind (nothing live was ever contacted).
