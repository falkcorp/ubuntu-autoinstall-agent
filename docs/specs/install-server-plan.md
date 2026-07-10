<!-- file: docs/specs/install-server-plan.md -->
<!-- version: 1.0.0 -->
<!-- guid: 2f48d6b9-8010-47a2-83a1-0f39ec0d1548 -->
<!-- last-edited: 2026-07-09 -->

# Install Server Operability (install-server) — Implementation Plan

Companion to [`docs/specs/install-server-design.md`](install-server-design.md). Five tasks, mapped 1:1 to the briefs in `docs/agent-tasks/install-server/`. Decisions referenced below are **LOCKED** in the design spec — do not reopen them during execution.

**Baseline gate (every step):** `cargo test --lib --offline` (baseline **237 passed**) and `cargo build --offline`. Python-mirror steps add `python3 -m py_compile scripts/autoinstall-agent.py`; the shell step adds `bash -n scripts/deploy-usb-configs.sh`.

**Hard operational rules in force:** never touch 172.16.2.30 or len-serv-003 (all steps are code/docs only); `scripts/autoinstall-agent.py` and web-root helpers are REPO MIRRORS — a human deploys (`scp` + `sudo systemctl restart autoinstall-agent`), briefs document that step and never execute it; no real `luks_key`/`root_password`/`tpm2_pin` ever enters git; workers stay in their worktree and never push/PR/merge — the coordinator owns all git.

---

## Wave order

Waves are GLOBAL across the operation (skeleton is authoritative). Within this workstream, TASK-01/02/03/05 all edit `scripts/autoinstall-agent.py` and are therefore strictly serialized across waves; TASK-04 touches only `scripts/deploy-usb-configs.sh` and runs in parallel in wave 1.

| Global wave | install-server tasks | Runs alongside (other workstreams) |
|---|---|---|
| 1 | TASK-01 (webhook-flip-success), TASK-04 (secret-injection-placement) | installer-robustness 01/02/06/08, testing-gates 02 |
| 2 | TASK-02 (serve-agent-binary) | installer-robustness 03/04/05, testing-gates 01 |
| 3 | TASK-03 (list-configs-endpoint) | installer-robustness 07, boot-prod 01 |
| 4 | TASK-05 (status-dashboard) | phase-rerun 01 |

Dependency chain: TASK-01 → TASK-02 → TASK-03 → TASK-05 (file-collision serialization on `scripts/autoinstall-agent.py`); TASK-04 has no dependencies. Each later task rebases onto `origin/main` after the previous wave's merge.

---

## Step 1 — Widen webhook auto-flip to `success` + tolerate missing iPXE file

**Brief:** `docs/agent-tasks/install-server/TASK-01-webhook-flip-success.md` · src `todo:usb-report-flip` · P1 · S · Haiku-class · wave 1 · deps: none
**Files:** `scripts/autoinstall-agent.py`

Implements design C1 (LOCKED Decision 2). In the `/api/webhook` POST handler, widen the auto-flip condition from `status in ("finished", "complete")` to `status in ("finished", "complete", "success")` — the Rust agent (`post_status` in `src/network/ssh_installer/status.rs`) posts `running`/`failed`/`success`, never `finished`/`complete`, so today a successful install never flips. Wrap the `flip_ipxe()` call so a host with no `mac-<hexmac>.ipxe` file (USB-only install) logs the miss and still returns 200 — the tolerance wrapper must NOT swallow the happy path for hosts that do have an iPXE file (anti-over-suppression case in the brief). Reuse the existing `flip_ipxe` helper; add no second flip path. Bump the mirror's version header. Document (not execute) the human deploy step.

- Run: `python3 -m py_compile scripts/autoinstall-agent.py`
  Expected: exit 0, no output
- Run: `grep -n 'status in ("finished", "complete", "success")' scripts/autoinstall-agent.py`
  Expected: 1 hit
- Run: `cargo test --lib --offline && cargo build --offline`
  Expected: 237+ passed; 0 failed; build exit 0

## Step 2 — `deploy-usb-configs.sh --inject-from <secrets.yaml>` (server-local secret placement)

**Brief:** `docs/agent-tasks/install-server/TASK-04-secret-injection-placement.md` · src `todo:place-time-secrets` · P2 · M · Sonnet-class · wave 1 (parallel with Step 1 — disjoint files) · deps: none
**Files:** `scripts/deploy-usb-configs.sh`

Implements design C4 (LOCKED Decision 1: **NO HTTP secret-write/placement API** — rejected `POST /api/place-config` and TLS+auth-on-:25000 alternatives stay rejected). Add an optional `--inject-from <secrets.yaml>` flag: fill `REPLACE_AT_PLACE_TIME` slots in a `mktemp`'d copy (umask 077, `trap rm EXIT`), then run the copy through the EXISTING placement path so all existing gates still fire — unknown-host refusal, missing-source refusal, and the `PLACEHOLDER="REPLACE_AT_PLACE_TIME"` refusal as the backstop for unfilled slots. Never echo/log secret values; refuse a secrets file located inside the repo tree; without the flag, behavior is byte-identical. Committed example configs stay placeholder-bearing.

- Run: `bash -n scripts/deploy-usb-configs.sh`
  Expected: exit 0
- Run: `grep -n 'inject-from' scripts/deploy-usb-configs.sh`
  Expected: >=1 hit (flag parsing present)
