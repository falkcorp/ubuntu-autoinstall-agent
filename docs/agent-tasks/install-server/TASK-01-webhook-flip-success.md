<!-- file: docs/agent-tasks/install-server/TASK-01-webhook-flip-success.md -->
<!-- version: 1.0.0 -->
<!-- guid: 139f284d-e8ab-4ece-a902-5565618c310e -->
<!-- last-edited: 2026-07-09 -->

# TASK-01 — Auto-flip on USB/SSH install success: widen webhook flip statuses to include `success` + tolerate missing iPXE file (todo:usb-report-flip)

**Priority:** P1 · **Effort:** S · **Recommended subagent:** Haiku-class · python-services subagent · **Why:** one-tuple widening + a small guarded decision helper in the py mirror; the status-vocabulary mismatch is scout-confirmed, not hypothetical · **Depends on:** none

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/install-server-webhook-flip-success" -b agent/install-server-webhook-flip-success origin/main
cd "$REPO/.worktrees/install-server-webhook-flip-success"
git rebase origin/main
```

(Protocol is also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

## Goal

In `scripts/autoinstall-agent.py`'s `/api/webhook` POST handler, widen the auto-flip condition from `status in ("finished", "complete")` to also accept `"success"`, gated so only FINAL results flip (never per-phase `running` posts), and tolerate hosts that have no `mac-<hexmac>.ipxe` file (USB-only installs) without erroring the webhook response. Implements design C1 / LOCKED Decision 2 of `docs/specs/install-server-design.md` — the server side widens; the Rust status vocabulary is NOT touched. REUSE the existing `flip_ipxe(hostname, target="boot-local-disk")` helper and the existing `log()`/`log_event()`/`send_json()` idioms — do NOT write a second flip path, do NOT add new registries or state.

## Background (verify before editing)

- Scout-confirmed vocabulary mismatch: the Rust installer (`src/network/ssh_installer/status.rs` `post_status`, driven by `Installer::report` when `--report-url` is set via `set_report_url`) posts `status` values **`running`** (start + per-phase), **`failed`**, and **`success`** (final, `progress=100`) — **never** `finished`/`complete`. The webhook auto-flip fires only on `("finished", "complete")` (cloud-init's `reporting.sh` vocabulary), so a successful uaa SSH/USB install never flips its host to `boot-local-disk` today.
- Rust payload fields (from `post_status`): `source_ip, timestamp, origin="uaa-ssh-installer", description, name` (hostname), `result, event_type="status_update", status, progress, message, files:[]`.
- `flip_ipxe` returns `(False, "No iPXE file found for <hostname>")` when the host has no `mac-<hexmac>.ipxe` file — the case for USB-only hosts that never netboot-registered. That outcome must be logged and swallowed, never turned into a non-200 webhook response. (`find_ipxe_file_by_hostname` can also raise, e.g. if `IPXE_BOOT_DIR` is unreadable — wrap the call.)
- The current handler already returns `send_json(200, {"ok": True})` unconditionally after the flip attempt — PRESERVE that.
- **HARD RULES in force:** `scripts/autoinstall-agent.py` is a REPO MIRROR — never touch 172.16.2.30; the deploy is a documented HUMAN step (`scp` + `sudo systemctl restart autoinstall-agent`), you only record it. No secrets enter git. Stay in your worktree; NEVER push/PR/merge — the coordinator owns all git.

**Re-verify these anchors** — line numbers drift, they are a starting point only. Zero hits = STOP and report:

```bash
grep -n 'def do_POST' scripts/autoinstall-agent.py                     # expect: 1 hit ~line 421
grep -n 'status in ("finished", "complete")' scripts/autoinstall-agent.py   # expect: 1 hit ~line 489
grep -n 'def flip_ipxe' scripts/autoinstall-agent.py                   # expect: 1 hit ~line 101
grep -n '"success", 100' src/network/ssh_installer/installer.rs        # expect: 1 hit ~line 235
grep -n 'pub async fn post_status' src/network/ssh_installer/status.rs # expect: 1 hit ~line 23
grep -n 'fn set_report_url' src/network/ssh_installer/installer.rs     # expect: 1 hit ~line 47
```

## Step-by-step

1. Run the anchor greps above. The Rust files are READ-ONLY context (they prove the vocabulary); the ONLY file you edit is `scripts/autoinstall-agent.py`.
2. Add a module-level pure decision helper **immediately above `def flip_ipxe`** (re-find it: `grep -n 'def flip_ipxe' scripts/autoinstall-agent.py`). It must use only `data.get(...)` and builtins (a test extracts and `exec`s exactly this function), and must keep the literal widened tuple on one line so the acceptance grep hits:

   ```python
   def webhook_should_flip(data):
       """True only for FINAL successful-install webhook payloads.

       cloud-init reporting.sh posts status "finished"/"complete" (always final).
       The Rust uaa installer posts event_type "status_update" with status
       "running" (start + per-phase), "failed", and "success" (final at
       progress 100) — a status_update may flip only on a final result.
       """
       status = data.get("status", "")
       name = data.get("name", "")
       if name and status in ("finished", "complete", "success"):
           if data.get("event_type") == "status_update":
               return status == "success" or data.get("progress") == 100
           return True
       return False
   ```

   Edge semantics (spelled out — do not guess): missing/empty `name` → False (nothing to flip, not an error); `status` = `running`/`failed`/anything else → False; missing `event_type` (cloud-init path) with a tuple status → True; `event_type == "status_update"` with a tuple status → True only when `status == "success"` or `progress == 100`; missing `progress` on a `status_update` whose status is `success` → still True.
3. In the `/api/webhook` block inside `do_POST`, replace the line `if status in ("finished", "complete") and name:` with `if webhook_should_flip(data):` and wrap the flip call so no flip failure can escape:

   ```python
           if webhook_should_flip(data):
               try:
                   ok, msg = flip_ipxe(name)
               except Exception as e:
                   ok, msg = False, f"flip failed: {e}"
               # Missing mac-<hexmac>.ipxe (USB-only host) or any flip error is
               # logged and swallowed — the webhook itself still succeeded.
               log(f"WEBHOOK {name} status={status} -> auto-flip: {msg}")
           else:
               log(f"WEBHOOK {name} event_type={data.get('event_type')} status={status}")
   ```

   Purely additive otherwise: do NOT change `log_event(data)`, the files-saving loop, or the final `self.send_json(200, {"ok": True})` — the response stays 200 `{"ok": True}` whether the flip succeeded, found no iPXE file, or raised.
4. Bump the mirror's file header: `# version: 1.2.0` → next minor from whatever is current after your rebase (e.g. `1.3.0`), `# last-edited: 2026-07-09`; keep the guid.
5. Record (do NOT execute — HUMAN step) the deploy note; it is already in the module docstring and in `docs/agent-tasks/install-server/orchestration.md`:

   ```bash
   # HUMAN deploy (documentation only — never run by an agent):
   scp scripts/autoinstall-agent.py 172.16.2.30:/var/www/html/cloud-init/scripts/autoinstall-agent.py
   ssh 172.16.2.30 'sudo systemctl restart autoinstall-agent'
   ```

