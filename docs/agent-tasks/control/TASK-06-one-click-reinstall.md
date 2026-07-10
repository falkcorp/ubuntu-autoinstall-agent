<!-- file: docs/agent-tasks/control/TASK-06-one-click-reinstall.md -->
<!-- version: 1.0.0 -->
<!-- guid: e9c3dedb-83c6-44b7-87c5-19e3d2097c02 -->
<!-- last-edited: 2026-07-10 -->

# TASK-06 — Fill reinstall.rs: one-click ReinstallMachine — dual-layer boot-target reconciliation, power cycle, bounded watch + fail-safe flip-back + cooldown (ws2-control)

**Priority:** P1 · **Effort:** M · **Recommended subagent:** Sonnet-class · rust-service subagent · **Why:** hardware-adjacent orchestration with deny-list + refusal rules (never unimatrixone, never unapproved) — every guard here prevents an unattended machine wipe. · **Depends on:** TASK-01 + core-proto CP-03 (wave-4 gated: CT-01 merged for the `reinstall.rs` stub AND CP-03 merged for `FleetConfig` with the unimatrixone power deny-list)

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/control-one-click-reinstall" -b agent/control-one-click-reinstall origin/main
cd "$REPO/.worktrees/control-one-click-reinstall"
git rebase origin/main
```

(Protocol also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

## Goal

Fill the CT-01 stub `crates/uaa-control/src/reinstall.rs` (your EXCLUSIVE file) with `ReinstallMachine` per spec C3 + Decision 13: set `machines.boot_target = custom-autoinstall` (the ONE authoritative field) → project it to BOTH layers (uaa-web iPXE flip AND uaa-pxe dnsmasq target; **refuse if either layer cannot be reconciled**) → power cycle via the uaa-core power library (explicit `off` then `on` — reset/cycle are unrepresentable there) → watch install events until success or a bounded timeout; **on timeout, attempt a fail-safe flip-back to `local-disk` on both layers + loud alert**; a reinstall counter refuses a re-trigger within a cooldown window unless the operator explicitly confirms.

Hard refusals (fail-closed, BEFORE any side effect): the FleetConfig power deny-list (`unimatrixone`) and any host whose registry `status != approved`.

Purely additive to `reinstall.rs`. Reuse — do not invent parallels:
- **`run_power_action` / `PowerAction`** from the uaa-core power library (`src/power/mod.rs` pre-CP-01 — verify grep below; `crates/uaa-core/src/power/mod.rs` after). Do NOT shell out to ipmitool or ssh yourself — the library already embeds the IPMI-via-server rule. Wrap it behind a narrow local `trait PowerControl` so tests inject a mock (the default impl calls `run_power_action` with an `SshClient`-backed executor at runtime only).
- **`FleetConfig`** (CP-03, `crates/uaa-core/src/fleet.rs`) for the deny-list — do NOT hardcode `unimatrixone` here; read `fleet.power_deny_list`.
- **`WebClient` / `PxeClient` traits** from CT-05's `saga.rs` (same wave — if unmerged when you start, declare identical local traits and leave a one-line TODO to unify; do not block).

## Background (verify before editing)

- Spec: C3 "One-click reinstall" bullet (bounded watch, fail-safe flip-back, cooldown, refusals), Decision 13 (boot_target single source of truth; projections must reconcile or refuse), Decision 15 (power stays a uaa-core library; ipmitool via `ssh 172.16.2.30`).
- Sequence semantics (spell twice — here and Step 3): (1) guards: deny-list → typed refusal; `status != approved` → typed refusal; cooldown active AND `confirm != true` → typed refusal naming the remaining window. (2) registry write `boot_target=custom-autoinstall`. (3) project: `WebClient.flip_boot_target(mac, custom-autoinstall)` AND `PxeClient.set_boot_target(mac, custom-autoinstall)` — if EITHER fails, flip both back to `local-disk` best-effort, restore the registry field, return a reconciliation error (the host must never be left half-projected). (4) power cycle: `PowerControl.off(host)` then `PowerControl.on(host)` (never reset/cycle). (5) watch: poll the install-event source (narrow `trait InstallWatch { async fn latest_status(&self, mac) -> Result<Option<InstallStatus>>; }`) with an injectable clock until `success` → done, or `failed`/timeout (default 45 min, config) → fail-safe: flip BOTH layers back to `local-disk`, set registry `boot_target=local-disk`, `tracing::error!` alert, return a timeout outcome. Flip-back best-effort failures are loudly logged AND surfaced in the outcome (never swallowed).
- Cooldown: persist `last_reinstall_at` per mac (via the registry store seam); a re-trigger within 30 min (config) requires `confirm: true` in the request; the refusal message names the minutes remaining.
- Everything is trait-injected; **no test touches hardware, SSH, or a live service** — mock power, mock web/pxe clients, mock watch, mock registry, tick-controlled clock.

**Path map:** after CP-01 (wave 1) merges, `src/**` lives at
`crates/uaa-core/src/**` and the CLI at `crates/uaa/src/**`. The greps above cite
pre-move paths (verifiable on today's main); at execution time run them at the old
path, then the mapped path. Zero hits at BOTH = STOP and report.

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

- **Re-verify these anchors before editing** — line numbers drift; zero hits at both
  old and mapped path = STOP and report:
  ```bash
  grep -n "pub async fn run_power_action" src/power/mod.rs            # expect: 1 hit (~line 159; mapped: crates/uaa-core/src/power/mod.rs)
  grep -n "cooldown" docs/specs/constellation-design.md               # expect: 1 hit (~line 470 — bounded-reinstall rule)
  grep -n "consistent: bool\|refuse if either layer" docs/specs/constellation-design.md  # expect: 1+ hits (Decision 13 reconciliation)
  grep -n "pub struct FleetConfig" docs/specs/constellation-design.md # expect: 1 hit (C1 sketch; the real one lands with CP-03 at crates/uaa-core/src/fleet.rs)
  test -f crates/uaa-control/src/reinstall.rs && echo OK              # expect: OK (wave gate: CT-01 merged; missing = STOP, too early)
  grep -n "power_deny_list\|deny" crates/uaa-core/src/fleet.rs        # expect: 1+ hits at execution time (wave gate: CP-03 merged; missing = STOP)
  ```

## Step-by-step

1. Run the ⛔ START HERE block, then every anchor grep above (old path, then mapped path). Any zero-hit-at-both / missing-file result → STOP and report.

2. **Seams + types.** `ReinstallRequest { mac, confirm: bool }`, `ReinstallOutcome { Done, TimedOutFlippedBack { flip_back_ok: bool }, Refused(RefusalReason) }`, `RefusalReason { DenyListed, NotApproved, CooldownActive { remaining_min }, Unreconciled(String) }`; `ReinstallDeps` bundling `&dyn WebClient/PxeClient/PowerControl/InstallWatch` + registry seam + `FleetConfig` + clock + config `{ watch_timeout, cooldown }`.
3. **Driver `pub async fn reinstall_machine(deps, req) -> Result<ReinstallOutcome>`** implementing EXACTLY the Background sequence: guards first (deny-list, not-approved, cooldown-without-confirm — each returns BEFORE any mock records a call), then registry write, dual projection with refuse-and-restore on single-layer failure, power off→on, bounded watch, fail-safe flip-back + alert on timeout/failure, cooldown stamp on every attempt that reached the power step. Repeat the never-half-projected law: one layer Ok + one layer Err → both flipped back + registry restored + `Unreconciled` returned.
4. **Wire** a thin `pub async fn handle_reinstall(...)` entry the operator plane (CT-07) and gRPC layer will call; runtime `PowerControl` default impl calls `run_power_action` (uaa-core) — construction only, no logic.
5. **Unit tests** (all-mock, tick clock, no sleeps): `test_denylist_refused_zero_side_effects` (unimatrixone in `FleetConfig.power_deny_list` → `Refused(DenyListed)`, ALL mocks record 0 calls); `test_unapproved_refused_zero_side_effects`; `test_cooldown_refused_without_confirm` (+ names remaining minutes) and `test_cooldown_bypassed_with_confirm`; `test_single_layer_failure_flips_back_and_restores` (pxe Err → web flipped back, registry `boot_target` restored to prior value, outcome `Unreconciled`, power NEVER called); `test_power_off_then_on_order` (recorded power calls exactly `[off, on]` — never reset/cycle strings anywhere); `test_watch_success_done`; `test_watch_timeout_flips_back_and_alerts` (both layers re-flipped to local-disk, registry restored, outcome `TimedOutFlippedBack{flip_back_ok:true}`); `test_flip_back_failure_surfaced` (`flip_back_ok:false`, error logged — not swallowed); and the anti-over-suppression test `test_approved_host_reinstall_happy_path` — an approved, non-denied, out-of-cooldown host flows through EVERY guard to `Done` with call order `[registry write, web flip, pxe set, power off, power on, watch…]`.
6. Bump the file header (version + last-edited) on every file you touch; new files get a fresh 4-line header with a new uuid4 (`uuidgen | tr 'A-F' 'a-f'`).

## How to test

```bash
cargo test --lib --offline && cargo build --offline
# Expected: all tests pass (baseline 311 + prior control tests + the ~10 new tests), build clean
cargo clippy --offline -- -D warnings
# Expected: no warnings
grep -rn "chassis power reset\|chassis power cycle" crates/uaa-control/src/
# Expected: 0 hits (unreliable verbs stay unrepresentable)
grep -rn "unimatrixone" crates/uaa-control/src/reinstall.rs | grep -v "test\|//"
# Expected: 0 hits (deny-list comes from FleetConfig, never hardcoded in the driver)
```

## Acceptance criteria

- [ ] Only `reinstall.rs` (+ minimal wiring line) changed: `git diff origin/main --stat` shows no other `crates/uaa-control/src/` file.
- [ ] Refusals fail-closed with zero side effects: `test_denylist_refused_zero_side_effects`, `test_unapproved_refused_zero_side_effects`, `test_cooldown_refused_without_confirm` all assert 0 recorded mock calls.
- [ ] Never-half-projected proven: `test_single_layer_failure_flips_back_and_restores` passes (both layers restored, registry restored, power never invoked).
- [ ] Bounded watch + fail-safe proven: `test_watch_timeout_flips_back_and_alerts` and `test_flip_back_failure_surfaced` pass.
- [ ] Power discipline: `test_power_off_then_on_order` passes; `grep -rn "chassis power reset\|chassis power cycle" crates/uaa-control/src/` → 0 hits; power goes only through the `PowerControl` seam (`grep -n "Command::new\|ssh " crates/uaa-control/src/reinstall.rs` → 0 code hits).
- [ ] **Anti-over-suppression:** `test_approved_host_reinstall_happy_path` passes — a legitimate reinstall clears every guard and completes.
- [ ] `cargo test --lib --offline && cargo build --offline` exits 0; clippy clean; no test touches hardware, SSH, or network.
- [ ] File headers bumped on every changed file (`git diff origin/main --stat` files each show a version bump; guids unchanged).

## Commit message

```
feat(control): one-click ReinstallMachine — dual-layer reconciliation, power cycle, bounded watch, fail-safe flip-back, cooldown (ws2-control)

Fills the CT-01 reinstall.rs stub per spec C3/Decision 13: boot_target is the
single authoritative field, projected to BOTH uaa-web and uaa-pxe or refused
and restored (never half-projected); power cycles off-then-on through the
uaa-core power library seam (reset/cycle unrepresentable); install watch is
bounded with fail-safe flip-back to local-disk + loud alert on timeout; a
30-min cooldown refuses re-triggers without explicit confirm. Deny-list
(unimatrixone) and not-approved refusals return before any side effect.
All seams mocked — zero hardware/SSH/network in tests.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

If `grep -n "pub async fn reinstall_machine" crates/uaa-control/src/reinstall.rs` hits, already applied — run the acceptance checks instead of re-applying. Rollback = revert the single commit; `reinstall.rs` returns to CT-01's header-only stub; the feature is dormant unless invoked by the operator plane, and no host, BMC, or server state exists to unwind.
