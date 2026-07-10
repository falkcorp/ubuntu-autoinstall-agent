<!-- file: docs/agent-tasks/testing-gates/TASK-03-constellation-e2e-vm.md -->
<!-- version: 1.0.0 -->
<!-- guid: 05871e36-182d-4f28-a685-d641a7d2adad -->
<!-- last-edited: 2026-07-10 -->

# TASK-03 — Build the constellation e2e VM gate: scripts/vm-validate-constellation.sh (ws10-gates)

**Priority:** P1 · **Effort:** L · **Recommended subagent:** Sonnet-class · shell-harness subagent · **Why:** the M5 gate; harness composition, no new product logic. · **Depends on:** none inside this workstream (wave-8 gated: global waves 4–7 MERGED — IP-04 parity fixtures, PK-01 install CA + EnrollService, PK-02 agent enroll client, WB-02 placement RPCs, and PX-01 pxe crate must all be on `origin/main` before dispatch)

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/testing-gates-constellation-e2e-vm" -b agent/testing-gates-constellation-e2e-vm origin/main
cd "$REPO/.worktrees/testing-gates-constellation-e2e-vm"
git rebase origin/main
```

(Protocol also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

## Goal

Create a NEW script `scripts/vm-validate-constellation.sh` — the constellation-era extension of the existing VM gate — and extend `docs/vm-validation.md` to document it. Per the design spec (`docs/specs/constellation-design.md`, milestone **M5 — E2E VM gate. THE gate before any hardware.**, and the Testing-table row "VM e2e (extended vm-validate): enroll→approve→cert→install→19-check verify sweep in QEMU+swtpm"), the script:

1. launches `uaa-control`, `uaa-web`, and `uaa-pxe` on LOOPBACK with a temp CA (workdir-scoped, never `/var/lib/uaa` or `/etc/uaa`) and a `cockroach start-single-node --insecure` backing store (documented host dependency; skip-with-loud-error if absent),
2. boots the VM agent through the existing QEMU+swtpm machinery,
3. walks the full lifecycle: enroll → approve (via API) → cert issued → install → verify sweep,
4. prints a PASS/FAIL machine-greppable report mirroring the existing `==== VERIFY-ON-VM REPORT ====` / `GATE: PASS` / `GATE: FAIL (...)` format.

REUSE — do not invent parallels:

- **`scripts/vm-validate.sh` helper functions** (`die`, `stage_echo`, `ssh_run`, `scp_run`, `wait_for_ssh` — verify: `grep -n "ssh_run() {" scripts/vm-validate.sh`). The new script SOURCES these by extraction (Step 3) — it **NEVER modifies `scripts/vm-validate.sh`** (that script stays authoritative for the single-host Path-B gate until TP-04's port is proven). Do NOT copy-paste the function bodies; do NOT rewrite an SSH wrapper.
- **The report format** of `print_report` (`grep -n '==== VERIFY-ON-VM REPORT ====' scripts/vm-validate.sh` — line ~118): the new report uses the same banner, the same `GATE: PASS` / `GATE: FAIL (<first failing stage>)` terminal lines, and the same `=============================` footer, so existing operator greps keep working.
- **Machine-plane approve endpoint parity**: approval goes through the ported `POST /api/approve/<mac>` (ground truth `grep -n "/api/approve" scripts/autoinstall-agent.py` — regex handler ~line 332), served by uaa-control after IP-03/IP-04; the CSR is approved via the PK-01 enrollment-plane approve path.
- **`examples/configs/install/vm-test.yaml`** — the existing throwaway VM config (TASK-01 artifact). Reuse it; do NOT create a second VM config.

Purely additive: one new script + one documentation edit. No Rust code changes, no changes to any existing script.

## Background (verify before editing)

- `scripts/vm-validate.sh` is the existing 8-stage (0–7) single-host gate: preflight → workspace → boot-iso → interrogate → install → boot-disk → assert → report. Its helpers live between stable markers: one-liners `die()` (~line 48) and `stage_echo()` (~line 98), and an SSH/SCP helper block from the literal marker line `# --- SSH/SCP helpers ---...` (~line 145) down to just before the literal line `stage_echo 0 preflight` (~line 190). The helpers reference caller-defined variables `SSH_PORT`, `SSH_USER`, `SSH_LIVE_PASSWORD`, `HAVE_SSHPASS` — your script must define all four BEFORE eval'ing the extracted block.
- `scripts/vm-validate.sh` cannot be sourced whole: it parses args and `die`s on a missing `--iso` at top level. That is WHY extraction-eval (Step 3) is the sourcing mechanism.
- QEMU user-mode networking: the guest reaches host-loopback services at `10.0.2.2`. Services bind `127.0.0.1` on the host; the guest-side enrollment URL is `https://10.0.2.2:7444`, seeds URL `http://10.0.2.2:25000`, boot files `http://10.0.2.2:8081`.
- CockroachDB is a HOST DEPENDENCY, not vendored: if `command -v cockroach` fails, the script must print a loud multi-line error naming the install source and exit with code 3 — it must NOT print `GATE: PASS` and must NOT silently exit 0 (spec M5: a skipped gate is not a passed gate).
- Spec anchors: milestone list under `## Milestones` (M5), test row "VM e2e (extended vm-validate)" under `## Testing`, machine-plane parity under section C3, enrollment plane per spec C6/Decision 25 context. Cite file: `docs/specs/constellation-design.md`.
- `docs/vm-validation.md` exists with header guid `bd881ea8-3d72-4911-8eb1-ae5560cc7b97` at version 1.0.0 — you EDIT it: bump version to 1.1.0, `last-edited: 2026-07-10`, KEEP that guid.

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

