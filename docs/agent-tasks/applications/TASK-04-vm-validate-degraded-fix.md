<!-- file: docs/agent-tasks/applications/TASK-04-vm-validate-degraded-fix.md -->
<!-- version: 1.1.0 -->
<!-- guid: 742f395d-3e7d-4324-b1d4-d055c46d660c -->
<!-- last-edited: 2026-07-16 -->

# TASK-04 — Fix `vm-validate.sh` accepting `degraded` as PASS (DS-APP-04)

**Priority:** P0 · **Effort:** S · **Recommended subagent:** Haiku-class · shell subagent · **Why:** a two-line shell-gate fix with an exact before/after; mechanical. · **Depends on:** none (dispatch immediately, ahead of wave 1)

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/applications-vm-validate-degraded-fix" -b agent/applications-vm-validate-degraded-fix origin/main
cd "$REPO/.worktrees/applications-vm-validate-degraded-fix"
git rebase origin/main
```

(Protocol also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

## Goal

`scripts/vm-validate.sh`'s Stage 6 accepts `systemctl is-system-running` returning **`degraded`** as a PASS. But `degraded` is returned by systemd **precisely when one or more units have FAILED**. So the QEMU gate — the gate that authorizes touching real hardware — currently reports success on a machine with broken services.

**This is a pre-existing bug, not new work**, and it blocks nothing else in this package. Fix: treat `degraded` as a FAIL, and print which units failed so the failure is actionable rather than a bare exit code.

REUSE — do not invent parallels:

- **`fail_stage`** is the existing failure helper — verify: `grep -n "fail_stage()" scripts/vm-validate.sh` (or `grep -n "fail_stage 6"`). Use it; do NOT invent a new exit path or call `exit 1` directly.
- **`ssh_run`** is the existing remote-command helper — verify: `grep -n "ssh_run()" scripts/vm-validate.sh`. Use it for the new `systemctl list-units --failed` call; do NOT hand-roll an `ssh` invocation.
- **`$ASSERT_LOG`** is where every assertion's raw output is appended. Append yours too, mirroring the surrounding lines.

## Background (verify before editing)

- The current logic reads (re-verify with the grep block below — do not trust this quote's position):
  ```bash
  MU_OUT="$(ssh_run 30 root "systemctl is-system-running --wait" 2>&1 || true)"
  echo "$MU_OUT" >>"$ASSERT_LOG"
  if echo "$MU_OUT" | grep -qE "running|degraded"; then
    echo "PASS: multi-user reached (systemctl is-system-running: $MU_OUT)" | tee -a "$ASSERT_LOG"
  else
    ...
  ```
  The `|degraded` alternation is the bug.
- `systemctl is-system-running` exit-code/output semantics that matter here: `running` = all units OK; **`degraded` = system is up but ≥1 unit FAILED**; `starting`/`initializing` = still coming up (the `--wait` flag blocks past these); `maintenance`/`stopping` = not a healthy multi-user state.
- Edge semantics (spelled out here AND in acceptance):
  - **`running`** → PASS, unchanged.
  - **`degraded`** → **FAIL**, and print `systemctl list-units --failed --no-legend` so the operator sees *which* unit died. A bare "FAIL: degraded" is not actionable.
  - **The existing `else` fallback** (which retries `systemctl is-active multi-user.target`) must **not** rescue `degraded`. `multi-user.target` is `active` on a degraded system, so routing `degraded` into that fallback re-introduces the bug through the back door. Handle `degraded` explicitly **before** the fallback.
  - **Empty/unreachable output** (ssh failed) → existing behavior, unchanged. Do not widen this task.

**HARD RULES (non-negotiable):**
- NO hardware actions. This script is only ever run against QEMU. Do not run it in this task — a `bash -n` syntax check plus the greps below is the whole verification surface.
- NEVER wipe/write/deploy on 172.16.2.30 or len-serv-003. NEVER power on unimatrixone.
- Purely additive/surgical: change the `degraded` handling only. Do NOT restructure Stage 6, do NOT touch stages 0–5, do NOT reformat the file.
- Stay inside your worktree; never `git push`, `gh pr`, or merge — report done and stop.

- **Re-verify these anchors before editing** — line numbers drift; zero hits = STOP and report:
  ```bash
  grep -n 'grep -qE "running|degraded"' scripts/vm-validate.sh
  # expect: 1 hit (~line 517) — THE BUG: the edit target
  grep -n "is-system-running" scripts/vm-validate.sh
  # expect: 1-2 hits — the assertion block
  grep -n "fail_stage 6" scripts/vm-validate.sh
  # expect: 2-3 hits — the Stage-6 failure calls whose style you copy. (Do NOT grep bare
  # "fail_stage": ~18 hits across all stages, and an ambiguous target is how a weak
  # executor edits the wrong stage.)
  grep -n "tee -a \"\$ASSERT_LOG\"" scripts/vm-validate.sh
  # expect: several hits — the exact append idiom to copy for your PASS line.
  # For the one you are editing, anchor on the assertion itself:
  grep -n -A2 "is-system-running" scripts/vm-validate.sh
  # expect: 1 block — MU_OUT assignment + the ASSERT_LOG append + the buggy condition
  ```

## Step-by-step

1. Locate the assertion with `grep -n 'grep -qE "running|degraded"' scripts/vm-validate.sh` — never trust a line number from this brief.
2. Replace the single accepting condition with an explicit three-way branch, keeping the surrounding style:
   ```bash
   MU_OUT="$(ssh_run 30 root "systemctl is-system-running --wait" 2>&1 || true)"
   echo "$MU_OUT" >>"$ASSERT_LOG"
   if echo "$MU_OUT" | grep -q "degraded"; then
     FAILED_UNITS="$(ssh_run 15 root "systemctl list-units --failed --no-legend" 2>&1 || true)"
     echo "$FAILED_UNITS" >>"$ASSERT_LOG"
     fail_stage 6 "system is degraded — one or more units FAILED: ${FAILED_UNITS}"
   elif echo "$MU_OUT" | grep -q "running"; then
     echo "PASS: multi-user reached (systemctl is-system-running: $MU_OUT)" | tee -a "$ASSERT_LOG"
   else
     # ... existing fallback, UNCHANGED ...
   ```
   The `degraded` check must come **first** — otherwise the `else` fallback's `is-active multi-user.target` (which is `active` on a degraded system) silently rescues it.
3. Do not change anything else in the file.
4. Bump the header (`version` + `last-edited`) in `scripts/vm-validate.sh`; keep its existing guid.

**Anti-over-suppression:** this task tightens a gate, so the over-suppression risk runs the other way — a too-broad `degraded` match failing a healthy run. The acceptance criteria therefore require proving the **`running` path still PASSes** (the happy path survives the new guard), not only that `degraded` now fails.

## How to test

There is no unit-test harness for this script, and running the real QEMU gate is out of scope for this task. Verify mechanically:

```bash
bash -n scripts/vm-validate.sh
# Expected: exit 0 (no syntax error).

