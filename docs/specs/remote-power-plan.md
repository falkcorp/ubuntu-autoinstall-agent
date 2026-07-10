<!-- file: docs/specs/remote-power-plan.md -->
<!-- version: 1.0.0 -->
<!-- guid: bc873684-ca5a-405e-8f3d-0a8ae5a14f42 -->
<!-- last-edited: 2026-07-09 -->

# Remote Power Control (`uaa power`) — Implementation Plan

Companion to [remote-power-design.md](remote-power-design.md). All design decisions there are **LOCKED** — this plan sequences the work; it does not reopen choices (no local ipmitool, no reset/cycle, no DASH implementation, no committed secrets).

**Workstream:** `remote-power` · **Tasks:** 1 · **Global wave:** 5

---

## Wave order

The workstream has a single task, `remote-power/TASK-01`, scheduled in **global wave 5**. It is dependency-free (`Depends on: none`) but is wave-gated by file collisions, not by logic:

| Shared file | Colliding tasks that must be MERGED first |
|---|---|
| `src/cli/args.rs` | `phase-rerun/TASK-01` (wave 4) |
| `src/main.rs` | `phase-rerun/TASK-01` (wave 4) |
| `src/cli/commands.rs` | `installer-robustness/TASK-02` (wave 1), `installer-robustness/TASK-03` (wave 2), `installer-robustness/TASK-07` (wave 3), `phase-rerun/TASK-01` (wave 4) |

`src/power/mod.rs` and `src/lib.rs` are uncontested. Wave-5 peer `phase-rerun/TASK-02` shares **no** files with this task, so both wave-5 tasks may run concurrently. Do not dispatch TASK-01 until every wave-≤4 task above is merged to `main`; the worker's first action (`git rebase origin/main` in the START HERE block) then picks up the final shape of the three CLI wiring files.

## Steps (1:1 with task briefs)

### Step 1 — `remote-power/TASK-01`: power subcommand + IPMI-via-server path

**Brief:** [docs/agent-tasks/remote-power/TASK-01-power-subcommand-ipmi.md](../agent-tasks/remote-power/TASK-01-power-subcommand-ipmi.md)
**Priority/Effort/Tier:** P2 · M · Sonnet-class · additive · wave 5

Scope (from the design spec, sections C1/C2):

1. NEW `src/power/mod.rs`: `PowerAction` (`On|Off|Status` only — reset/cycle unrepresentable), `PowerMethod` (`Ipmi{bmc,user}` / `AmdDash`), `lookup_host` hardcoded registry (`unimatrixone` → `Ipmi{bmc:"172.16.3.150",user:"ADMIN"}`; `len-serv-001/002/003` → `AmdDash` stub), `POWER_SERVER = "172.16.2.30"`, `build_ipmi_command` (password via `IPMI_PASSWORD=` env prefix + `ipmitool -E`, single-quote passwords rejected), `run_power_action` over `&mut dyn CommandExecutor`, plus the eight unit tests from the design's Testing table (mock executor mirroring `MockExecutor`, `src/autoinstall/verify.rs:527`).
2. `src/lib.rs`: `pub mod power;`.
3. `src/cli/args.rs`: `Commands::Power { hostname, action, ipmi_password }` with `#[arg(long, env = "UAA_IPMI_PASSWORD", hide_env_values = true)]`.
4. `src/main.rs`: fully-qualified `Commands::Power` match arm (style of the existing `Commands::LocalInstall` arm).
5. `src/cli/commands.rs`: `pub async fn power_command` — validate registry + password **before** SSH, then `SshClient::new()` / `connect(POWER_SERVER, ...)` (idiom of `place_command`, `src/cli/commands.rs:733-734`), delegate to `run_power_action`, print outcome, disconnect.
6. File headers bumped (version + last-edited) on all five files; new file gets a fresh 4-line header.

Hard rules restated for the executor (also in the brief):

- ipmitool runs FROM THE SERVER over SSH (`ssh 172.16.2.30 'ipmitool -I lanplus -H <bmc> ...'`), never on macOS — the local path must not exist in code.
- Explicit `power off` / `power on` / `status` only; no reset/cycle anywhere, including tests and docs.
- No real BMC password in source, tests, fixtures, or committed docs (ADMIN/ADMIN stays in project memory only); the built command string is never logged unredacted.
- Code/docs only — no live command against any BMC or server during implementation; validation is unit tests + build.

**Gates for this step:**

- Run: `cargo test --lib --offline`
  Expected: `test result: ok.` with **245+ passed; 0 failed** (baseline 237 + ≥8 new power tests)
- Run: `cargo build --offline`
  Expected: compiles with no errors
- Run: `cargo clippy --offline`
  Expected: no new warnings in `src/power/`, `src/cli/`, `src/main.rs`, `src/lib.rs`
- Run: `grep -rni "reset\|cycle" src/power/mod.rs`
  Expected: 0 hits in emitted-command code paths (comments explaining the exclusion are acceptable; no `chassis power reset`/`cycle` string anywhere)
- Run: `grep -rn "ADMIN.*ADMIN\|-P '" src/power/ src/cli/`
  Expected: 0 hits (no hardcoded credential, no `-P` argv password)
- Run: `grep -rni "ipmi" src/ --include="*.rs" | grep -v "src/power/\|src/cli/args.rs\|src/cli/commands.rs\|src/main.rs"`
  Expected: 0 hits (IPMI logic contained to the new module + wiring)

## Verification anchors (worker re-verifies before editing)

From the evidence scout — the brief carries these verbatim; listed here so the plan is self-checking:

```bash
cd /Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent/.worktrees/remote-power-power-subcommand-ipmi
grep -n "pub enum Commands" src/cli/args.rs
# expect: 1 hit at line 27
grep -n "Commands::" src/main.rs
# expect: 13 hits, match arms from line 36 (CreateImage) through line 162 (RenderUserData); Commands::LocalInstall at line 111
grep -n "pub async fn ssh_install_command" src/cli/commands.rs
# expect: 1 hit at line 275
grep -n "pub trait CommandExecutor" src/network/executor.rs
# expect: 1 hit at line 11
grep -rni "ipmi\|wake.on.lan\|\bwol\b\|etherwake\|magic packet\|chassis power" src/ --include="*.rs"
# expect: 0 hits
grep -n "COCKROACH_SERVER_IP" src/autoinstall/host_spec.rs
# expect: 1+ hits, const definition at line 16
```

Line numbers may have drifted by the time wave 5 runs (wave-≤4 tasks edit `args.rs`/`main.rs`/`commands.rs`); the grep hits themselves, not the line numbers, are authoritative.

## Rollout / rollback

- Worktree `$REPO/.worktrees/remote-power-power-subcommand-ipmi`, branch `agent/remote-power-power-subcommand-ipmi` off `origin/main`; the worker never pushes/PRs — coordinator owns git.
- Single additive commit; `git revert` removes the subcommand cleanly (design spec, Rollback section).
- Post-merge manual validation (operator, optional, NOT part of the task): `UAA_IPMI_PASSWORD=... uaa power unimatrixone status` — read-only, safe; `on`/`off` only during a planned unimatrixone maintenance window.

## Done criteria for the workstream

- [ ] `remote-power/TASK-01` merged to `main` with all gates green.
- [ ] `uaa power --help` shows exactly `on|off|status` actions and the `--ipmi-password` / `UAA_IPMI_PASSWORD` password path.
- [ ] `uaa power len-serv-001 status` (no password needed to hit the stub) exits nonzero with an error naming `docs/agent-tasks/DEFERRED.md`.
- [ ] No `ipmitool` invocation path exists that runs on the local machine.
