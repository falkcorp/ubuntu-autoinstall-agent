<!-- file: docs/agent-tasks/remote-power/TASK-01-power-subcommand-ipmi.md -->
<!-- version: 1.0.0 -->
<!-- guid: 9050b216-402a-46d0-a1dc-ae853c684f41 -->
<!-- last-edited: 2026-07-09 -->

# TASK-01 — uaa power <host> on|off|status: machine-class dispatch scaffolding + IPMI-via-server path (todo:remote-power)

**Priority:** P2 · **Effort:** M · **Recommended subagent:** Sonnet-class · rust-cli subagent · **Why:** new isolated module + thin CLI wiring, purely additive; IPMI runs remotely per hard op rule and everything is unit-testable against the existing CommandExecutor mock seam. · **Depends on:** none (wave-5 gated: `installer-robustness/TASK-02/-03/-07` and `phase-rerun/TASK-01` must be MERGED first — they edit the same CLI files)

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/remote-power-power-subcommand-ipmi" -b agent/remote-power-power-subcommand-ipmi origin/main
cd "$REPO/.worktrees/remote-power-power-subcommand-ipmi"
git rebase origin/main
```

(Protocol is also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

## Goal

Add `uaa power <hostname> <on|off|status>`: a NEW module `src/power/mod.rs` holding a `PowerMechanism` enum (`Ipmi { bmc_host, username }`, `AmdDash`, `IntelAmt`, `WakeOnLan` — only `Ipmi` implemented; the other three return a typed NotImplemented-style error naming `docs/agent-tasks/DEFERRED.md`), a hardcoded host registry (`unimatrixone` → IPMI via BMC `172.16.3.150` user `ADMIN`; `len-serv-001/002/003` → `AmdDash` stub), and an execution path that SSHes to the server `172.16.2.30` and runs `ipmitool` THERE — never locally. Wire it into the CLI: `pub mod power;` in `src/lib.rs`, a `Commands::Power` variant in `src/cli/args.rs`, a fully-qualified match arm in `src/main.rs`, and a `power_command` handler in `src/cli/commands.rs`.

REUSE — do not invent parallels for any of these:

- **`SshClient`** (`src/network/ssh.rs` — `pub struct SshClient`, `pub async fn connect(&mut self, host: &str, username: &str)`) for the connection to `172.16.2.30`. Do NOT write a new SSH wrapper or shell out to `ssh` via `std::process::Command`.
- **`CommandExecutor`** trait (`src/network/executor.rs` — the proven test seam; `execute_with_output` returns captured stdout). `run_power_action` takes `&mut dyn CommandExecutor` so tests inject a mock.
- **`AutoInstallError::ConfigError` / `AutoInstallError::SystemError`** (`src/error.rs`) for all error paths. Do NOT add a new error enum or variant.
- **Mock idiom:** mirror `MockExecutor` (`src/autoinstall/verify.rs`, a HashMap command→response test mock implementing `CommandExecutor`) inside `#[cfg(test)]` in `src/power/mod.rs`. Do NOT add a mocking crate.
- Define a new `pub const POWER_SERVER: &str = "172.16.2.30";` in `src/power/mod.rs`. Do NOT reuse `COCKROACH_SERVER_IP` (`src/autoinstall/host_spec.rs`) — same value, different semantic role (locked design decision).