(For THIS task, authoring-time validation is `bash -n` + shellcheck + the cargo gate. The full VM run is the script's *runtime* job — Linux-host-only, operator-run after merge, exactly like TASK-01.)

- **Re-verify these anchors before editing** — line numbers drift; zero hits at both
  old and mapped path = STOP and report:
  ```bash
  grep -n "set -euo pipefail" scripts/vm-validate.sh
  # expect: 1 hit (line ~43) — proves the existing harness is intact and unmodified
  grep -n '==== VERIFY-ON-VM REPORT ====' scripts/vm-validate.sh
  # expect: 1 hit (~118) — the report banner you mirror
  grep -n "GATE: PASS" scripts/vm-validate.sh
  # expect: 1 hit (~128)
  grep -n "ssh_run() {" scripts/vm-validate.sh
  # expect: 1 hit (~151)
  grep -n "SSH/SCP helpers" scripts/vm-validate.sh
  # expect: 1 hit (~145) — extraction start marker
  grep -n "^stage_echo 0 preflight" scripts/vm-validate.sh
  # expect: 1 hit (~190) — extraction end marker
  grep -n "/api/approve" scripts/autoinstall-agent.py
  # expect: 2+ hits (~13, ~332) — approve-endpoint ground truth
  grep -n "guid: bd881ea8-3d72-4911-8eb1-ae5560cc7b97" docs/vm-validation.md
  # expect: 1 hit — the guid you must KEEP
  ls scripts/vm-validate-constellation.sh 2>/dev/null | wc -l
  # expect: 0 — the new script does not exist yet
  ```

## Step-by-step

1. Run the ⛔ START HERE block, then every anchor grep above. Any zero-hit grep → STOP and report.

2. **Create `scripts/vm-validate-constellation.sh`** with a fresh 4-line header (`# file: scripts/vm-validate-constellation.sh`, `# version: 1.0.0`, a NEW guid from `uuidgen | tr 'A-F' 'a-f'`, `# last-edited: 2026-07-10`) after `#!/usr/bin/env bash`, then `set -euo pipefail`. Comment banner states: "Constellation e2e VM gate (spec M5): loopback uaa-control/uaa-web/uaa-pxe + single-node CockroachDB + temp CA; enroll→approve→cert→install→verify inside QEMU+swtpm. Sources helpers from scripts/vm-validate.sh — NEVER edits it."

3. **Helper extraction (the sourcing mechanism).** Immediately after arg parsing:
   ```bash
   VV="${REPO_ROOT}/scripts/vm-validate.sh"
   HELPERS="$(sed -n '/SSH\/SCP helpers/,/^stage_echo 0 preflight/p' "$VV" | sed '$d')"
   [ -n "$HELPERS" ] || { echo "ERROR: helper extraction from vm-validate.sh came back empty" >&2; exit 1; }
   eval "$(grep -m1 '^die()' "$VV")"
   eval "$(grep -m1 '^stage_echo()' "$VV")"
   eval "$HELPERS"
   ```
   Define `SSH_PORT`, `SSH_USER`, `SSH_LIVE_PASSWORD`, `HAVE_SSHPASS` (same defaults as vm-validate.sh: `10022`, `ubuntu-server`, `default`, detected via `command -v sshpass`) BEFORE the `eval` lines. The empty-extraction check is mandatory — a drifted marker must fail loudly, never proceed with missing functions.

4. **Flags** (mirror vm-validate.sh's `--flag value` loop): `--iso`, `--agent`, `--config` (default `examples/configs/install/vm-test.yaml`), `--workdir` (default `./vm-validate-constellation-work`), `--disk-size`, `--ssh-port`, `--boot-timeout`, `--install-timeout`, plus new `--control-bin`, `--web-bin`, `--pxe-bin` (defaults `target/release/uaa-control|uaa-web|uaa-pxe`), and **`--preflight-only`** (run stages 0–1 then exit 0 printing `PREFLIGHT: OK` — this is the testability seam for the skip-guard below).

5. **Stage 0 — preflight.** Check `qemu-system-x86_64`, `swtpm`, `qemu-img`, `ssh`. Then the cockroach guard:
   ```bash
   if ! command -v cockroach >/dev/null 2>&1; then
     echo "ERROR: cockroach not found on PATH." >&2
     echo "The constellation gate REQUIRES CockroachDB (documented host dependency)." >&2
     echo "Install: https://www.cockroachlabs.com/docs/stable/install-cockroachdb" >&2
     echo "GATE NOT RUN — this is a SKIP-with-error, not a pass." >&2
     exit 3
   fi
   ```
   Exit code 3 is reserved for this skip; it must be unreachable when cockroach IS present (anti-over-suppression check in acceptance). Also verify the three service binaries exist and are executable.

6. **Stage 1 — control plane up.** All state under `$WORKDIR`: start `cockroach start-single-node --insecure --listen-addr=127.0.0.1:26257 --http-addr=127.0.0.1:0 --store="$WORKDIR/crdb"` in the background (record pid). Generate the temp CA in `$WORKDIR/ca` using the uaa-control CA bootstrap (PK-01/PK-03 `ca issue-service` server-local path) — never read or write `/var/lib/uaa`, `/etc/uaa`, or any path outside `$WORKDIR`. Launch `uaa-control` (machine plane 127.0.0.1:25000, enrollment 127.0.0.1:7444, operator 127.0.0.1:8443), `uaa-web` (127.0.0.1:8081, webroot `$WORKDIR/webroot`), and `uaa-pxe` (workdir-scoped `dhcp-hostsdir`/`dhcp-optsdir` under `$WORKDIR/pxe`, reload disabled — no live dnsmasq on the gate host), each with pid recorded and stdout/stderr to `$WORKDIR/logs/`. Poll each health endpoint with `curl -fsS` until up or a 60s timeout (`fail_stage`-style on timeout). If `--preflight-only`: tear down, print `PREFLIGHT: OK`, exit 0.

7. **Stage 2 — boot VM.** Reuse the TASK-01 QEMU+swtpm invocation shape (OVMF, `-drive if=virtio` qcow2 scratch disk in `$WORKDIR`, swtpm socket, `-netdev user,...,hostfwd=tcp:127.0.0.1:${SSH_PORT}-:22`), booting `--iso`. `wait_for_ssh "$SSH_USER"` for the live session.

8. **Stage 3 — enroll.** Copy `--agent` into the guest via `scp_run`; run `uaa enroll --server https://10.0.2.2:7444 --ca "$WORKDIR/ca/install-ca.crt"` (guest reaches host loopback at 10.0.2.2 — copy the CA cert into the guest first). Confirm the CSR landed: poll the operator/enrollment API from the HOST (`curl -fsS` against 127.0.0.1) until the pending CSR appears.

9. **Stage 4 — approve via API.** Host-side: approve the machine via `POST http://127.0.0.1:25000/api/approve/<mac>` (parity endpoint) and approve/sign the CSR via the PK-01 enrollment approve API. Then poll from the guest side until `uaa enroll` reports the cert persisted (PK-02 resume loop) — assert the issued cert file exists in the guest.

10. **Stage 5 — install + verify.** Drive the install exactly as vm-validate.sh stage 4 does (agent invocation with `--config`, install timeout), reboot to disk, unlock, then run the verify sweep (the 19-check `verify_host` path via the agent CLI) and capture per-check results. Track `FIRST_FAILING_STAGE` on any failure via a local `fail_stage`-equivalent that prints the constellation report and exits 1.

11. **Stage 6 — report.** Print, mirroring the existing format byte-conventions:
    ```
    ==== VERIFY-ON-VM REPORT ====
    constellation: enroll=<PASS|FAIL> approve=<PASS|FAIL> cert=<PASS|FAIL> install=<PASS|FAIL> verify=<PASS|FAIL>
    GATE: PASS            (or: GATE: FAIL (<first failing stage>))
    =============================
    ```
    `GATE: PASS` is printed ONLY when every lifecycle step passed. `tee -a "$WORKDIR/logs/report.log"`.

12. **Cleanup trap.** Mirror vm-validate.sh's cleanup discipline verbatim in spirit: kill ONLY pids this script started (qemu, swtpm, cockroach, uaa-control, uaa-web, uaa-pxe, readers) — never `pkill` by name.

13. **Edit `docs/vm-validation.md`**: bump header to `version: 1.1.0`, `last-edited: 2026-07-10`, KEEP guid `bd881ea8-3d72-4911-8eb1-ae5560cc7b97`. Append a section `## Constellation gate (scripts/vm-validate-constellation.sh)` covering: what it adds over the base gate, the cockroach host dependency + exit-3 skip semantics, the 10.0.2.2 guest→host URL rule, invocation example, and the line "**THIS SCRIPT PASSING IS THE M5 GATE — no hardware attempt before it passes.**"

14. `chmod +x scripts/vm-validate-constellation.sh`; run `bash -n` and (if installed) `shellcheck` on it.

15. Bump the file header (version + last-edited) on every file you touch; new files get a fresh 4-line header with a new uuid4 (`uuidgen | tr 'A-F' 'a-f'`).

## How to test

```bash
cargo test --lib --offline && cargo build --offline
# Expected: all tests pass (baseline 311; this task adds no Rust code so the count is unchanged), build clean
cargo clippy --offline -- -D warnings
# Expected: no warnings (no Rust touched)
bash -n scripts/vm-validate-constellation.sh
# Expected: exit 0, no output
shellcheck scripts/vm-validate-constellation.sh
# Expected: no errors (or note "shellcheck not installed" and say so in your report)
PATH="$(mktemp -d)" /bin/bash scripts/vm-validate-constellation.sh --iso /dev/null --agent /dev/null; echo "exit=$?"
# Expected: loud multi-line cockroach error on stderr, "GATE NOT RUN" line, exit=3 (or an earlier preflight die if qemu is also missing on this box — the cockroach branch itself must be code-reviewable as exit 3)
git diff origin/main -- scripts/vm-validate.sh | wc -l
# Expected: 0 — the existing gate script is byte-identical
grep -n "GATE: PASS" scripts/vm-validate-constellation.sh
# Expected: 1+ hits — report parity
```

The full VM run (`sudo ./scripts/vm-validate-constellation.sh --iso ... --agent ...` on a Linux host with KVM + cockroach) is the operator's post-merge runtime test — NOT part of the authoring gate, exactly like TASK-01.

## Acceptance criteria

- [ ] Script exists and is executable: `test -x scripts/vm-validate-constellation.sh && echo OK` → `OK`.
- [ ] Helpers are SOURCED, not copied: `grep -n 'vm-validate.sh' scripts/vm-validate-constellation.sh` → ≥1 hit (extraction path), and `grep -c "sshpass -p" scripts/vm-validate-constellation.sh` → 0 (no pasted helper bodies).
- [ ] `scripts/vm-validate.sh` untouched: `git diff origin/main -- scripts/vm-validate.sh | wc -l` → 0.
- [ ] Cockroach skip is loud and distinct: `grep -n "GATE NOT RUN" scripts/vm-validate-constellation.sh` → 1 hit and `grep -n "exit 3" scripts/vm-validate-constellation.sh` → 1+ hits.
- [ ] Report parity: `grep -n '==== VERIFY-ON-VM REPORT ====' scripts/vm-validate-constellation.sh` → 1 hit; `grep -n 'GATE: FAIL' scripts/vm-validate-constellation.sh` → 1+ hits.
- [ ] Lifecycle walked in order: `grep -n "enroll\|approve\|cert\|install\|verify" scripts/vm-validate-constellation.sh | head -20` shows stages in enroll→approve→cert→install→verify order, and approval uses `/api/approve/` (`grep -n "/api/approve/" scripts/vm-validate-constellation.sh` → 1+ hits).
- [ ] All state workdir-scoped: `grep -n "/var/lib/uaa\|/etc/uaa" scripts/vm-validate-constellation.sh` → 0 hits.
- [ ] Anti-over-suppression: the cockroach guard does not block the happy path — with a stub on PATH (`D=$(mktemp -d); printf '#!/bin/sh\nexit 0\n' > "$D/cockroach"; chmod +x "$D/cockroach"; PATH="$D:$PATH" bash -n scripts/vm-validate-constellation.sh` plus a code-review check that the guard is `if ! command -v cockroach` and `--preflight-only` reaches `PREFLIGHT: OK` past the guard: `grep -n "PREFLIGHT: OK" scripts/vm-validate-constellation.sh` → 1 hit AFTER the guard's line number).
- [ ] `docs/vm-validation.md` updated: `grep -n "vm-validate-constellation.sh" docs/vm-validation.md` → 1+ hits; `grep -n "guid: bd881ea8-3d72-4911-8eb1-ae5560cc7b97" docs/vm-validation.md` → 1 hit (guid KEPT); `grep -n "version: 1.1.0" docs/vm-validation.md` → 1 hit.
- [ ] `cargo test --lib --offline && cargo build --offline` exits 0; clippy clean.
- [ ] File headers bumped on every changed file (`git diff origin/main --stat` files each show a version bump; guids unchanged; the new script has a fresh header).

## Commit message

```
feat(testing): add constellation e2e VM gate script vm-validate-constellation.sh (ws10-gates)

New scripts/vm-validate-constellation.sh (spec M5 — THE gate before any
hardware): launches uaa-control/uaa-web/uaa-pxe on loopback with a temp
workdir CA and cockroach start-single-node --insecure (host dep; loud
exit-3 skip if absent), boots the agent VM via QEMU+swtpm, and walks
enroll -> approve (API) -> cert -> install -> 19-check verify. Sources
helpers from scripts/vm-validate.sh by extraction (that script is byte-
identical) and mirrors its greppable ==== VERIFY-ON-VM REPORT ==== /
GATE: PASS|FAIL format. docs/vm-validation.md gains the constellation
section (guid kept, version bumped).

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

Additive. If `grep -n "GATE NOT RUN" scripts/vm-validate-constellation.sh` hits AND `grep -n "vm-validate-constellation.sh" docs/vm-validation.md` hits, the task is already applied — run the Acceptance criteria checks instead of re-applying. Rollback = revert the single commit: it removes the new script and the appended docs section cleanly; `scripts/vm-validate.sh`, every Rust crate, and all other harness files stay untouched (the base gate never depended on the new script).