## How to test

```bash
python3 -m py_compile scripts/autoinstall-agent.py
# Expected: exit 0, no output

python3 - <<'PY'
src = open("scripts/autoinstall-agent.py").read()
start = src.index("def webhook_should_flip")
end = src.index("\ndef ", start + 1)
ns = {}
exec(src[start:end], ns)
f = ns["webhook_should_flip"]
assert f({"status": "success", "name": "h", "event_type": "status_update", "progress": 100})   # uaa final success flips
assert f({"status": "success", "name": "h", "event_type": "status_update"})                    # success is final even without progress
assert not f({"status": "running", "name": "h", "event_type": "status_update", "progress": 50})  # per-phase post never flips
assert not f({"status": "failed", "name": "h", "event_type": "status_update", "progress": 40})   # failure never flips
assert f({"status": "finished", "name": "h"})   # cloud-init path unchanged (anti-over-suppression)
assert f({"status": "complete", "name": "h"})   # cloud-init path unchanged
assert not f({"status": "success", "name": ""}) # no hostname -> no flip, no error
print("webhook_should_flip: 7 decision assertions passed")
PY
# Expected: webhook_should_flip: 7 decision assertions passed

cargo test --lib --offline
# Expected: 237+ passed; 0 failed (untouched Rust stays green)
cargo build --offline
# Expected: exit 0
```

## Acceptance criteria

- [ ] Flip tuple widened: `grep -n 'status in ("finished", "complete", "success")' scripts/autoinstall-agent.py` → 1 hit; `grep -n 'status in ("finished", "complete")' scripts/autoinstall-agent.py` → 0 hits (grep exits 1).
- [ ] Decision helper present and pure: the `python3` heredoc test above prints `webhook_should_flip: 7 decision assertions passed`.
- [ ] Anti-over-suppression: the `finished`/`complete` (cloud-init) assertions in that test pass AND `grep -n 'ok, msg = flip_ipxe(name)' scripts/autoinstall-agent.py` → 1 hit — the tolerance wrapper still reaches the real flip on the happy path.
- [ ] Flip failure tolerated: `grep -n 'flip failed:' scripts/autoinstall-agent.py` → 1 hit (the `except Exception` arm), and `grep -n 'self.send_json(200, {"ok": True})' scripts/autoinstall-agent.py` still hits inside the webhook block (response unchanged).
- [ ] Only `scripts/autoinstall-agent.py` changed: `git diff --name-only origin/main` lists exactly that file.
- [ ] Tests green: `python3 -m py_compile scripts/autoinstall-agent.py` exits 0; `cargo test --lib --offline` shows 237+ passed, 0 failed; `cargo build --offline` exits 0.
- [ ] File header actually bumped BY THIS TASK (the file already carries today's date at HEAD, so a date grep is vacuous): `git diff origin/main -- scripts/autoinstall-agent.py | grep -c '^+# version:'` → 1 (this diff touched the version line) AND `git diff origin/main -- scripts/autoinstall-agent.py | grep -c '^[+-]# guid:'` → 0 (guid untouched).

## Commit message

```
fix(install-server): auto-flip iPXE on uaa install success; tolerate missing iPXE file

Widen the /api/webhook auto-flip tuple to ("finished", "complete", "success") via a
webhook_should_flip helper: the Rust installer posts running/failed/success (never
finished/complete), so successful installs never flipped. status_update events flip
only on final results (success or progress==100); a missing mac-<hexmac>.ipxe
(USB-only host) or flip exception is logged and swallowed — webhook stays 200.
Mirror only; human deploys via scp + systemctl restart autoinstall-agent.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

Idempotency (additive): `grep -n 'def webhook_should_flip' scripts/autoinstall-agent.py && grep -n 'status in ("finished", "complete", "success")' scripts/autoinstall-agent.py` — if both hit, the change is already applied; run the acceptance checks instead of re-applying. Rollback: `git revert` the single commit — the flip returns to firing only on `finished`/`complete` (today's never-flips-on-success behavior); no state, no data, and no deployed server is affected until a human re-deploys the mirror; siblings unaffected.
