<!-- file: docs/agent-tasks/applications/TASK-05-vm-gate-cockroach-readiness.md -->
<!-- version: 1.0.0 -->
<!-- guid: 6733cbfd-793a-424f-a03e-ea63ae75b87b -->
<!-- last-edited: 2026-07-16 -->

# TASK-05 — VM gate: Cockroach readiness assertion + `vm-test.yaml` application spec (DS-APP-05)

**Priority:** P1 · **Effort:** M · **Recommended subagent:** Sonnet-class · shell+yaml subagent · **Why:** gate semantics — `is-active` is insufficient (a node can sit active retrying a join forever), so the assertion must prove readiness. · **Depends on:** DS-APP-03 (the install step) **and** DS-APP-04 (the `degraded` fix)

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/applications-vm-gate-cockroach-readiness" -b agent/applications-vm-gate-cockroach-readiness origin/main
cd "$REPO/.worktrees/applications-vm-gate-cockroach-readiness"
git rebase origin/main
```

(Protocol also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

**Wave gate — BOTH must be merged:**
- `grep -n "pub fn derive_cockroach_endpoints" crates/uaa-core/src/network/ssh_installer/applications.rs` → 1 hit (DS-APP-03)
- `grep -c 'fail_stage 6 "system is degraded' scripts/vm-validate.sh` → 1 (DS-APP-04)

Zero hits on either = gate not met: STOP and report.

## Goal

Make the QEMU gate **actually prove** a Cockroach node works. Today it cannot:

1. `vm-test.yaml` has **no application entry**, so with `applications: []` the gate installs no Cockroach and still goes green.
2. Stage 6 asserts only LUKS + `rpool`/`bpool` + multi-user — there is **no Cockroach assertion at all**.

Add a single-node `CockroachSpec` to `vm-test.yaml` and a **readiness** assertion to Stage 6.

REUSE — do not invent parallels:

- **`ssh_run`** for remote commands — verify: `grep -n "ssh_run" scripts/vm-validate.sh`. Do NOT hand-roll `ssh`.
- **`fail_stage`** for failures — verify: `grep -n "fail_stage" scripts/vm-validate.sh`. Do NOT `exit 1` directly.
- **`$ASSERT_LOG`** — append raw output the same way the surrounding assertions do.
- **The existing Stage-6 assertion style** (`cryptsetup status luks`, `zpool list`) is your template — verify: `grep -n "cryptsetup status luks" scripts/vm-validate.sh`.

## Background (verify before editing)

- **⚠ `is-active` is NOT sufficient, and this is the whole point of the task.** `cockroach.service` has `Restart=always`, so a node that starts, fails to join, and retries forever sits `active (running)` indefinitely. An `is-active` assertion would pass on a node that never joined a cluster. The gate must assert **readiness**: the node answers SQL. Use the local SQL port with the node's own certs:
  ```bash
  cockroach sql --certs-dir=/var/lib/cockroach/certs --host=<sql_addr> -e 'SELECT 1'
  ```
  (or the HTTP health endpoint `http://127.0.0.1:38080/health?ready=1`). `is-active` may be asserted **in addition**, never instead.
- **`vm-test.yaml` is a single VM with no siblings.** Its `CockroachSpec` must have `seed_ip` = the VM's own IP (`10.0.2.15`), so the node is its own seed and forms a one-node cluster. `locality` should be VM-specific (e.g. `region=vm,cluster-unit=qemu`) — do **not** copy the fleet's `region=us,cluster-unit=lenovo` into a throwaway VM.
- **`vm-test.yaml` is a THROWAWAY, VM-only config.** Its header says every secret is disposable and committed deliberately. Keep that property: your addition contains no secret. Do **not** copy fleet values into it, and do **not** add `REPLACE_AT_PLACE_TIME` (the gate refuses to run on a config containing placeholders — verify: `grep -n "REPLACE_AT_PLACE_TIME" scripts/vm-validate.sh`).
- **Cockroach in QEMU needs egress** to `binaries.cockroachdb.com` and reachability to the cert endpoint. QEMU user-mode networking (`-netdev user`) provides outbound NAT, so the binary download works. **The cert fetch targets `172.16.2.30:25000`, which a user-mode-networked VM cannot reach.** Handle this explicitly: either (a) point the VM's `CockroachSpec` at a cert path that the gate pre-seeds, or (b) if the cert fetch cannot work in QEMU, **say so in your report and fail the task rather than weakening the assertion** — a gate that skips the cert step proves less than it claims. Do not silently stub it.
- Edge semantics (spelled out here AND in acceptance):
  - **Cockroach not ready within the timeout** → `fail_stage 6`, printing `systemctl status cockroach --no-pager` and the last journal lines so the failure is actionable.
  - **Readiness succeeds** → PASS.
  - **A pre-DS-APP-03 target** (no cockroach installed) → the assertion must fail, not skip. A skipped assertion is how the current gate reports success on nothing.