Naming note: the design spec's data-model sketch (`docs/specs/remote-power-design.md`) calls the enum `PowerMethod` with fields `bmc`/`user`; **this brief's names are authoritative**: `PowerMechanism`, fields `bmc_host`/`username`, plus the `IntelAmt` and `WakeOnLan` stub variants (the spec's Goals and Decision 6 require the AMT/WoL arms). Same semantics otherwise — every other spec decision is LOCKED and restated inline below.

## Background (verify before editing)

- Design spec: `docs/specs/remote-power-design.md` (decisions LOCKED); plan: `docs/specs/remote-power-plan.md`. This brief is self-contained — everything you need is inline.
- There is NO power/IPMI/WoL code anywhere in `src/` today (scout-verified, grep below hits 0). This task is greenfield module + wiring, purely additive: do not modify any existing subcommand, match arm, handler, or import beyond appending the new items and bumping file headers.
- CLI shape: `pub enum Commands` in `src/cli/args.rs` (`#[derive(Subcommand)]`); dispatch is a single `match cli.command` in `src/main.rs` whose arms use the fully-qualified `ubuntu_autoinstall_agent::cli::args::Commands::<Variant>` idiom (e.g. the `LocalInstall` arm); handlers are `pub async fn *_command` functions in `src/cli/commands.rs` (e.g. `ssh_install_command`). Follow that idiom exactly.
- Server-connection idiom to copy: `place_command` in `src/cli/commands.rs` does `SshClient::new()` then `connect(server, server_user)` — mirror that shape in `power_command`, connecting to `POWER_SERVER`.

**HARD RULES (operation contract — non-negotiable):**

1. **NEVER run `ipmitool` locally on macOS.** It crashes silently against Supermicro BMCs (empty output, no error — this cost a full debugging day on 2026-07-09). The ONLY execution path is over SSH to the server `172.16.2.30`, running `ipmitool` there. No `std::process::Command("ipmitool")` may exist anywhere in this change.
2. **Explicit `power off` / `power on` / `status` only — NO `reset`/`cycle`** (unreliable on the X10DSC+). Make the unreliable verbs UNREPRESENTABLE: the `PowerAction` enum has exactly three variants, so clap rejects `reset`/`cycle` at parse time.
3. **SECRETS:** the BMC password comes from the `UAA_IPMI_PASSWORD` env var or the `--ipmi-password` flag at runtime. It is NEVER hardcoded in source, tests, fixtures, or docs committed to git (the real credential lives in project memory only — do not copy it here or into code). The built command string embeds the password and MUST NEVER be logged — log only the redacted form (see Step 4).
4. **Code/docs only.** Do not run any live command against any BMC, the server `172.16.2.30`, or any host. Validation is unit tests + build. NEVER touch 172.16.2.30's state or len-serv-003.
5. You are in wave 5: four earlier tasks edited `src/cli/args.rs`, `src/main.rs`, and `src/cli/commands.rs`, so every line number below WILL have drifted. The grep hits are authoritative; a zero-hit grep means STOP and report.

**Re-verify these anchors** — line numbers drift, they are a starting point only:

```bash
grep -n "pub enum Commands" src/cli/args.rs
# expect: 1 hit at line 27
grep -n "Commands::" src/main.rs
# expect: 13 hits, match arms from line 36 (CreateImage) through line 162 (RenderUserData); Commands::LocalInstall at line 111
# (wave 4 adds arms/flags — >=13 hits and drifted lines are fine; the fully-qualified idiom is what you copy)
grep -n "pub async fn ssh_install_command" src/cli/commands.rs
# expect: 1 hit at line 275
grep -n "pub trait CommandExecutor" src/network/executor.rs
# expect: 1 hit at line 11
grep -rni "ipmi\|wake.on.lan\|\bwol\b\|etherwake\|magic packet\|chassis power" src/ --include="*.rs"
# expect: 0 hits
grep -n "COCKROACH_SERVER_IP" src/autoinstall/host_spec.rs
# expect: 1+ hits, const definition at line 16
grep -n "struct MockExecutor" src/autoinstall/verify.rs
# expect: 1 hit at line 527
# Symbol anchors (no line numbers — re-locate by symbol):
grep -n "pub struct SshClient" src/network/ssh.rs              # expect: 1 hit (~line 14)
grep -n "pub async fn connect" src/network/ssh.rs              # expect: 1 hit (~line 49)
grep -n "pub async fn place_command" src/cli/commands.rs       # expect: 1 hit — copy its SshClient::new()+connect shape
grep -n "pub mod" src/lib.rs                                    # expect: ~9 hits — append pub mod power; alphabetically
grep -n "ConfigError\|SystemError" src/error.rs                 # expect: hits — the two variants you use
grep -n "async fn execute_with_output" src/network/executor.rs  # expect: hits — trait method returning Result<String>
```

## Step-by-step

1. Run the ⛔ START HERE block, then the anchor greps above. Any zero-hit grep → STOP and report.

2. **Create `src/power/mod.rs`** with a fresh 4-line header (`// file: src/power/mod.rs`, `// version: 1.0.0`, a NEW guid you generate with `uuidgen | tr A-F a-f`, `// last-edited: 2026-07-09`), containing:

   ```rust
   /// Power action requested on the CLI. Exactly three variants — reset/cycle
   /// are intentionally UNREPRESENTABLE (unreliable on the X10DSC+).
   #[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
   pub enum PowerAction {
       On,
       Off,
       Status,
   }

   /// How a given machine is power-controlled. Only Ipmi is implemented;
   /// the other mechanisms return a NotImplemented-style error naming
   /// docs/agent-tasks/DEFERRED.md.
   #[derive(Debug, Clone, PartialEq, Eq)]
   pub enum PowerMechanism {
       /// Supermicro-style BMC via ipmitool lanplus — executed ON THE SERVER
       /// (172.16.2.30) over SSH, never locally (macOS ipmitool crashes
       /// silently against Supermicro BMCs).
       Ipmi {
           /// BMC IP as reachable from the server.
           bmc_host: &'static str,
           /// BMC username. The password is NEVER stored here — it arrives
           /// at runtime via --ipmi-password / UAA_IPMI_PASSWORD.
           username: &'static str,
       },
       /// AMD DASH (Lenovo M715q) — UNIMPLEMENTED stub.
       AmdDash,
       /// Intel AMT — UNIMPLEMENTED stub.
       IntelAmt,
       /// Wake-on-LAN — UNIMPLEMENTED stub.
       WakeOnLan,
   }

   /// The host that runs ipmitool for us. Deliberately NOT the same constant
   /// as COCKROACH_SERVER_IP (same value, different role).
   pub const POWER_SERVER: &str = "172.16.2.30";

   /// Hardcoded v1 host registry. Returns None for unknown hostnames.
   pub fn lookup_host(hostname: &str) -> Option<PowerMechanism> {
       match hostname {
           "unimatrixone" => Some(PowerMechanism::Ipmi {
               bmc_host: "172.16.3.150",
               username: "ADMIN",
           }),
           "len-serv-001" | "len-serv-002" | "len-serv-003" => Some(PowerMechanism::AmdDash),
           _ => None,
       }
   }
   ```

3. **In the same file, add the command builder + its redacted twin.** The password travels as an `IPMI_PASSWORD=...` env-var prefix consumed by `ipmitool -E` — NOT as a `-P` argv token (a `-P` token is visible in the server's `ps` output; this is a locked design decision, do not "simplify" back to `-P`).

   ```rust
   /// Full command executed ON THE SERVER. NEVER log this string — it embeds
   /// the password. Log redacted_ipmi_command() instead.
   pub fn build_ipmi_command(bmc_host: &str, username: &str, password: &str,
                             action: PowerAction) -> crate::error::Result<String>;

   /// Password-free form of the same command, safe for logs and errors.
   pub fn redacted_ipmi_command(bmc_host: &str, username: &str,
                                action: PowerAction) -> String;
   ```

   - Built shape (normative): `IPMI_PASSWORD='<pw>' ipmitool -E -I lanplus -H <bmc_host> -U <username> chassis power <on|off|status>`. Map `PowerAction::On→"on"`, `Off→"off"`, `Status→"status"` — these three strings are the ONLY `chassis power` verbs the module may ever emit.
   - If `password` contains a single quote (`'`), return `Err(AutoInstallError::ConfigError(...))` — REJECT, do not escape (fail-closed, no shell-injection surface). An empty password is also a `ConfigError`.
   - Redacted shape: `ipmitool -E -I lanplus -H <bmc_host> -U <username> chassis power <action>` — no `IPMI_PASSWORD=` prefix, no password characters.

4. **Add the dispatcher** `pub async fn run_power_action(executor: &mut dyn crate::network::CommandExecutor, hostname: &str, action: PowerAction, ipmi_password: Option<&str>) -> crate::error::Result<String>` (adjust the trait path to wherever `CommandExecutor` is re-exported — check `src/network/mod.rs`). Semantics — every failure is FAIL-CLOSED (returns Err BEFORE any executor call; the mock must record zero commands on these paths):
   - `lookup_host` → `None`: `AutoInstallError::ConfigError` listing the known hostnames (`unimatrixone`, `len-serv-001`, `len-serv-002`, `len-serv-003`). No command sent.
   - `AmdDash` / `IntelAmt` / `WakeOnLan`: `AutoInstallError::SystemError` whose message contains the mechanism name, the hostname, "not implemented", and the literal path `docs/agent-tasks/DEFERRED.md`. No command sent.
   - `Ipmi` with `ipmi_password` `None` or empty: `AutoInstallError::ConfigError` naming BOTH supply mechanisms (`UAA_IPMI_PASSWORD` and `--ipmi-password`). No command sent.
   - `Ipmi` with a password: build the command via `build_ipmi_command` (its `'`-rejection propagates), log ONLY `redacted_ipmi_command(...)` (via the existing `tracing`/log macros the codebase uses — never the built string), run it with `executor.execute_with_output(&cmd).await`, and return the captured stdout (e.g. `"Chassis Power is on"`) verbatim. A nonzero remote exit surfaces as the executor's existing error — do not wrap or retry.

5. **Register the module:** in `src/lib.rs` add `pub mod power;` in alphabetical position among the existing `pub mod` lines. Touch nothing else in the file except its version header bump.

6. **CLI variant** — append to `pub enum Commands` in `src/cli/args.rs` (do not modify existing variants):

   ```rust
   /// Remote power control (IPMI runs on the server 172.16.2.30 — never locally)
   Power {
       /// Target hostname (must be in the built-in registry, e.g. unimatrixone)
       hostname: String,

       /// on | off | status (reset/cycle intentionally unsupported)
       #[arg(value_enum)]
       action: crate::power::PowerAction,

       /// BMC password; falls back to $UAA_IPMI_PASSWORD. Never hardcode.
       #[arg(long, env = "UAA_IPMI_PASSWORD", hide_env_values = true)]
       ipmi_password: Option<String>,
   },
   ```

   If the crate's clap features do not support `env = ...` (build error), drop the `env`/`hide_env_values` attrs and instead read `std::env::var("UAA_IPMI_PASSWORD").ok()` as the fallback inside `power_command` — the observable behavior (flag wins, env is fallback) must hold either way.

7. **Match arm in `src/main.rs`** — append one arm in the same fully-qualified style as the existing arms (verify with `grep -n "ubuntu_autoinstall_agent::cli::args::Commands::" src/main.rs` — copy the `LocalInstall` arm's shape):

   ```rust
   ubuntu_autoinstall_agent::cli::args::Commands::Power {
       hostname,
       action,
       ipmi_password,
   } => power_command(&hostname, action, ipmi_password.as_deref()).await,
   ```

   (Match the surrounding arms' exact call/`await`/error-handling shape — some arms wrap the call; mirror whichever pattern `LocalInstall` uses after wave 4's edits.)

8. **Handler in `src/cli/commands.rs`** — append `pub async fn power_command(hostname: &str, action: crate::power::PowerAction, ipmi_password: Option<&str>) -> Result<()>`:
   - FIRST validate without touching the network: `lookup_host(hostname)` must be `Some(PowerMechanism::Ipmi{..})` and the password must be present — otherwise return the same errors as `run_power_action` (call a small shared pre-check or just call `run_power_action` with a never-connecting executor — simplest correct option: perform lookup + stub + password checks in `run_power_action` itself and connect ONLY after a local pre-validation pass in `power_command` that repeats lookup/stub/password checks). The requirement: unknown host, stub mechanism, and missing password MUST all fail without any SSH connection attempt.
   - Then mirror `place_command`'s idiom: `let mut client = SshClient::new(); client.connect(crate::power::POWER_SERVER, <same server username place_command uses — re-read place_command for the current value>).await?;`, call `run_power_action(&mut client, hostname, action, ipmi_password).await?`, `println!` the returned outcome line, disconnect/drop as `place_command` does.

9. **Unit tests** — `#[cfg(test)] mod tests` at the bottom of `src/power/mod.rs`, with a local `MockExecutor` mirroring the `verify.rs` idiom PLUS a `Vec<String>` of recorded commands so zero-command assertions work. Passwords in tests are obviously fake (`"test-secret"`); NEVER the real credential. Required tests:

   | Test | Asserts |
   |---|---|
   | `test_lookup_host_registry` | `unimatrixone` → `Ipmi{bmc_host:"172.16.3.150",username:"ADMIN"}`; each of `len-serv-001/002/003` → `AmdDash`; `"nonexistent"` → `None` |
   | `test_build_ipmi_command_shape` | built string contains `ipmitool -E -I lanplus -H 172.16.3.150 -U ADMIN chassis power on` and starts with `IPMI_PASSWORD=`; contains NO `-P ` token |
   | `test_redacted_command_omits_password` | for password `"test-secret"`, `redacted_ipmi_command` output contains neither `test-secret` nor `IPMI_PASSWORD`; still contains `-H 172.16.3.150` and `chassis power` |
   | `test_build_ipmi_command_never_reset` | for EVERY `PowerAction` variant, the built string contains neither `reset` nor `cycle` |
   | `test_build_ipmi_command_rejects_quote` | password `"a'b"` → `Err` (ConfigError) |
   | `test_run_power_action_unknown_host` | unknown host → `Err` naming the known hosts; mock recorded 0 commands |
   | `test_run_power_action_dash_stub` | `len-serv-001` → `Err` whose message contains `docs/agent-tasks/DEFERRED.md`; mock recorded 0 commands |
   | `test_run_power_action_missing_password` | `unimatrixone` + `None` password → `Err` mentioning `UAA_IPMI_PASSWORD`; mock recorded 0 commands |
   | `test_run_power_action_status_output` | **anti-over-suppression / happy path:** `unimatrixone` + `Status` + `Some("test-secret")` against a mock returning `"Chassis Power is on"` → `Ok("Chassis Power is on")`, and the single recorded command equals `build_ipmi_command("172.16.3.150","ADMIN","test-secret",PowerAction::Status).unwrap()` (i.e. the status action really goes through and builds the correct server-side command) |

10. Bump file headers: `src/lib.rs`, `src/cli/args.rs`, `src/main.rs`, `src/cli/commands.rs` get version bumped + `last-edited: 2026-07-09`, guids preserved; `src/power/mod.rs` gets the fresh header from Step 2.

## How to test

```bash
cargo test --lib --offline
# Expected: test result: ok. 246+ passed; 0 failed  (baseline 237 + the 9 tests above; earlier waves may have added more — never fewer than 246)
cargo build --offline
# Expected: exit 0, no errors
cargo clippy --offline
# Expected: no new warnings in src/power/, src/cli/, src/main.rs, src/lib.rs
grep -rn "chassis power reset\|chassis power cycle" src/
# Expected: 0 hits
grep -rn -- "-P '" src/power/ src/cli/
# Expected: 0 hits (no -P argv password anywhere)
grep -rn "test-secret\|IPMI_PASSWORD" src/power/mod.rs | grep -v "cfg(test)\|mod tests" | head
# Manual check: IPMI_PASSWORD appears only in the builder/comments, never in a log/println of the built string
```

## Acceptance criteria

- [ ] Tests green: `cargo test --lib --offline` prints `test result: ok.` with ≥246 passed; 0 failed.
- [ ] `cargo build --offline` exits 0 and `cargo clippy --offline` shows no new warnings in the touched files.
- [ ] Module + wiring present: `grep -n "pub mod power" src/lib.rs` → 1 hit; `grep -n "Commands::Power" src/main.rs src/cli/args.rs` → ≥1 hit per file (args.rs declares `Power {`, main.rs matches it); `grep -n "pub async fn power_command" src/cli/commands.rs` → 1 hit.
- [ ] All four mechanisms representable, one implemented: `grep -n "AmdDash\|IntelAmt\|WakeOnLan" src/power/mod.rs` → each appears ≥2 times (variant + stub match arm), and `grep -n "docs/agent-tasks/DEFERRED.md" src/power/mod.rs` → ≥1 hit (stub error text).
- [ ] reset/cycle unrepresentable: `grep -rn "chassis power reset\|chassis power cycle" src/` → 0 hits, and `PowerAction` has exactly the 3 variants (`grep -c "^\s*On,\|^\s*Off,\|^\s*Status," src/power/mod.rs` → 3).
- [ ] No local ipmitool path: `grep -rn "Command::new(\"ipmitool\")\|Command::new(\"ssh\")" src/` → 0 hits (execution goes through `CommandExecutor` only).
- [ ] No secret, no `-P`: `grep -rn "ADMIN.*ADMIN\|-P '" src/power/ src/cli/` → 0 hits; no real BMC password anywhere in the diff (`git diff origin/main --stat` reviewed).
- [ ] Redaction proven: `grep -n "test_redacted_command_omits_password" src/power/mod.rs` → 1 hit and the test passes in the suite.
- [ ] Fail-closed edge cases proven: `test_run_power_action_unknown_host`, `test_run_power_action_dash_stub`, `test_run_power_action_missing_password` all assert the mock recorded ZERO commands.
- [ ] **Anti-over-suppression:** `test_run_power_action_status_output` passes — the `status` action for `unimatrixone` still goes through the guards and the recorded command equals the builder's output containing `chassis power status` (the guard stack does not block the happy path).
- [ ] File headers bumped: `grep -n "last-edited: 2026-07-09" src/power/mod.rs src/lib.rs src/cli/args.rs src/main.rs src/cli/commands.rs` → 5 hits (1 per file).

## Commit message

```
feat(power): add uaa power <host> on|off|status with IPMI-via-server path

New src/power module: PowerMechanism dispatch (unimatrixone -> IPMI BMC
172.16.3.150; len-serv-00x -> AMD DASH stub; AMT/WoL stubs) with a hardcoded
host registry. IPMI commands are built as `IPMI_PASSWORD='<pw>' ipmitool -E
-I lanplus ...` and executed ON THE SERVER (172.16.2.30) over SSH via the
existing CommandExecutor seam — never locally (macOS ipmitool crashes
silently on Supermicro BMCs). reset/cycle are unrepresentable in PowerAction;
password comes from UAA_IPMI_PASSWORD/--ipmi-password and is never logged
(redacted command form only). Stub mechanisms fail loudly pointing at
docs/agent-tasks/DEFERRED.md. 9 unit tests against a recording mock.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

Idempotency (additive — check for the NEW thing's presence): `grep -n "pub mod power" src/lib.rs && test -f src/power/mod.rs && grep -n "Commands::Power" src/main.rs` — if all three hit, the task may already be applied; run the Acceptance criteria checks instead of re-applying. Rollback: `git revert` the single commit — it removes `src/power/mod.rs` and the four appended wiring hunks cleanly; no data, no config files, no server-side or BMC state exists to unwind (the feature is dormant unless invoked), and sibling worktrees are unaffected.