- Run: `grep -n 'PLACEHOLDER="REPLACE_AT_PLACE_TIME"' scripts/deploy-usb-configs.sh`
  Expected: 1 hit (refusal gate preserved, not removed)
- Run: `grep -c 'REPLACE_AT_PLACE_TIME' examples/configs/install/len-serv-003.yaml`
  Expected: 4 (committed configs untouched)
- Run: `cargo test --lib --offline && cargo build --offline`
  Expected: 237+ passed; 0 failed; build exit 0

## Step 3 — `GET /api/health` + agent-binary serving docs

**Brief:** `docs/agent-tasks/install-server/TASK-02-serve-agent-binary.md` · src `todo:usb-agent-serving` · P1 · S · Haiku-class · wave 2 · deps: install-server/TASK-01 (same-file serialization)
**Files:** `scripts/autoinstall-agent.py`, `docs/netboot-autodeploy.md`

Implements design C2 (LOCKED Decision 3: read-only). New GET route returning service liveness, registry counts (0 when a registry file is absent/unreadable), and agent-binary presence for `/var/www/html/uaa/uaa-amd64` via `os.path.isfile`/`os.stat` — the `/uaa/` directory may not exist on the server (`build-musl.sh` only prints the install hint), so absence is the handled default, never an exception. Response shape per the design's Data model. Docs: add the `/var/www/html/uaa/` nginx path, the `UAA_AGENT_URL` bootstrap flow (`installer-image/nocloud/uaa-usb-bootstrap.sh` curls `http://172.16.2.30/uaa/uaa-amd64`), and the human deploy step (scp + `sudo systemctl restart autoinstall-agent` + `curl :25000/api/health` verify).

- Run: `python3 -m py_compile scripts/autoinstall-agent.py`
  Expected: exit 0
- Run: `grep -n '/api/health' scripts/autoinstall-agent.py`
  Expected: >=1 hit in the do_GET routing
- Run: `grep -n 'uaa-amd64' docs/netboot-autodeploy.md`
  Expected: >=1 hit (serving path documented)
- Run: `cargo test --lib --offline && cargo build --offline`
  Expected: 237+ passed; 0 failed; build exit 0

## Step 4 — `GET /api/uaa-configs` placed-config inventory

**Brief:** `docs/agent-tasks/install-server/TASK-03-list-configs-endpoint.md` · src `todo:install-server-extras` · P2 · S · Haiku-class · wave 3 · deps: install-server/TASK-02 (same-file serialization)
**Files:** `scripts/autoinstall-agent.py`

Implements design C3 (LOCKED Decision 3: metadata only — **never file contents**, placed `uaa.yaml` files hold real secrets). Scan `/var/www/html/cloud-init/<hexmac>/uaa.yaml`, skipping non-hexmac entries; join hostname from `registry.json` by MAC (None when unregistered); report `mtime` (ISO-8601 UTC) and `placeholder_free` (byte-scan for `REPLACE_AT_PLACE_TIME`). Missing cloud-init root → `{"configs": [], "count": 0}` with 200. Note `/autoinstall/uaa-config` (single-host fetch) ALREADY SHIPPED in PR #27 — reference its resolution idiom, do not re-plan or duplicate it. Structure the scan as a reusable collection function so Step 5's dashboard calls it instead of re-scanning.

- Run: `python3 -m py_compile scripts/autoinstall-agent.py`
  Expected: exit 0
- Run: `grep -n '/api/uaa-configs' scripts/autoinstall-agent.py`
  Expected: >=1 hit in the do_GET routing
- Run: `grep -n 'placeholder_free' scripts/autoinstall-agent.py`
  Expected: >=1 hit
- Run: `cargo test --lib --offline && cargo build --offline`
  Expected: 237+ passed; 0 failed; build exit 0

## Step 5 — `GET /dashboard` single-page status view

**Brief:** `docs/agent-tasks/install-server/TASK-05-status-dashboard.md` · src `todo:install-server-extras` · P3 · M · Sonnet-class · wave 4 · deps: install-server/TASK-03 (same-file serialization + function reuse)
**Files:** `scripts/autoinstall-agent.py`

Implements design C5 (LOCKED Decision 3: display-only). Single-page HTML, no external assets, inline CSS only: registry table, last events (reuse the `/api/events` tail idiom over `events.jsonl`), placed-config inventory (call Step 4's collection function — no duplicate scan), agent-binary presence (call Step 3's check function). Every interpolated value passes through `html.escape`. No forms, no POST, no state mutation. Document the human deploy step.

- Run: `python3 -m py_compile scripts/autoinstall-agent.py`
  Expected: exit 0
- Run: `grep -n '/dashboard' scripts/autoinstall-agent.py`
  Expected: >=1 hit in the do_GET routing
- Run: `grep -n 'html.escape' scripts/autoinstall-agent.py`
  Expected: >=1 hit (escaping in place)
- Run: `cargo test --lib --offline && cargo build --offline`
  Expected: 237+ passed; 0 failed; build exit 0

---

## Completion

Workstream is done when all five briefs' acceptance criteria pass, the flip-tuple grep shows the widened tuple, all four `scripts/autoinstall-agent.py` versions merged cleanly through waves 1→2→3→4, and the mirror deploy step is documented in each touched task. Deployment to 172.16.2.30 remains a HUMAN action outside this plan. See `docs/agent-tasks/install-server/README.md` for the task table and `docs/agent-tasks/ORCHESTRATION.md` for the coordinator/worker protocol.
