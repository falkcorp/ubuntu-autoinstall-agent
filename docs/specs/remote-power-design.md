<!-- file: docs/specs/remote-power-design.md -->
<!-- version: 1.0.0 -->
<!-- guid: dbd8f510-2a25-4782-9d2c-64d75f53b463 -->
<!-- last-edited: 2026-07-09 -->

# Remote Power Control (`uaa power`) — Design Spec

**Status:** Approved — ready for implementation planning
**Scope:** Rust CLI only — one new module (`src/power/`) plus CLI wiring (`src/lib.rs`, `src/cli/args.rs`, `src/main.rs`, `src/cli/commands.rs`). No installer-path changes, no server-side changes, no hardware actions during implementation. Explicit follow-ups: AMD DASH implementation is deferred (see Non-goals).

---

## Motivation

Reinstall iterations on `unimatrixone` (Supermicro X10DSC+, BMC `172.16.3.150`) require repeated power off/on cycles that are today done by hand-typed `ssh 172.16.2.30 'ipmitool ...'` one-liners. Two concrete problems:

1. **No power-control code exists in the repo at all.** Grep-verified — zero hits:

   ```bash
   grep -rni "ipmi\|wake.on.lan\|\bwol\b\|etherwake\|magic packet\|chassis power" src/ --include="*.rs"
   # expect: 0 hits
   ```

2. **Running `ipmitool` locally on macOS is a silent-failure trap.** macOS `ipmitool` crashes silently against Supermicro BMCs (project-verified). The only reliable path is executing `ipmitool` **on the server** (`172.16.2.30`) over SSH. A hand-typed one-liner makes it too easy to forget this and run locally; a codified subcommand makes the safe path the only path.

The building blocks already exist and are reused, not reinvented:

- `SshClient` (`src/network/ssh.rs`, `pub struct SshClient` at line 14, `pub async fn connect(&mut self, host: &str, username: &str)` at line 49) — the same connect-to-server pattern `place_command` already uses (`src/cli/commands.rs:733-734`: `SshClient::new()` + `connect(server, server_user)`).
- The `CommandExecutor` trait (`src/network/executor.rs:11`) — the proven test seam; `MockExecutor` (`src/autoinstall/verify.rs:527`) and `RecordingMock` (`src/autoinstall/place.rs:380`) show the mock idiom the new module's tests mirror.
- CLI plumbing: `pub enum Commands` (`src/cli/args.rs:27`), single `match cli.command` dispatch with fully-qualified arms (`src/main.rs`, 13 `Commands::` arms), handler functions in `src/cli/commands.rs` (e.g. `pub async fn ssh_install_command` at line 275).

**Goal:** `uaa power <hostname> on|off|status` performs safe, class-dispatched remote power control, with the IPMI path always executed on the server, never locally.

## Goals

- One subcommand: `uaa power <hostname> <action>` where `<action>` ∈ {`on`, `off`, `status`}.
- Machine-class dispatch from a hardcoded host registry (v1 hosts: `unimatrixone` → IPMI; `len-serv-001/002/003` → AMD DASH stub).
- IPMI path executes `ipmitool` **on `172.16.2.30` over SSH** via the existing `SshClient`/`CommandExecutor` machinery.
- BMC password supplied at runtime (`UAA_IPMI_PASSWORD` env or `--ipmi-password` flag), never present in source or git.
- DASH/AMT/WoL enum arms exist and fail loudly with a `NotImplemented`-style error pointing at `docs/agent-tasks/DEFERRED.md`.
- Fully unit-testable without hardware: command-builder and dispatch logic tested against a `CommandExecutor` mock.

## Non-goals (v1)

- **AMD DASH / Intel AMT / Wake-on-LAN implementations** — deferred; no DASH credentials or drivers are installed on any lenserv yet. The enum arms return an explicit error, they do not attempt anything.
- **`reset` / `cycle` actions** — deliberately absent (see Decision 4); not "deferred", **excluded**.
- Config-file-driven host registry (YAML) — the v1 registry is hardcoded Rust; externalizing it is a later refactor if the host list grows.
- Any change to install flows, the server's `autoinstall-agent.py`, or serial/SOL console features.

## Decisions (locked)

