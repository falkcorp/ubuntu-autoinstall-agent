<!-- file: docs/agent-tasks/tooling-port/TASK-04-vm-validate-port.md -->
<!-- version: 1.0.1 -->
<!-- guid: dfee93a5-3bf5-4bb6-b4fd-2e32cbee09bb -->
<!-- last-edited: 2026-07-10 -->

# TASK-04 — `uaa vm-validate`: port the 8-stage QEMU+swtpm harness (workspace/qcow2/swtpm/boot/install/assert/report) reusing utils/qemu.rs+vm.rs (ws9-tooling)

**Priority:** P2 · **Effort:** L · **Recommended subagent:** Sonnet-class · rust-port subagent · **Why:** orchestration port of the safety gate itself; report format must stay machine-greppable · **Depends on:** CP-01 (wave-2 gated: `core-proto/TASK-01` workspace conversion MERGED and this worktree rebased — the stub file this task fills does not exist before then)

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/tooling-port-vm-validate-port" -b agent/tooling-port-vm-validate-port origin/main
cd "$REPO/.worktrees/tooling-port-vm-validate-port"
git rebase origin/main
```

(Protocol also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

## Goal

Fill the CP-01-created stub `crates/uaa-core/src/vm_validate.rs` with a Rust port of `scripts/vm-validate.sh` (v1.0.0, 535 lines) — the 8-stage (stage 0–7) QEMU+swtpm VM validation gate — and replace the `todo!()` of the pre-wired `uaa vm-validate` CLI dispatch (CP-01, `crates/uaa/src/`). The `==== VERIFY-ON-VM REPORT ====` output MUST stay byte-compatible with the script's format — downstream tooling and TG-03's constellation e2e greps it (spec Testing table: "VM e2e (extended vm-validate)"; spec M5: "THE gate before any hardware"). **CRITICAL scope rule (skeleton shared_state): `scripts/vm-validate.sh` stays AUTHORITATIVE until TG-03 proves the port — do not modify, rename, or delete the script, and do not change `docs/vm-validation.md` to point at the port.** Purely additive.

REUSE — do not invent parallels:

- **qemu/vm helpers** (`src/utils/qemu.rs` — verify: `grep -n "pub async fn" src/utils/qemu.rs` (NOTE: the fns are `pub async fn`, a plain `"pub fn"` grep returns 0); `src/utils/vm.rs` — verify: `grep -n "pub struct VmManager" src/utils/vm.rs`). Reuse `QemuUtils` (`get_image_info`, image handling) and mirror `VmManager`'s process-lifecycle idioms (pid tracking, `kill_qemu`) rather than reinventing them. Where an existing helper hardcodes behavior the harness needs differently, add a NEW fn in `vm_validate.rs` — never modify `qemu.rs`/`vm.rs` behavior used by the 311 baseline tests.
- **`CommandExecutor`** trait (pre-move: `src/network/executor.rs` — verify: `grep -n "pub trait CommandExecutor" src/network/executor.rs`). EVERY external process (`qemu-system-x86_64`, `swtpm`, `qemu-img`, `ssh`, `scp`, `command -v` probes, `uname`) goes through it so tests mock the whole harness. NO direct `std::process::Command` in `vm_validate.rs`.
- **Mock idiom:** `MockExecutor` (`src/autoinstall/verify.rs` — verify: `grep -n "struct MockExecutor" src/autoinstall/verify.rs`) + recorded commands. No mocking crate.
- **`AutoInstallError::*`** (`src/error.rs`). No new error enum.

## Background (verify before editing)

Ground truth is `scripts/vm-validate.sh`. The 8 stages, each with its own log file under `<workdir>/logs/NN-<name>.log`:

- **Stage 0 preflight:** Linux host required (macOS has no KVM — the script says run on the server or any amd64 Linux box; the PORT only ever runs commands through the executor, so in production it refuses on non-Linux `uname -s`); required tools `qemu-system-x86_64 swtpm qemu-img ssh scp`; `sshpass` and `socat` are WARN-only fallbacks; OVMF firmware discovery across 4 dirs × (`OVMF_CODE_4M.fd`|`OVMF_CODE.fd`, same for VARS); `/dev/kvm` not writable = WARN + TCG fallback, not failure; **`--config` containing `REPLACE_AT_PLACE_TIME` = hard die** ("never install with unsubstituted secrets").
- **Stage 1 workspace:** `qemu-img create -f qcow2 <workdir>/disk.qcow2 <size>`; copy OVMF_VARS; start `swtpm socket --tpmstate dir=... --ctrl type=unixio,path=... --tpm2 --daemon --pid file=...`; wait ≤20s for pidfile THEN ≤20s for socket (pid read first so cleanup can always kill OUR swtpm — never `pkill` by name, the host may run other VMs).
- **Stage 2 boot-iso:** qemu with OVMF pflash args, virtio qcow2 disk (guest sees `/dev/vda` — partition-suffix proof), `-cdrom <iso> -boot order=dc`, swtpm chardev + `tpm-tis`, user-net `hostfwd=tcp::<ssh_port>-:22`, serial to a log file, `-display none -no-reboot`, `+ -enable-kvm -cpu host` iff KVM; wait for SSH within `--boot-timeout` (default 600).
- **Stage 3 interrogate (report-only, never fails the gate):** over SSH, `systemctl list-units/list-unit-files '*subiquity*'` → `observed_units`; verdict `COVERED` iff every observed unit is in the mask list `subiquity-server.service serial-subiquity@.service snap.subiquity.subiquity-server.service`, else `GAP (unit <u> not in mask list)`; `command -v <tool>` for `debootstrap sgdisk zpool cryptsetup dracut clevis` → present/MISSING per tool.
- **Stage 4 install:** scp agent → `/tmp/uaa` + config → `/tmp/vm-test.yaml`; `sudo /tmp/uaa install --config /tmp/vm-test.yaml` with `--install-timeout` (default 3600; timeout is a FAIL, never a skip); assert the log shows ≥7 `Phase completed:` lines AND `Phase 6: Final setup` AND `Installation completed successfully` (deliberately NO `--hold-on-failure`/`--pause-after-storage` — those route through the silent-on-success macro and break the greps; keep this comment in the port).
- **Stage 5 boot-disk:** reboot from the installed qcow2 with the SAME swtpm state (no `-cdrom`); optional socat best-effort LUKS-passphrase auto-answer on the serial console; wait for root SSH.
- **Stage 6 assert (each failure = fail_stage 6):** `cryptsetup status luks` must contain `is active`; `zpool list -H -o name` must list BOTH `rpool` and `bpool` (exact-line match); multi-user via `systemctl is-system-running --wait` matching `running|degraded`, fallback `systemctl is-active multi-user.target` == `active`.
- **Stage 7 report:** print the report with `GATE: PASS`; any earlier `fail_stage` prints it with `GATE: FAIL (stage N: <msg>)` and exits 1. Report format (byte-compatible, from `print_report`): header `==== VERIFY-ON-VM REPORT ====`, marker-72 block (`observed-units:`, `masked-by-build-script:`, `verdict:`), marker-81 block (one `  %-12s %s` line per tool, `UNKNOWN` when stage 3 not reached), `GATE: PASS` or `GATE: FAIL (...)`, footer `=============================`.
- Cleanup: kill only pids THIS harness started (qemu, swtpm), on every exit path.

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

For THIS task additionally: unit tests must never launch a real qemu/swtpm — the entire harness is exercised against the recording mock; the only disk the production code may ever address is the qcow2 it creates itself under `--workdir`.

**Path map:** after CP-01 (wave 1) merges, `src/**` lives at
`crates/uaa-core/src/**` and the CLI at `crates/uaa/src/**`. The greps above cite
pre-move paths (verifiable on today's main); at execution time run them at the old
path, then the mapped path. Zero hits at BOTH = STOP and report.

- **Re-verify these anchors before editing** — line numbers drift; zero hits at both
  old and mapped path = STOP and report:
  ```bash
  grep -n "VERIFY-ON-VM REPORT" scripts/vm-validate.sh
  # expect: 1+ hits (print_report, ~line 118)
  grep -n "cryptsetup status\|zpool list\|multi-user" scripts/vm-validate.sh | head -5
  # expect: hits (stage 6 assertions, ~lines 491-528)
  grep -n "stage_echo" scripts/vm-validate.sh | head -10
  # expect: 9+ hits (stages 0-7 markers)
  grep -n "pub async fn" src/utils/qemu.rs | head -5
  # expect: hits (QemuUtils API — then at crates/uaa-core/src/utils/qemu.rs)
  grep -n "pub struct VmManager" src/utils/vm.rs
  # expect: 1 hit (then at crates/uaa-core/src/utils/vm.rs)
  grep -n "pub trait CommandExecutor" src/network/executor.rs
  # expect: 1 hit (then at crates/uaa-core/src/network/executor.rs)
  ```
- Execution-time checks (crates/ exists only after CP-01):
  ```bash
  test -f crates/uaa-core/src/vm_validate.rs && grep -n "todo!" crates/uaa-core/src/vm_validate.rs
  # expect: headered stub exists; absent = STOP, CP-01 not merged
  grep -rn "vm.validate\|vm_validate" crates/uaa/src/ | head -5
  # expect: the pre-wired `uaa vm-validate` variant + todo!() arm to replace
  ```

## Step-by-step

1. Run the ⛔ START HERE block, then every anchor grep above. Any zero-hit result at both paths → STOP and report.
2. In `crates/uaa-core/src/vm_validate.rs` define `pub struct VmValidateOptions { pub iso: PathBuf, pub agent: PathBuf, pub config: PathBuf, pub workdir: PathBuf, pub disk_size: String /* "40G" */, pub ssh_port: u16 /* 10022 */, pub boot_timeout: u64 /* 600 */, pub install_timeout: u64 /* 3600 */ }` (defaults = the script's), and the report model:
   ```rust
   pub struct VerifyOnVmReport { pub observed_units: Option<String>, pub marker72_verdict: Option<String>,
                                 pub tool_status: BTreeMap<String, ToolStatus /* Present|Missing|Unknown */>,
                                 pub gate: GateResult /* Pass | Fail { first_failing_stage: String } */ }
   pub fn render_report(r: &VerifyOnVmReport) -> String;   // byte-compatible with print_report
   ```
   `render_report` is a PURE function; write it against the script's exact output (header, `marker build-installer-image.sh:72 ...` / `:81 ...` lines, `masked-by-build-script:` list, `%-12s` tool columns, `GATE:` line, `=====` footer, `UNKNOWN (stage 3 not reached)` fallbacks).
3. Add the pure evaluators (all unit-testable without any process):
   - `pub fn evaluate_marker72(observed_units: &str) -> String` — `COVERED` / `GAP (unit <u> not in mask list)` against the 3-unit mask list (same literals as `image_build.rs::MASK_UNITS` once TP-03 lands — duplicate the literals here, do not create a cross-file dependency that would collide with TP-03's wave);
   - `pub fn evaluate_install_log(log: &str) -> Result<u32>` — the three stage-4 assertions (≥7 `Phase completed:`, `Phase 6: Final setup`, case-insensitive `Installation completed successfully`), returning the phase count;
   - `pub fn evaluate_stage6(crypt_out: &str, zpool_out: &str, is_system_running: &str, multi_user_fallback: Option<&str>) -> Result<()>` — `is active` substring; exact-line `rpool` AND `bpool`; `running|degraded` or fallback `active`;
   - `pub fn config_has_placeholder(text: &str) -> bool` — the stage-0 `REPLACE_AT_PLACE_TIME` die.
4. Add the stage orchestrator `pub async fn vm_validate(executor: &mut dyn CommandExecutor, opts: &VmValidateOptions) -> Result<VerifyOnVmReport>` running stages 0–7 in order with the semantics in Background: WARN-not-fail for sshpass/socat/KVM; report-only stage 3; fail-closed stage ordering (a failing stage renders the report with `GATE: FAIL (stage N: <msg>)` before returning `Err`); per-stage log files under `<workdir>/logs/`; cleanup kills only recorded pids (track qemu/swtpm pids returned by the executor commands; NEVER emit a `pkill` command).
5. Replace the `todo!()` in the pre-wired `uaa vm-validate` CLI module in `crates/uaa/src/`: flags `--iso`, `--agent`, `--config`, `--workdir`, `--disk-size`, `--ssh-port`, `--boot-timeout`, `--install-timeout` (defaults above), local executor, print the rendered report to stdout, exit 1 on `GATE: FAIL`.
6. Unit tests (recording MockExecutor; golden-style string assertions for the report):

   | Test | Asserts |
   |---|---|
   | `test_render_report_pass_format` | full-pass report renders EXACTLY: first line `==== VERIFY-ON-VM REPORT ====`, contains `GATE: PASS`, last line `=============================`, one line per tool in mask/tool order |
   | `test_render_report_fail_and_unknowns` | stage-2 failure before interrogate → `verdict: UNKNOWN (stage 3 not reached)`, every tool `UNKNOWN`, `GATE: FAIL (stage 2: ...)` |
   | `test_marker72_covered_and_gap` | observed `subiquity-server.service` → `COVERED`; observed `weird.service` → `GAP (unit weird.service not in mask list)` |
   | `test_install_log_assertions` | 7 `Phase completed:` + `Phase 6: Final setup` + success line → `Ok(7)`; 6 phase lines → `Err`; missing success line → `Err` |
   | `test_stage6_assertions` | `is active`/`rpool\nbpool`/`running` → `Ok`; `rpool` only → `Err`; `degraded` accepted; fallback `active` accepted when is-system-running says `starting` |
   | `test_placeholder_config_dies_stage0` | config text with `REPLACE_AT_PLACE_TIME` → `vm_validate` returns `Err` and the mock recorded NO qemu/swtpm/qemu-img command |
   | `test_no_pkill_ever` | happy-path run: no recorded command contains `pkill`; kills reference recorded pids only |
   | `test_full_pass_command_sequence` | **anti-over-suppression:** mocked all-stages-pass run returns `GateResult::Pass`; recorded commands include (in order) `qemu-img create -f qcow2`, `swtpm socket`, a qemu invocation containing `tpm-tis` and `hostfwd=tcp::10022-:22`, the scp/install commands, and the stage-6 triple — the guard stack does not block a clean pass |

7. Bump the file header (version + last-edited) on every file you touch; new files get a fresh 4-line header with a new uuid4 (`uuidgen | tr 'A-F' 'a-f'`).

## How to test

```bash
cargo test --lib --offline && cargo build --offline
# Expected: all tests pass (baseline 311 + your new tests), build clean
cargo clippy --offline -- -D warnings
# Expected: no warnings
cargo test --lib --offline vm_validate
# Expected: the 8 tests above all pass
grep -rn "std::process::Command" crates/uaa-core/src/vm_validate.rs
# Expected: 0 hits (executor-only)
grep -rn "pkill" crates/uaa-core/src/vm_validate.rs
# Expected: 0 hits outside the comment explaining why it is banned
git diff origin/main -- scripts/vm-validate.sh docs/vm-validation.md
# Expected: empty (the script stays AUTHORITATIVE until TG-03 proves the port)
```

## Acceptance criteria

- [ ] Port complete: `grep -n "pub async fn vm_validate" crates/uaa-core/src/vm_validate.rs` → 1 hit; `grep -n "todo!" crates/uaa-core/src/vm_validate.rs` → 0 hits; the `vm-validate` CLI arm no longer contains `todo!`.
- [ ] Report byte-compatible: `grep -n "==== VERIFY-ON-VM REPORT ====" crates/uaa-core/src/vm_validate.rs` → ≥1 hit; `test_render_report_pass_format` and `test_render_report_fail_and_unknowns` pass.
- [ ] All 8 stages represented (each stage's `logs/NN-<name>.log` literal appears in the port; wave-gate grep — runs against your finished worktree file, pattern verified to yield exactly 8 against `scripts/vm-validate.sh`):
  ```bash
  grep -oE '00-preflight|01-workspace|02-boot-iso|03-interrogate|04-install|05-boot-disk|06-assert|07-report' crates/uaa-core/src/vm_validate.rs | sort -u | wc -l
  # Expected: 8
  ```
- [ ] Stage-6 semantics exact: `test_stage6_assertions` passes (rpool AND bpool exact-line; `degraded` accepted; multi-user fallback).
- [ ] Placeholder die + no-pkill proven: `test_placeholder_config_dies_stage0` (zero VM commands) and `test_no_pkill_ever` pass.
- [ ] **Anti-over-suppression:** `test_full_pass_command_sequence` passes — a clean run traverses every stage and reports `GATE: PASS`.
- [ ] Script untouched: `git diff origin/main -- scripts/vm-validate.sh docs/vm-validation.md` empty.
- [ ] `cargo test --lib --offline && cargo build --offline` exits 0; clippy clean.
- [ ] File headers bumped on every changed file (`git diff origin/main --stat` files each show a version bump; guids unchanged).

## Commit message

```
feat(vm): port vm-validate.sh 8-stage QEMU+swtpm harness to uaa vm-validate (ws9-tooling)

Fills crates/uaa-core/src/vm_validate.rs: stage 0-7 orchestration (preflight
incl. REPLACE_AT_PLACE_TIME die, qcow2+OVMF+swtpm workspace, ISO boot with
tpm-tis + hostfwd SSH, report-only interrogate of both VERIFY-ON-VM markers,
7-phase install assertions, same-TPM-state disk boot, LUKS/rpool+bpool/
multi-user asserts) with the ==== VERIFY-ON-VM REPORT ==== output kept
byte-compatible via a pure render_report. All processes via CommandExecutor
(mock-tested, no real qemu in tests); kills only own pids, never pkill.
scripts/vm-validate.sh stays authoritative until TG-03 proves the port.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

If `grep -n "pub async fn vm_validate" crates/uaa-core/src/vm_validate.rs` hits (and `grep -n "todo!"` in the same file shows 0), already applied — run the acceptance checks instead of re-applying. Rollback = revert the single commit; `scripts/vm-validate.sh` (still the authoritative gate), `docs/vm-validation.md`, `src/utils/qemu.rs`/`vm.rs` behavior, and all 311 baseline tests stay untouched (the stub returns to its CP-01 `todo!()` form).
