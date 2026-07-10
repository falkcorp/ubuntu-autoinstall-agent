<!-- file: docs/agent-tasks/install-server/TASK-02-serve-agent-binary.md -->
<!-- version: 1.0.0 -->
<!-- guid: bcdded55-3267-432d-8ce5-b6d9d8649227 -->
<!-- last-edited: 2026-07-09 -->

# TASK-02 — First-class agent-binary serving: /api/health reports uaa-amd64 presence; document /var/www/html/uaa/ nginx path + deploy step (todo:usb-agent-serving)

**Priority:** P1 · **Effort:** S · **Recommended subagent:** Haiku-class · python-services subagent · **Why:** one read-only GET route mirroring the existing `/api/registry` JSON idiom + a docs section; no secret surface · **Depends on:** TASK-01 (same-file serialization on `scripts/autoinstall-agent.py` — wave 2, start only after TASK-01's PR merges)

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/install-server-serve-agent-binary" -b agent/install-server-serve-agent-binary origin/main
cd "$REPO/.worktrees/install-server-serve-agent-binary"
git rebase origin/main
```

(Protocol is also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

## Goal

Add a read-only `GET /api/health` route to `scripts/autoinstall-agent.py` returning service liveness, registry counts, and an `os.stat`-based presence report for the served agent binary `/var/www/html/uaa/uaa-amd64` — where an ABSENT file or directory is the handled default (`present: false`), never an exception. Also document the binary-serving path in `docs/netboot-autodeploy.md`. Implements design C2 / LOCKED Decision 3 (read-only) of `docs/specs/install-server-design.md`. REUSE the existing helpers exactly: `load_registry()`, `load_yk_registry()`, `load_tang_registry()` (all already return `{}` on missing/corrupt files — that IS the counts-default-to-0 behavior), `self.send_json(...)`, and the already-imported `os` and `datetime`. Do NOT re-implement registry loading, do NOT add new state files, do NOT add any POST handler.

## Background (verify before editing)

- `/var/www/html/uaa/uaa-amd64` is the canonical deploy target: `scripts/build-musl.sh` names it twice (header comment + the "Deploy (human)" hint it PRINTS — it never installs), and `installer-image/nocloud/uaa-usb-bootstrap.sh` curls it via `UAA_AGENT_URL` (default `http://172.16.2.30/uaa/uaa-amd64`, served by plain nginx on :80). Because the deploy is only a printed hint, **the `/uaa/` directory may not exist on the server at all** — the health check must `stat` and default to absent, never assume the directory.
- The agent's JSON routes live in `do_GET`; the simplest reference idiom is the `/api/registry` block (`self.send_json(200, load_registry())`). Registry/state files live under `/var/log/cockroach-autoinstall/` (`REGISTRY_FILE`, `YUBIKEY_REGISTRY_FILE`, `TANG_REGISTRY_FILE`); registry entries carry `status` (`pending`|`approved`).
- `docs/netboot-autodeploy.md` documents the nginx web-root layout (`/var/www/html/{ipxe/boot/mac-<hexmac>.ipxe, ubuntu/, isos/, cloud-init/}`) and the port-25000 service endpoints — the `/uaa/` path is missing from both, which is what the docs half of this task fixes.
- **HARD RULES in force:** `scripts/autoinstall-agent.py` is a REPO MIRROR — never touch 172.16.2.30; the deploy is a documented HUMAN step (`scp` + `sudo systemctl restart autoinstall-agent`), you only record it. No secrets enter git; this endpoint returns counts and stat metadata only. Stay in your worktree; NEVER push/PR/merge — the coordinator owns all git.

**Re-verify these anchors** — line numbers drift, they are a starting point only. Zero hits = STOP and report:

```bash
grep -n 'def do_GET' scripts/autoinstall-agent.py                        # expect: 1 hit ~line 180
grep -n 'REGISTRY_FILE = ' scripts/autoinstall-agent.py                  # expect: 3 hits ~lines 36-38
grep -n '/var/www/html/uaa/uaa-amd64' scripts/build-musl.sh              # expect: 2 hits ~lines 17 and 46
grep -n 'UAA_AGENT_URL:-http' installer-image/nocloud/uaa-usb-bootstrap.sh   # expect: 1 hit ~line 33
grep -n 'ipxe/boot/mac-' docs/netboot-autodeploy.md                      # expect: 2 hits ~lines 80 and 267
# reuse targets (cite-by-symbol):
grep -n 'def load_registry\|def load_yk_registry\|def load_tang_registry' scripts/autoinstall-agent.py   # expect: 3 hits
grep -n 'if path == "/api/registry"' scripts/autoinstall-agent.py        # expect: 1 hit (route-placement anchor)
```

## Step-by-step

1. Run the anchor greps. Files to edit: `scripts/autoinstall-agent.py` and `docs/netboot-autodeploy.md` — nothing else.
2. Near the other path constants at the top of `scripts/autoinstall-agent.py` (right after the `CLOUD_INIT_BASE = ...` line — re-find: `grep -n 'CLOUD_INIT_BASE = ' scripts/autoinstall-agent.py`), add:

   ```python
   UAA_BINARY_PATH = "/var/www/html/uaa/uaa-amd64"
   ```
3. Add a module-level helper **immediately above `def load_registry`** (re-find: `grep -n 'def load_registry' scripts/autoinstall-agent.py`). It must take the path as a parameter and use only `os`, `datetime`, and builtins (a test extracts and `exec`s exactly this function):

   ```python
   def agent_binary_status(path):
       """stat the served uaa binary. ABSENT file/dir is the normal, handled
       case (build-musl.sh only PRINTS the deploy hint) — never an exception."""
       info = {"path": path, "present": False, "size": None, "mtime": None}
       try:
           st = os.stat(path)
       except OSError:
           return info
       info["present"] = True
       info["size"] = st.st_size
       info["mtime"] = datetime.utcfromtimestamp(st.st_mtime).strftime("%Y-%m-%dT%H:%M:%SZ")
       return info
   ```

   Edge semantics (do not guess): missing file, missing `/uaa/` directory, or permission error → `OSError` → `{"present": False, "size": None, "mtime": None}` with the path still reported. Never raise, never create the directory.
4. In `do_GET`, insert the route **immediately before the `if path == "/api/registry":` block** (anchor grep above), following the same `send_json` idiom:

   ```python
           # ── Health / liveness ──
           if path == "/api/health":
               reg = load_registry()
               self.send_json(200, {
                   "status": "ok",
                   "registry_hosts": len(reg),
                   "registry_approved": sum(1 for e in reg.values() if e.get("status") == "approved"),
                   "yubikeys": len(load_yk_registry()),
                   "tang_servers": len(load_tang_registry()),
                   "agent_binary": agent_binary_status(UAA_BINARY_PATH),
               })
               return
   ```

   Purely additive: do not modify any existing route, helper, or import. Missing/corrupt registry files already yield `{}` from the loaders → counts report 0 and the endpoint still returns 200 (degraded-but-honest, per the design's failure-mode table).
5. In `docs/netboot-autodeploy.md`, add a new subsection right after the `### autoinstall-agent HTTP service (port 25000 on the server)` section (re-find: `grep -n 'autoinstall-agent HTTP service' docs/netboot-autodeploy.md`), titled `### uaa agent binary serving (/var/www/html/uaa/)`, stating:
   - nginx on plain `:80` serves `/var/www/html/uaa/uaa-amd64`; `installer-image/nocloud/uaa-usb-bootstrap.sh` downloads it via `UAA_AGENT_URL` (default `http://172.16.2.30/uaa/uaa-amd64`).
   - Build: `scripts/build-musl.sh` on Linux (or the CI `musl-build.yml` artifact `uaa-amd64`).
   - Deploy is a HUMAN step: `sudo install -D -m 0755 target/x86_64-unknown-linux-musl/release/ubuntu-autoinstall-agent /var/www/html/uaa/uaa-amd64` on the server — the directory does not exist until this first run.
   - Verify: `curl -s http://172.16.2.30:25000/api/health | python3 -m json.tool` → `agent_binary.present`.
   - Add `GET /api/health` to the endpoint list in the service section.
6. Bump file headers: `scripts/autoinstall-agent.py` version minor-bump from its current post-rebase value + `last-edited: 2026-07-09` (keep guid); `docs/netboot-autodeploy.md` `1.1.0` → `1.2.0` + `last-edited: 2026-07-09` (keep guid).
7. Record (do NOT execute — HUMAN step) the mirror deploy note:

   ```bash
   # HUMAN deploy (documentation only — never run by an agent):
   scp scripts/autoinstall-agent.py 172.16.2.30:/var/www/html/cloud-init/scripts/autoinstall-agent.py
   ssh 172.16.2.30 'sudo systemctl restart autoinstall-agent'
   curl -s http://172.16.2.30:25000/api/health | python3 -m json.tool
   ```

## How to test

```bash
python3 -m py_compile scripts/autoinstall-agent.py
# Expected: exit 0, no output

python3 - <<'PY'
import os, tempfile
from datetime import datetime
src = open("scripts/autoinstall-agent.py").read()
start = src.index("def agent_binary_status")
end = src.index("\ndef ", start + 1)
ns = {"os": os, "datetime": datetime}
exec(src[start:end], ns)
f = ns["agent_binary_status"]
absent = f("/nonexistent-dir-task02/uaa-amd64")   # missing DIRECTORY, not just file
assert absent == {"path": "/nonexistent-dir-task02/uaa-amd64", "present": False, "size": None, "mtime": None}, absent
tmp = tempfile.NamedTemporaryFile(delete=False); tmp.write(b"x" * 10); tmp.close()
present = f(tmp.name)
assert present["present"] is True and present["size"] == 10, present
assert present["mtime"] and present["mtime"].endswith("Z")
os.unlink(tmp.name)
print("agent_binary_status: absent-dir + present cases passed")
PY
# Expected: agent_binary_status: absent-dir + present cases passed

cargo test --lib --offline
# Expected: 237+ passed; 0 failed (untouched Rust stays green)
cargo build --offline
# Expected: exit 0
```

## Acceptance criteria

- [ ] Route present: `grep -n 'if path == "/api/health"' scripts/autoinstall-agent.py` → 1 hit inside `do_GET`.
- [ ] Stat helper present and absent-safe: the `python3` heredoc test above prints `agent_binary_status: absent-dir + present cases passed` (absent `/uaa/` directory → `present: false`, never an exception).
- [ ] Reuse, not reinvention: `grep -c 'def load_registry' scripts/autoinstall-agent.py` → 1 (no duplicate loader added); `grep -n 'load_tang_registry()' scripts/autoinstall-agent.py` shows the health route calling the existing helper.
- [ ] Docs updated: `grep -n 'uaa-amd64' docs/netboot-autodeploy.md` → ≥2 hits and `grep -n '/api/health' docs/netboot-autodeploy.md` → ≥1 hit.
- [ ] Only the two named files changed: `git diff --name-only origin/main` lists exactly `scripts/autoinstall-agent.py` and `docs/netboot-autodeploy.md`.
- [ ] Tests green: `python3 -m py_compile scripts/autoinstall-agent.py` exits 0; `cargo test --lib --offline` shows 237+ passed, 0 failed; `cargo build --offline` exits 0.
- [ ] File headers actually bumped BY THIS TASK in both files (date greps are vacuous — the py mirror already carries today's date at HEAD): `git diff origin/main -- scripts/autoinstall-agent.py docs/netboot-autodeploy.md | grep -c '^+# version:\|^+<!-- version:'` → 2 (both version lines touched in this diff) AND `git diff origin/main -- scripts/autoinstall-agent.py docs/netboot-autodeploy.md | grep -c 'guid:'` → 0 (guids untouched).
- Anti-over-suppression: N/A

## Commit message

```
feat(install-server): GET /api/health with agent-binary presence + serving docs

Read-only liveness route: registry/yubikey/tang counts (0 when a registry file is
absent) and os.stat-based presence of /var/www/html/uaa/uaa-amd64 — absent dir is
the handled default since build-musl.sh only prints the deploy hint. Document the
nginx /uaa/ serving path, UAA_AGENT_URL bootstrap flow, and the human deploy steps
in docs/netboot-autodeploy.md. Mirror only; human deploys via scp + systemctl
restart autoinstall-agent.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

Idempotency (additive): `grep -n 'if path == "/api/health"' scripts/autoinstall-agent.py && grep -n 'def agent_binary_status' scripts/autoinstall-agent.py` — if both hit, the change is already applied; run the acceptance checks instead of re-applying. Rollback: `git revert` the single commit removes the route, the helper, and the docs section; no state or data exists behind the endpoint, and the deployed server is unaffected until a human re-deploys the mirror; siblings unaffected.