**HARD RULES (non-negotiable):**
- NO hardware actions. This script only ever runs against QEMU. **Do NOT run the full gate in this task** — it needs a QEMU host and minutes of runtime; `bash -n` plus the greps are your verification surface. If you believe a live run is required, say so in your report.
- NEVER wipe/write/deploy on 172.16.2.30 or len-serv-003. NEVER power on unimatrixone.
- Do NOT weaken any existing Stage-6 assertion. Purely additive.
- Do NOT re-introduce `degraded`-as-PASS (DS-APP-04 fixed it).
- Stay inside your worktree; never `git push`, `gh pr`, or merge — report done and stop.

- **Re-verify these anchors before editing** — line numbers drift; zero hits = STOP and report:
  ```bash
  grep -n "pub fn derive_cockroach_endpoints" crates/uaa-core/src/network/ssh_installer/applications.rs
  # expect: 1 hit — DS-APP-03 merged (0 hits = wave gate not met, STOP)
  grep -c 'fail_stage 6 "system is degraded' scripts/vm-validate.sh
  # expect: 1 — DS-APP-04 merged (0 = wave gate not met, STOP)
  grep -n "cryptsetup status luks" scripts/vm-validate.sh
  # expect: 1 hit (~line 499) — the Stage-6 assertion style to mirror; add yours nearby
  grep -n "REPLACE_AT_PLACE_TIME" scripts/vm-validate.sh
  # expect: 1 hit (~line 233) — the placeholder refusal; your yaml addition must not trip it
  grep -n "expect_fido2\|tang_servers" examples/configs/install/vm-test.yaml
  # expect: 2 hits — the documented VM-gate non-goals; your addition sits alongside
  ```

## Step-by-step

1. In `examples/configs/install/vm-test.yaml`, add a single-node application entry with a comment explaining it is a one-node cluster (`seed_ip` = self) and that its values are VM-only:
   ```yaml
   # Single-node CockroachDB so the QEMU gate actually exercises the Phase-5
   # application path. seed_ip is this VM itself: a one-node cluster seeds
   # itself. VM-only values — never copy the fleet's locality here.
   applications:
     - kind: cockroach
       seed_ip: 10.0.2.15
       locality: region=vm,cluster-unit=qemu
   ```
   Bump the file's header (`version` + `last-edited`); keep its guid.
2. In `scripts/vm-validate.sh`'s Stage 6, **after** the existing LUKS/zpool/multi-user assertions, add a readiness assertion with a bounded retry (Cockroach takes seconds to accept SQL after `start`):
   ```bash
   CRDB_READY=""
   for _ in $(seq 1 30); do
     if ssh_run 15 root "cockroach sql --certs-dir=/var/lib/cockroach/certs --host=10.0.2.15:36257 -e 'SELECT 1'" >>"$ASSERT_LOG" 2>&1; then
       CRDB_READY=yes; break
     fi
     sleep 5
   done
   if [ -n "$CRDB_READY" ]; then
     echo "PASS: cockroach answers SELECT 1 (node is ready)" | tee -a "$ASSERT_LOG"
   else
     ssh_run 15 root "systemctl status cockroach --no-pager" >>"$ASSERT_LOG" 2>&1 || true
     ssh_run 15 root "journalctl -u cockroach --no-pager -n 50" >>"$ASSERT_LOG" 2>&1 || true
     fail_stage 6 "cockroach never became ready (SELECT 1 failed for 150s) — see $ASSERT_LOG"
   fi
   ```
   **Do not** substitute `systemctl is-active cockroach` for this — `Restart=always` makes it pass on a node retrying a join forever.
3. Do not change any existing assertion.
4. Bump the header on `scripts/vm-validate.sh`; keep its guid.

