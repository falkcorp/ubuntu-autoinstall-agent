<!-- file: docs/agent-tasks/uaa-pxe/TASK-02-pxe-health.md -->
<!-- version: 1.0.0 -->
<!-- guid: 20324b68-c2f4-48ad-82a6-8b6baeef1991 -->
<!-- last-edited: 2026-07-10 -->

# TASK-02 — Health RPC: dnsmasq/tftpd unit state + TFTP self-probe + boot-target consistency verification (ws6-pxe)

**Priority:** P2 · **Effort:** S · **Recommended subagent:** Sonnet-class · rust-services subagent · **Why:** probe logic against mocked executor output. · **Depends on:** TASK-01 (wave-7 gated: PX-01 merged to `origin/main` — this task fills the `health.rs` stub PX-01 created; PX-02/03/04 are parallel-safe, disjoint stub files)

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/uaa-pxe-pxe-health" -b agent/uaa-pxe-pxe-health origin/main
cd "$REPO/.worktrees/uaa-pxe-pxe-health"
git rebase origin/main
```

(Protocol also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

## Goal

Fill `crates/uaa-pxe/src/health.rs` (the PX-01 stub — this task is its EXCLUSIVE filler) with the `PxeService.Health` RPC (spec §C5, Decision 13): (1) dnsmasq AND tftpd-hpa systemd unit state, (2) a TFTP self-probe fetching a known file from localhost, (3) boot-target consistency verification — the projected hostsdir/optsdir files on disk match what the request says the registry intends. Everything runs through the existing `CommandExecutor` seam; ONLY `health.rs` (plus wiring the RPC arm in `main.rs`) changes.

REUSE — do not invent parallels for any of these:

- **`CommandExecutor`** trait (`crates/uaa-core/src/network/executor.rs`; pre-move `src/network/executor.rs` — verify: `grep -n "pub trait CommandExecutor" crates/uaa-core/src/network/executor.rs`). `check_silent` for unit-state, `execute_with_output` for the probe. NO `std::process::Command`.
- **`render_host_lines` / `mac_file_stem` / `BootTarget`** from `crates/uaa-pxe/src/boot_config.rs` (PX-01) for the consistency check — re-render the intent and compare to disk. Do NOT re-implement the rendering.
- **Mock idiom:** the recording `MockExecutor` pattern already inside `crates/uaa-pxe/src/boot_config.rs` tests (PX-01) — copy it into `health.rs` tests. No mocking crate.

**Path map:** after CP-01 (wave 1) merges, `src/**` lives at `crates/uaa-core/src/**` and the CLI at `crates/uaa/src/**`. The greps below cite pre-move paths (verifiable on today's main); at execution time run them at the old path, then the mapped path. Zero hits at BOTH = STOP and report.

## Background (verify before editing)

- **tftpd-hpa serves TFTP, NOT dnsmasq** — ground truth `unimatrixone-pxe-boot-status.md` (~line 585: root `/srv/tftp`, listening `0.0.0.0:69`; dnsmasq is proxy-DHCP only). Health must therefore check BOTH units — a green dnsmasq with a dead tftpd-hpa still breaks arch-7 PXE boots.
- The same doc (~line 456) records that tftpd-hpa produces ZERO log entries even when working — so log-based health is worthless; that is WHY the self-probe exists: actually fetch a file over TFTP from 127.0.0.1.
- Probe command (through the executor): `curl -s --connect-timeout 5 tftp://127.0.0.1/<probe_file> -o /dev/null && echo PROBE_OK` — `probe_file` comes from `PxeConfig` (new field `tftp_probe_file`, default `ipxe.efi`); success = output contains `PROBE_OK`.
- Edge semantics (here AND in acceptance): a unit reported inactive → that component `healthy: false` with reason string, RPC still returns Ok (Health REPORTS, never errors on unhealthy — only transport/internal faults become `tonic::Status`). Consistency check with NO expected targets in the request → `consistent: true` (vacuous), NOT an error. A hostsdir file present on disk for a MAC the request says should be `local-disk` → inconsistent (stale projection is exactly the Decision-13 failure this catches). Missing file for `local-disk` → consistent.

**HARD RULES (non-negotiable):**
- NO hardware actions. Validate ONLY in-repo (`cargo`) and, where a brief says so, the QEMU+swtpm harness (`scripts/vm-validate.sh`). Code that COULD touch hardware is written and unit-tested against mock executors only.
- NEVER wipe, write to, or deploy on 172.16.2.30 ("the server") or len-serv-003.
- `disk_device` is read from the live target at runtime, never guessed or hardcoded.
- ipmitool runs via `ssh 172.16.2.30`, never on macOS.
- NEVER power on unimatrixone (U1).
- No real secret in any file: `REPLACE_AT_PLACE_TIME` placeholders stay placeholders.
- Stay inside your worktree; never `git push`, `gh pr`, or merge — report done and stop.

- **Re-verify these anchors before editing** — line numbers drift; zero hits at both old and mapped path = STOP and report:
  ```bash
  grep -n "tftpd-hpa" unimatrixone-pxe-boot-status.md | head -3
  # expect: hits (~lines 456, 585 — tftpd-hpa serves TFTP, not dnsmasq)
  grep -n "Filled by uaa-pxe/TASK-02\|TASK-02" crates/uaa-pxe/src/health.rs
  # expect: 1 hit (the PX-01 stub; absent file = PX-01 not merged, STOP)
  grep -n "pub fn render_host_lines" crates/uaa-pxe/src/boot_config.rs
  # expect: 1 hit (PX-01 artifact you reuse)
  grep -n "rpc Health" proto/uaa/pxe/v1/pxe.proto
  # expect: 1 hit
  grep -n "pub trait CommandExecutor" src/network/executor.rs
  # expect: 1 hit (mapped: crates/uaa-core/src/network/executor.rs)
  grep -n "async fn check_silent" src/network/executor.rs
  # expect: 1 hit (mapped path as above)
  ```

## Step-by-step

1. Run the ⛔ START HERE block, then the anchor greps above. Any STOP condition → report and stop.
2. In `crates/uaa-pxe/src/health.rs`, define `pub struct HealthReport { pub dnsmasq_active: bool, pub tftpd_active: bool, pub tftp_probe_ok: bool, pub consistent: bool, pub inconsistencies: Vec<String>, pub reasons: Vec<String> }` mapping 1:1 onto the proto `HealthResponse` fields.
3. `pub async fn unit_active(executor: &mut dyn CommandExecutor, unit: &str) -> bool` → `check_silent("systemctl is-active --quiet <unit>")`; call for `dnsmasq` and `tftpd-hpa`. An executor error counts as inactive with a reason string (fail-safe reporting, never a panic).
4. `pub async fn tftp_probe(executor: &mut dyn CommandExecutor, probe_file: &str) -> bool` → run the curl probe command from Background; success iff output contains `PROBE_OK`.
5. `pub fn check_consistency(cfg: &PxeConfig, expected: &[(String, BootTarget, ExpectedRenderInputs)]) -> (bool, Vec<String>)` — pure, no executor: for each (mac, target) in the request, re-render via `render_host_lines` and compare with the on-disk `<hostsdir>/<stem>.conf` and `<optsdir>/<stem>.conf`: `local-disk` expects BOTH absent; other targets expect byte-identical content. Every mismatch appends a human-readable line (`"aa:bb:...: hostsdir stale (expected absent, found file)"`). Empty `expected` → `(true, vec![])`.
6. Implement the `Health` RPC arm in `main.rs` (replace the `unimplemented` stub arm PX-01 left): compose steps 3–5 into `HealthReport`, always `Ok(HealthResponse)` — unhealthy is DATA, not an error.
7. Unit tests (`#[cfg(test)]` in `health.rs`, tempdir + recording MockExecutor): `test_unit_active_both_units` (mock says active → both true; command strings name `dnsmasq` and `tftpd-hpa`); `test_unit_inactive_reported_not_error` (mock inactive → `Ok` report with `dnsmasq_active: false` + reason); `test_tftp_probe_ok_and_fail` (output `PROBE_OK` → true; error/other output → false); `test_consistency_detects_stale_local_disk_file` (file on disk, expected `local-disk` → inconsistent with message); `test_consistency_matches_rendered` (**anti-over-suppression:** a correctly-projected `custom-autoinstall` host with byte-identical files reports `consistent: true` with zero inconsistencies — the checker does not flag healthy state); `test_consistency_empty_expected_vacuous_true`.
8. Bump the file header (version + last-edited) on every file you touch; new files get a fresh 4-line header with a new uuid4 (`uuidgen | tr 'A-F' 'a-f'`).

## How to test

```bash
cargo test --lib --offline && cargo build --offline
# Expected: all tests pass (baseline 311 + earlier waves + your 6 new tests; 0 failed), build clean
cargo clippy --offline -- -D warnings
# Expected: no warnings
grep -rn "std::process::Command" crates/uaa-pxe/src/health.rs
# Expected: 0 hits
grep -n "tftpd-hpa" crates/uaa-pxe/src/health.rs
# Expected: hits (both units checked)
```

## Acceptance criteria

- [ ] Both units checked: `grep -n "tftpd-hpa" crates/uaa-pxe/src/health.rs` → ≥1 hit and `grep -n "is-active" crates/uaa-pxe/src/health.rs` → hits covering dnsmasq and tftpd-hpa.
- [ ] Probe exists and is executor-mediated: `grep -n "tftp://127.0.0.1" crates/uaa-pxe/src/health.rs` → 1+ hit; `grep -rn "std::process::Command" crates/uaa-pxe/src/health.rs` → 0 hits.
- [ ] Consistency reuses PX-01 rendering: `grep -n "render_host_lines" crates/uaa-pxe/src/health.rs` → ≥1 hit (no duplicated rendering logic).
- [ ] Unhealthy-is-data proven: `test_unit_inactive_reported_not_error` passes (RPC-level Ok with `dnsmasq_active: false`).
- [ ] **Anti-over-suppression:** `test_consistency_matches_rendered` passes — a correctly-projected host is NOT flagged inconsistent.
- [ ] Only assigned files changed: `git diff origin/main --stat` touches `crates/uaa-pxe/src/health.rs`, `crates/uaa-pxe/src/main.rs`, and headers only.
- [ ] `cargo test --lib --offline && cargo build --offline` exits 0; clippy clean.
- [ ] File headers bumped on every changed file (`git diff origin/main --stat` files each show a version bump; guids unchanged).

## Commit message

```
feat(pxe): implement Health RPC — unit state, TFTP self-probe, boot-target consistency (ws6-pxe)

Fills the PX-01 health.rs stub: systemd is-active checks for dnsmasq AND
tftpd-hpa (tftpd serves TFTP, not dnsmasq — and logs nothing, hence the
curl tftp:// self-probe), plus Decision-13 consistency verification that
re-renders intent via boot_config::render_host_lines and diffs the on-disk
hostsdir/optsdir projection. Unhealthy is report data, never an RPC error.
All probes via CommandExecutor mocks; 6 unit tests.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

Additive (stub-fill): if `grep -n "pub async fn tftp_probe" crates/uaa-pxe/src/health.rs` hits, the task is already applied — run the Acceptance criteria checks instead of re-applying. Rollback = revert the single commit; `health.rs` returns to the PX-01 stub and the `Health` RPC arm to `unimplemented` — `boot_config.rs`, the other stubs, and all sibling worktrees stay untouched.