# Prove the branch logic in isolation — degraded must FAIL, running must PASS:
for s in running degraded; do
  if echo "$s" | grep -q "degraded"; then echo "$s -> FAIL(correct)";
  elif echo "$s" | grep -q "running"; then echo "$s -> PASS(correct)"; fi
done
# Expected exactly:
#   running -> PASS(correct)
#   degraded -> FAIL(correct)

cargo test --lib --offline
# Expected: 634 passed, 0 failed — unchanged; this task touches no Rust.
```

## Acceptance criteria

- [ ] `bash -n scripts/vm-validate.sh` exits 0 — verify: `bash -n scripts/vm-validate.sh && echo SYNTAX_OK`
- [ ] `degraded` is no longer accepted — verify: `grep -c 'grep -qE "running|degraded"' scripts/vm-validate.sh` returns **0**
- [ ] `degraded` explicitly fails — verify: `grep -c 'fail_stage 6 "system is degraded' scripts/vm-validate.sh` returns **1**
- [ ] Failed units are printed — verify: `grep -c "list-units --failed" scripts/vm-validate.sh` returns **1**
- [ ] The `degraded` branch precedes the `running` branch — verify: `grep -n 'grep -q "degraded"\|grep -q "running"' scripts/vm-validate.sh` shows the `degraded` line at a **lower** line number
- [ ] Anti-over-suppression: the `running` path still PASSes — verify with the two-case loop in How-to-test; expected output is exactly `running -> PASS(correct)` / `degraded -> FAIL(correct)`
- [ ] Nothing else changed — verify: `git diff --stat origin/main` shows **only** `scripts/vm-validate.sh`, with fewer than 20 changed lines
- [ ] `cargo test --lib --offline` still 634 passed — verify: `cargo test --lib --offline 2>&1 | grep -E "^test result"`
- [ ] File header bumped — verify: `grep -n "last-edited: 2026-07" scripts/vm-validate.sh`

## Commit message

```
fix(vm-gate): treat systemctl degraded as FAIL, not PASS (DS-APP-04)

vm-validate.sh's Stage 6 accepted `systemctl is-system-running` returning
"degraded" as a PASS. systemd returns degraded precisely when one or more
units have FAILED — so the QEMU gate that authorizes touching real hardware
reported success on a machine with broken services.

Now: degraded fails the stage and prints `systemctl list-units --failed` so
the failure is actionable. The degraded check runs BEFORE the existing
is-active multi-user.target fallback, which would otherwise rescue it —
multi-user.target is active on a degraded system.

Pre-existing bug, found while planning the deploy-system package.

Co-Authored-By: Claude <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

**Polarity: transform.** If `grep -c 'grep -qE "running|degraded"' scripts/vm-validate.sh` returns **0** AND `grep -c 'fail_stage 6 "system is degraded' scripts/vm-validate.sh` returns **1**, the fix is already applied — run the acceptance checks instead of re-applying. Rollback = revert the single commit, restoring the old (buggy) permissive check; no data, schema, or Rust code is touched. DS-APP-05 also edits `scripts/vm-validate.sh` and must rebase after this merges — see the collision table in `../BREAKDOWN-2026-07-16.md`.