**Anti-over-suppression:** this task tightens a gate, so over-blocking is the risk — a too-short timeout failing a healthy node. The 30×5s bounded retry (150s) is the guard against that, and the acceptance criteria require the retry loop to be present rather than a single-shot check. The complementary risk (under-blocking) is covered by requiring the assertion to fail on a target with no Cockroach installed.

## How to test

There is no unit-test harness for this script, and a live QEMU run is out of scope for this task (it needs a QEMU host and minutes of runtime).

```bash
bash -n scripts/vm-validate.sh
# Expected: exit 0.

python3 -c "import yaml,sys; d=yaml.safe_load(open('examples/configs/install/vm-test.yaml')); \
  a=d['applications']; assert len(a)==1 and a[0]['kind']=='cockroach', a; \
  assert a[0]['seed_ip']=='10.0.2.15', a; print('vm-test.yaml OK:', a)"
# Expected: vm-test.yaml OK: [{'kind': 'cockroach', ...}]

cargo test --lib --offline
# Expected: passes — vm-test.yaml must still deserialize into InstallationConfig.
#           (test_install_example_configs_round_trip covers the four fleet configs;
#            vm-test is loaded by the gate, so a parse break shows up here or in build.)
```

## Acceptance criteria

- [ ] `bash -n scripts/vm-validate.sh` exits 0 — verify: `bash -n scripts/vm-validate.sh && echo SYNTAX_OK`
- [ ] `vm-test.yaml` parses and carries exactly one cockroach app — verify: the `python3 -c` one-liner in How-to-test prints `vm-test.yaml OK:`
- [ ] **Readiness, not `is-active`** — verify: `grep -c "SELECT 1\|health?ready=1" scripts/vm-validate.sh` returns ≥1, and `grep -c "is-active cockroach" scripts/vm-validate.sh` returns **0**
- [ ] Anti-over-suppression: a bounded retry exists so a slow-but-healthy node is not failed — verify: `grep -c "seq 1 30" scripts/vm-validate.sh` returns 1
- [ ] Failures are actionable — verify: `grep -c "journalctl -u cockroach" scripts/vm-validate.sh` returns 1
- [ ] `degraded` is still a FAIL (DS-APP-04 not regressed) — verify: `grep -c 'fail_stage 6 "system is degraded' scripts/vm-validate.sh` returns 1
- [ ] No placeholder in the VM config — verify: `grep -c "REPLACE_AT_PLACE_TIME" examples/configs/install/vm-test.yaml` returns **0**
- [ ] No fleet values leaked into the VM config — verify: `grep -c "cluster-unit=lenovo\|172.16" examples/configs/install/vm-test.yaml` returns **0**
- [ ] `cargo test --lib --offline` still green — verify: `cargo test --lib --offline 2>&1 | grep -E "^test result"`
- [ ] File headers bumped — verify: `grep -n "last-edited: 2026-07" scripts/vm-validate.sh examples/configs/install/vm-test.yaml`

## Commit message

```
test(vm-gate): assert cockroach readiness, add single-node spec to vm-test (DS-APP-05)

The QEMU gate could not prove the Phase-5 application path worked:
vm-test.yaml had no application entry, so applications: [] installed nothing
and the gate went green; and Stage 6 asserted only LUKS/zpool/multi-user.

Adds a single-node CockroachSpec (seed_ip = the VM itself) and a READINESS
assertion. is-active is deliberately not used: cockroach.service has
Restart=always, so a node retrying a join forever sits active and would pass.
The gate now requires the node to answer SELECT 1, with a bounded 150s retry
so a slow-but-healthy start is not failed, and prints systemctl status +
journalctl on failure.

Co-Authored-By: Claude <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge. **If the cert fetch cannot be made to work under QEMU user-mode networking, report that rather than weakening or skipping the assertion** — a gate that skips a step proves less than it claims.

## Idempotency / Rollback

**Polarity: additive.** If `grep -c "SELECT 1" scripts/vm-validate.sh` returns ≥1 AND `grep -c "kind: cockroach" examples/configs/install/vm-test.yaml` returns 1, this task is already applied — run the acceptance checks instead of re-applying. Rollback = revert the single commit; the gate returns to its LUKS/zpool/multi-user assertions and `vm-test.yaml` to `applications: []` (no application installed). No fleet config, data, or schema is touched — `vm-test.yaml` is a throwaway VM-only config. DS-APP-04 also edits `scripts/vm-validate.sh`; this task rebases after it.