1. **New subcommand `uaa power <hostname> on|off|status` in a NEW module `src/power/mod.rs`**, registered in `src/lib.rs`, with a `Commands::Power` variant in `src/cli/args.rs`, a match arm in `src/main.rs`, and a `power_command` handler in `src/cli/commands.rs`. Rejected alternative: bolting power logic into `src/cli/commands.rs` or `src/utils/` — a dedicated module keeps the wide-collision CLI files down to thin wiring and gives the registry/builder logic its own test module.
2. **Dispatch by machine class from a hardcoded host registry**: `unimatrixone` → `Ipmi { bmc: "172.16.3.150", user: "ADMIN" }`; `len-serv-001`, `len-serv-002`, `len-serv-003` → `AmdDash` (UNIMPLEMENTED stub). Unknown hostnames error out listing the known hosts. Rejected alternative: free-form `--bmc <ip>` flags — a registry prevents typo'd BMC IPs from power-cycling the wrong chassis.
3. **The IPMI command is EXECUTED ON THE SERVER over SSH** — reuse `SshClient`/`CommandExecutor` to `172.16.2.30` and run `ipmitool` there. HARD RULE: macOS `ipmitool` crashes silently against Supermicro BMCs — never run it locally. Rejected alternative: local `std::process::Command("ipmitool")` — crashes silently on the operator's Mac, which is exactly the failure this subcommand exists to eliminate.
4. **Explicit `power off` / `power on` only; NO `reset`/`cycle`.** `power reset`/`power cycle` are unreliable on the X10DSC+. The `PowerAction` enum has exactly three variants (`On`, `Off`, `Status`) so clap rejects anything else at parse time — the unreliable verbs are unrepresentable, not merely rejected at runtime.
5. **Password via env/flag, NEVER committed.** `--ipmi-password` flag, falling back to the `UAA_IPMI_PASSWORD` environment variable; error out with a clear message if neither is set (for `on`/`off`/`status` on an IPMI host). The known working credential (ADMIN/ADMIN) lives in project memory only and MUST NOT be hardcoded in source, tests, or docs committed to git. On the server side the password is passed to `ipmitool` via its `-E` mechanism (`IPMI_PASSWORD` env prefix), not as a `-P` argv token, so it never appears in the server's `ps` output as an ipmitool argument. Rejected alternative: a `-P '<password>'` argv token (visible in remote process listings) or a committed config default (a secret in git).
6. **DASH/AMT/WoL arms return a NotImplemented error** (`AutoInstallError::SystemError` with message text naming `docs/agent-tasks/DEFERRED.md`) rather than being omitted. This keeps the dispatch total over all registry hosts today, and makes the deferred work discoverable from the error message itself. Rejected alternative: implementing DASH now — no DASH driver or credentials are configured on any lenserv, so it would be untestable dead code.

## Data model

```rust
// src/power/mod.rs — complete v1 types (normative)

/// Power action requested on the CLI. Exactly three variants — `reset`/`cycle`
/// are intentionally unrepresentable (unreliable on the X10DSC+).
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum PowerAction {
    On,
    Off,
    Status,
}

/// How a given machine is power-controlled. One variant per machine class.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PowerMethod {
    /// Supermicro-style BMC reachable with ipmitool lanplus — executed ON THE
    /// SERVER (172.16.2.30) over SSH, never locally (macOS ipmitool crashes
    /// silently against Supermicro BMCs).
    Ipmi {
        /// BMC IP as seen from the server.
        bmc: &'static str,
        /// BMC username. The password is NEVER stored here — it arrives at
        /// runtime via --ipmi-password / UAA_IPMI_PASSWORD.
        user: &'static str,
    },
    /// AMD DASH (Lenovo M715q). UNIMPLEMENTED — returns an error pointing at
    /// docs/agent-tasks/DEFERRED.md until drivers + credentials exist.
    AmdDash,
}

/// Hardcoded v1 host registry. Returns None for unknown hostnames.
pub fn lookup_host(hostname: &str) -> Option<PowerMethod> {
    match hostname {
        "unimatrixone" => Some(PowerMethod::Ipmi { bmc: "172.16.3.150", user: "ADMIN" }),
        "len-serv-001" | "len-serv-002" | "len-serv-003" => Some(PowerMethod::AmdDash),
        _ => None,
    }
}
```

### Persistence

None. The registry is compiled in; the password is runtime-only (env/flag) and never written to disk, logs, or git.

## Components

### C1. Power module (`src/power/mod.rs`) — registry, command builder, executor

```rust
/// Build the exact command line to run ON THE SERVER for an IPMI action.
/// The password travels as an `IPMI_PASSWORD=...` env prefix consumed by
/// `ipmitool -E`, not as a -P argv token. Single quotes in the password are
/// rejected (fail-closed) rather than escaped, to rule out shell injection.
pub fn build_ipmi_command(
    bmc: &str,
    user: &str,
    password: &str,
    action: PowerAction,
) -> crate::Result<String>;

/// Dispatch a power action for `hostname` using `executor` (an already-connected
/// CommandExecutor — SshClient to 172.16.2.30 in production, a mock in tests).
/// Returns the human-readable outcome line (e.g. "Chassis Power is on").
pub async fn run_power_action(
    executor: &mut dyn crate::network::CommandExecutor,
    hostname: &str,
    action: PowerAction,
    ipmi_password: Option<&str>,
) -> crate::Result<String>;
```

Semantics (all error paths fail closed — no command is sent unless every precondition holds):

- `lookup_host` miss → `AutoInstallError::ConfigError` listing the known hostnames. No SSH traffic.
- `PowerMethod::AmdDash` → `AutoInstallError::SystemError("power: AMD DASH control for <host> is not implemented yet — see docs/agent-tasks/DEFERRED.md (driver + credential setup requires hardware access)")`. No SSH traffic.
- `PowerMethod::Ipmi` with no password available → `AutoInstallError::ConfigError` telling the operator to set `UAA_IPMI_PASSWORD` or pass `--ipmi-password`. No SSH traffic.
- Password containing `'` (single quote) → `ConfigError` (reject, don't escape).
- Built command (normative shape): `IPMI_PASSWORD='<pw>' ipmitool -E -I lanplus -H <bmc> -U <user> chassis power <on|off|status>` — the only three `chassis power` verbs ever emitted are `on`, `off`, `status`.
- Execution uses `CommandExecutor::execute_with_output` (`src/network/executor.rs:19`) so `status` output ("Chassis Power is on/off") is captured and returned; nonzero exit surfaces as the existing executor error with stderr attached.
- Logging/tracing MUST NOT include the built command string (it embeds the password). Log a redacted form: `ipmitool -E -I lanplus -H <bmc> -U <user> chassis power <action>`.

### C2. CLI wiring (`src/cli/args.rs`, `src/main.rs`, `src/cli/commands.rs`, `src/lib.rs`)

- `src/lib.rs`: add `pub mod power;` alongside the existing module registrations (`src/lib.rs:12-20`).
- `src/cli/args.rs`: new variant in `pub enum Commands` (line 27):

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

- `src/main.rs`: one fully-qualified match arm (same verbose style as `Commands::LocalInstall` at `src/main.rs:111`) destructuring the three fields and awaiting `power_command(...)`.
- `src/cli/commands.rs`: handler mirroring the `place_command` server-connection idiom (`src/cli/commands.rs:733-734`):

```rust
/// Handle `uaa power` — connects to THE SERVER (172.16.2.30) and delegates to
/// crate::power::run_power_action. ipmitool never runs on the local machine.
pub async fn power_command(
    hostname: &str,
    action: crate::power::PowerAction,
    ipmi_password: Option<&str>,
) -> Result<()>;
```

`power_command` performs registry/password validation **before** connecting (fail fast without SSH), then `SshClient::new()` + `connect("172.16.2.30", "jdfalk")`, calls `run_power_action`, prints the outcome line, and disconnects. The server IP is defined as a named `const POWER_SERVER: &str = "172.16.2.30";` in `src/power/mod.rs` (the existing `COCKROACH_SERVER_IP` at `src/autoinstall/host_spec.rs:16` happens to share the value but names a different role; do not couple to it).

## Migration / integration

Purely additive — no existing caller changes, no existing behavior changes. Before: `uaa power ...` is an unknown-subcommand clap error. After: the subcommand exists; every other subcommand's parse and dispatch is byte-for-byte unaffected (the new match arm is appended, existing arms untouched except the file-header version bump).

## Milestones

- **M1 — the whole feature (single task).** `remote-power/TASK-01`: module + registry + IPMI-via-server path + DASH stub + CLI wiring + unit tests. Additive; the one behavior change is the existence of the new subcommand, which does nothing unless invoked. There is no flag gate — safety is structural (no reset/cycle representable, no local ipmitool path exists in the code).

## Files modified

| File | Change |
|---|---|
| `src/power/mod.rs` | NEW — `PowerAction`, `PowerMethod`, `lookup_host`, `build_ipmi_command`, `run_power_action`, `POWER_SERVER`, unit tests |
| `src/lib.rs` | `pub mod power;` registration; header bump |
| `src/cli/args.rs` | `Commands::Power` variant; header bump |
| `src/main.rs` | `Commands::Power` match arm; header bump |
| `src/cli/commands.rs` | `pub async fn power_command` handler; header bump |

## Testing

All tests are `#[cfg(test)]` in `src/power/mod.rs`, using a local mock implementing `CommandExecutor` (mirror `MockExecutor`, `src/autoinstall/verify.rs:527`). No test contacts hardware, the server, or any BMC. No test contains a real password (use obviously fake values like `"test-secret"`).

| Test | Asserts |
|---|---|
| `test_lookup_host_registry` | `unimatrixone` → `Ipmi{bmc:"172.16.3.150",user:"ADMIN"}`; each `len-serv-00{1,2,3}` → `AmdDash`; `"nonexistent"` → `None` |
| `test_build_ipmi_command_shape` | built string contains `ipmitool -E -I lanplus -H 172.16.3.150 -U ADMIN chassis power on` and the `IPMI_PASSWORD=` prefix; contains no `-P ` token |
| `test_build_ipmi_command_never_reset` | for every `PowerAction` variant the built string never contains `reset` or `cycle` |
| `test_build_ipmi_command_rejects_quote` | password `"a'b"` → `Err(ConfigError)` |
| `test_run_power_action_unknown_host` | unknown host → `ConfigError` naming known hosts; mock records zero executed commands |
| `test_run_power_action_dash_stub` | `len-serv-001` → error whose message contains `docs/agent-tasks/DEFERRED.md`; mock records zero executed commands |
| `test_run_power_action_missing_password` | IPMI host + `None` password → `ConfigError` mentioning `UAA_IPMI_PASSWORD`; zero executed commands |
| `test_run_power_action_status_output` | mock returns `"Chassis Power is on"`; function returns it verbatim and the recorded command matches the builder output |

Gate: `cargo test --lib --offline` (baseline 237 passed — expect 237 + new power tests, 0 failed), `cargo build --offline`, `cargo clippy --offline`.

## Failure modes

| Failure | Behavior |
|---|---|
| Unknown hostname | Fail closed before SSH: `ConfigError` listing registry hosts |
| DASH-class host (`len-serv-00x`) | Fail closed before SSH: NotImplemented-style error pointing at `docs/agent-tasks/DEFERRED.md` |
| No password (flag unset, `UAA_IPMI_PASSWORD` unset) | Fail closed before SSH: `ConfigError` naming both supply mechanisms |
| Password contains `'` | Fail closed: `ConfigError` (no escaping heroics, no injection surface) |
| Server `172.16.2.30` unreachable / SSH auth fails | Existing `SshClient::connect` error propagates; nothing was attempted against the BMC |
| `ipmitool` missing on the server / wrong password / BMC down | Nonzero remote exit → executor error with stderr; command was `on`/`off`/`status` only, so no partial-reset states are possible |
| Operator asks for `reset`/`cycle` | clap parse error — the verbs do not exist in `PowerAction` |

## Rollback

Single additive commit on branch `agent/remote-power-power-subcommand-ipmi`; `git revert <sha>` removes the subcommand entirely and restores lib.rs/args.rs/main.rs/commands.rs wiring. No data, no config files, no server-side state to unwind. The feature is dormant unless explicitly invoked, so leaving it in place carries no background risk.

## Open questions (resolved — recorded for the plan)

1. ~~Local ipmitool with a `--via-server` escape hatch?~~ → No. The local path is not implemented at all (Decision 3); macOS ipmitool crashes silently against Supermicro BMCs.
2. ~~Support `reset`/`cycle` behind a `--force` flag?~~ → No. Unrepresentable in `PowerAction` (Decision 4); unreliable on the X10DSC+.
3. ~~Implement DASH for lenservs now?~~ → No. Stub + `docs/agent-tasks/DEFERRED.md` pointer (Decision 6); no driver/credentials exist on any lenserv.
4. ~~Reuse `COCKROACH_SERVER_IP`?~~ → No. Same value, different role; `src/power/mod.rs` defines its own `POWER_SERVER` const to avoid semantic coupling.
