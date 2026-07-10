<!-- file: docs/agent-tasks/install-server/TASK-05-status-dashboard.md -->
<!-- version: 1.0.0 -->
<!-- guid: db9fb3e2-b681-464e-92e3-d782b5262e60 -->
<!-- last-edited: 2026-07-09 -->

# TASK-05 — GET /dashboard: single-page HTML status view (registry, last events, placed configs, agent binary presence) (todo:install-server-extras)

**Priority:** P3 · **Effort:** M · **Recommended subagent:** Sonnet-class · python-services subagent · **Why:** display-only aggregation of existing registries with mandatory HTML-escaping and helper reuse across three prior tasks' functions — composition, not new logic · **Depends on:** TASK-03 (same-file serialization on `scripts/autoinstall-agent.py` + reuses TASK-02's `agent_binary_status` and TASK-03's `collect_uaa_configs` — wave 4, start only after TASK-03's PR merges)

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/install-server-status-dashboard" -b agent/install-server-status-dashboard origin/main
cd "$REPO/.worktrees/install-server-status-dashboard"
git rebase origin/main
```

(Protocol is also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

## Goal

Add a read-only `GET /dashboard` route to `scripts/autoinstall-agent.py` rendering one self-contained HTML page (no external assets, no JavaScript, inline `<style>` only) that aggregates: the machine registry, the last 20 webhook events, the placed-config inventory, and agent-binary presence. Display-only — no forms, no POST, no state mutation, no links that mutate state (design C5 / LOCKED Decision 3 of `docs/specs/install-server-design.md`). REUSE, do not duplicate: `load_registry()`, the `/api/events` tail idiom over `EVENTS_LOG`, `collect_uaa_configs(CLOUD_INIT_BASE, load_registry())` from TASK-03, and `agent_binary_status(UAA_BINARY_PATH)` from TASK-02. Every interpolated value passes through `html.escape`.

## Background (verify before editing)

- The events feed idiom to copy is the `/api/events` block in `do_GET`: `open(EVENTS_LOG).readlines()[-50:]` parsed with `json.loads`, the whole thing in `try/except` falling back to `[]`. The dashboard uses the same shape with `[-20:]`.
- Registry entries (per MAC key) carry `hostname, mac, ip, type, status, registered_at, tpm_ek`, plus `last_seen`/`last_ip` after checkins — some keys may be absent; render missing values as empty strings via `.get(...)`, never `KeyError`.
- The module imports `json, os, re, time, base64, hashlib, secrets` on one line — `html` is NOT imported yet; you must add it.
- Existing raw (non-JSON) response idiom to copy for the HTML response: the `/autoinstall/uaa-config` block (`send_response(200)` + explicit `Content-Type` + `Content-Length` + `wfile.write`).
- Placed configs may exist for hosts installed with real secrets — the inventory dicts from `collect_uaa_configs` are metadata-only by construction; render ONLY those four keys and never add a "view config" link or content fetch.
- **HARD RULES in force:** `scripts/autoinstall-agent.py` is a REPO MIRROR — never touch 172.16.2.30; the deploy is a documented HUMAN step (`scp` + `sudo systemctl restart autoinstall-agent`), you only record it. No secrets enter git and nothing on this page may expose placed-config contents. Stay in your worktree; NEVER push/PR/merge — the coordinator owns all git.

**Re-verify these anchors** — line numbers drift, they are a starting point only. Zero hits = STOP and report:

```bash
grep -n 'def do_GET' scripts/autoinstall-agent.py                    # expect: 1 hit ~line 180
grep -n 'REGISTRY_FILE = ' scripts/autoinstall-agent.py              # expect: 3 hits ~lines 36-38
# reuse targets (cite-by-symbol; the first two landed in waves 2 and 3 — if either
# is missing you are in the wrong wave: STOP and report):
grep -n 'def agent_binary_status' scripts/autoinstall-agent.py       # DEPENDENCY-GATED: 1 hit only AFTER TASK-02 merges (0 at planning-time HEAD is expected); 0 at dispatch = STOP, wrong wave
grep -n 'def collect_uaa_configs' scripts/autoinstall-agent.py       # DEPENDENCY-GATED: 1 hit only AFTER TASK-03 merges (0 at planning-time HEAD is expected); 0 at dispatch = STOP, wrong wave
grep -n 'if path == "/api/events"' scripts/autoinstall-agent.py      # expect: 1 hit (tail idiom to copy)
grep -n 'if path == "/autoinstall/uaa-config"' scripts/autoinstall-agent.py   # expect: 1 hit (raw-response idiom to copy)
grep -n 'def webhook_should_flip' scripts/autoinstall-agent.py       # DEPENDENCY-GATED: 1 hit only AFTER TASK-01 merges (0 at planning-time HEAD is expected); 0 at dispatch = STOP, wrong wave (render-fn placement anchor)
```

## Step-by-step

1. Run the anchor greps. The ONLY file you edit is `scripts/autoinstall-agent.py`.
2. Add `html` to the module's first import line: `import json, os, re, time, base64, hashlib, secrets` → `import html, json, os, re, time, base64, hashlib, secrets` (re-find: `grep -n 'import json, os, re' scripts/autoinstall-agent.py`).
3. Add a module-level pure render function **immediately above `def webhook_should_flip`** (anchor grep above). It must take all data as parameters and use ONLY the `html` module and builtins (a test extracts and `exec`s exactly this function — no `load_registry`, no `os`, no file I/O inside it):

   ```python
   def render_dashboard(registry, events, configs, binary):
       """Single-page, display-only status HTML. No external assets, no <script>,
       no forms — every interpolated value goes through esc()."""
       def esc(v):
           return html.escape("" if v is None else str(v), quote=True)
       out = []
       out.append("<!DOCTYPE html><html><head><meta charset='utf-8'>")
       out.append("<title>autoinstall-agent status</title><style>")
       out.append("body{font-family:sans-serif;margin:2em}table{border-collapse:collapse;margin-bottom:2em}")
       out.append("th,td{border:1px solid #999;padding:4px 8px;text-align:left}th{background:#eee}")
       out.append("</style></head><body><h1>autoinstall-agent — install server status</h1>")
       # agent binary
       out.append("<h2>Agent binary</h2><table><tr><th>path</th><th>present</th><th>size</th><th>mtime</th></tr>")
       out.append("<tr><td>%s</td><td>%s</td><td>%s</td><td>%s</td></tr></table>" % (
           esc(binary.get("path")), esc(binary.get("present")),
           esc(binary.get("size")), esc(binary.get("mtime"))))
       # registry
       out.append("<h2>Registry</h2><table><tr><th>hostname</th><th>mac</th><th>status</th><th>last_seen</th><th>last_ip</th></tr>")
       for mac, e in sorted(registry.items()):
           out.append("<tr><td>%s</td><td>%s</td><td>%s</td><td>%s</td><td>%s</td></tr>" % (
               esc(e.get("hostname")), esc(e.get("mac", mac)), esc(e.get("status")),
               esc(e.get("last_seen")), esc(e.get("last_ip"))))
       out.append("</table>")
       # placed configs (METADATA ONLY — never contents, never links to contents)
       out.append("<h2>Placed configs</h2><table><tr><th>hexmac</th><th>hostname</th><th>mtime</th><th>ready</th></tr>")
       for c in configs:
           out.append("<tr><td>%s</td><td>%s</td><td>%s</td><td>%s</td></tr>" % (
               esc(c.get("hexmac")), esc(c.get("hostname")), esc(c.get("mtime")),
               "yes" if c.get("placeholder_free") else "PLACEHOLDER"))
       out.append("</table>")
       # last events
       out.append("<h2>Last %d events</h2><table><tr><th>received_at</th><th>name</th><th>event_type</th><th>status</th><th>progress</th><th>message</th></tr>" % len(events))
       for ev in events:
           out.append("<tr><td>%s</td><td>%s</td><td>%s</td><td>%s</td><td>%s</td><td>%s</td></tr>" % (
               esc(ev.get("received_at")), esc(ev.get("name")), esc(ev.get("event_type")),
               esc(ev.get("status")), esc(ev.get("progress")), esc(ev.get("message"))))
       out.append("</table></body></html>")
       return "".join(out)
   ```

   Edge semantics (do not guess): empty registry/configs/events → tables render with header rows only (an empty fleet is a valid page, not an error); `None`/missing values → empty cells via `esc(None)` = `""`; `placeholder_free` False → the literal text `PLACEHOLDER` (a placed-but-unfilled config must be visible, not hidden).
4. In `do_GET`, insert the route **immediately after the `/api/uaa-configs` block** (re-find: `grep -n 'if path == "/api/uaa-configs"' scripts/autoinstall-agent.py` — DEPENDENCY-GATED like the anchors above: exists only after TASK-03 merges; 0 hits at dispatch = STOP, wrong wave), copying the `/api/events` tail idiom and the raw-response idiom named above:

   ```python
           # ── Status dashboard (display-only; no forms, no mutation) ──
           if path == "/dashboard":
               try:
                   lines = open(EVENTS_LOG).readlines()[-20:]
                   events = [json.loads(l) for l in lines]
               except Exception:
                   events = []
               reg = load_registry()
               body = render_dashboard(
                   reg, events,
                   collect_uaa_configs(CLOUD_INIT_BASE, reg),
                   agent_binary_status(UAA_BINARY_PATH),
               ).encode()
               self.send_response(200)
               self.send_header("Content-Type", "text/html; charset=utf-8")
               self.send_header("Content-Length", len(body))
               self.end_headers()
               self.wfile.write(body)
               return
   ```

   Purely additive: do not modify any existing route, helper, or the `/api/events` block you copied from; call the existing collection/stat helpers — do NOT re-scan directories or re-stat the binary inline.
5. Bump the mirror's file header: version minor-bump from its current post-rebase value, `last-edited: 2026-07-09`, guid unchanged.
6. Record (do NOT execute — HUMAN step) the deploy note:

   ```bash
   # HUMAN deploy (documentation only — never run by an agent):
   scp scripts/autoinstall-agent.py 172.16.2.30:/var/www/html/cloud-init/scripts/autoinstall-agent.py
   ssh 172.16.2.30 'sudo systemctl restart autoinstall-agent'
   # then browse http://172.16.2.30:25000/dashboard
   ```

## How to test

```bash
python3 -m py_compile scripts/autoinstall-agent.py
# Expected: exit 0, no output

python3 - <<'PY'
import html
src = open("scripts/autoinstall-agent.py").read()
start = src.index("def render_dashboard")
end = src.index("\ndef ", start + 1)
ns = {"html": html}
exec(src[start:end], ns)
f = ns["render_dashboard"]
out = f(
    {"aa:bb:cc:dd:ee:ff": {"hostname": "<script>alert(1)</script>", "mac": "aa:bb:cc:dd:ee:ff",
                           "status": "approved", "last_seen": 1750000000}},
    [{"received_at": 1750000000, "name": "len-serv-003", "event_type": "status_update",
      "status": "success", "progress": 100, "message": "done & dusted"}],
    [{"hexmac": "6c4b90bcf7f4", "hostname": "len-serv-003",
      "mtime": "2026-07-09T00:00:00Z", "placeholder_free": True},
     {"hexmac": "aabbccddeeff", "hostname": None,
      "mtime": "2026-07-09T00:00:00Z", "placeholder_free": False}],
    {"path": "/var/www/html/uaa/uaa-amd64", "present": False, "size": None, "mtime": None},
)
assert "<script>alert(1)</script>" not in out          # hostile hostname neutralized
assert "&lt;script&gt;" in out                          # ... by html.escape
assert "<script" not in out                             # page ships zero JS
assert 'src="http' not in out and 'href="http' not in out and "<form" not in out   # self-contained, display-only
assert "6c4b90bcf7f4" in out and "PLACEHOLDER" in out   # both configs visible incl. unfilled one
assert "done &amp; dusted" in out                        # event message escaped
empty = f({}, [], [], {"path": "p", "present": False, "size": None, "mtime": None})
assert "<h1>" in empty and "Registry" in empty           # empty fleet still renders
print("render_dashboard: 7 assertion groups passed")
PY
# Expected: render_dashboard: 7 assertion groups passed

cargo test --lib --offline
# Expected: 237+ passed; 0 failed (untouched Rust stays green)
cargo build --offline
# Expected: exit 0
```

## Acceptance criteria

- [ ] Route present: `grep -n 'if path == "/dashboard"' scripts/autoinstall-agent.py` → 1 hit inside `do_GET`; `grep -n 'html.escape' scripts/autoinstall-agent.py` → ≥1 hit.
- [ ] Render function correct: the `python3` heredoc test above prints `render_dashboard: 7 assertion groups passed` (escaping, no JS, no external assets, no forms, empty-fleet rendering).
- [ ] Reuse, not re-scan: `grep -n 'collect_uaa_configs(CLOUD_INIT_BASE' scripts/autoinstall-agent.py` → ≥2 hits (TASK-03's route + this one) and `grep -n 'agent_binary_status(UAA_BINARY_PATH)' scripts/autoinstall-agent.py` → ≥2 hits (TASK-02's route + this one); `grep -c 'def collect_uaa_configs' scripts/autoinstall-agent.py` → 1 (no duplicate).
- [ ] Display-only: `grep -n 'def do_POST' scripts/autoinstall-agent.py` → still exactly 1 hit and `git diff origin/main -- scripts/autoinstall-agent.py | grep '^+' | grep -c 'do_POST'` → 0 (no new POST handling added).
- [ ] Only `scripts/autoinstall-agent.py` changed: `git diff --name-only origin/main` lists exactly that file.
- [ ] Tests green: `python3 -m py_compile scripts/autoinstall-agent.py` exits 0; `cargo test --lib --offline` shows 237+ passed, 0 failed; `cargo build --offline` exits 0.
- [ ] File header actually bumped BY THIS TASK (a date grep is vacuous — earlier same-wave tasks already stamp today's date): `git diff origin/main -- scripts/autoinstall-agent.py | grep -c '^+# version:'` → 1 AND `git diff origin/main -- scripts/autoinstall-agent.py | grep -c '^[+-]# guid:'` → 0.
- Anti-over-suppression: N/A

## Commit message

```
feat(install-server): GET /dashboard single-page status view

Display-only HTML aggregation (inline CSS, zero JS, no external assets, no forms):
registry table, last 20 events via the /api/events tail idiom, placed-config
inventory via collect_uaa_configs, and agent-binary presence via
agent_binary_status. All interpolated values pass through html.escape; config
metadata only — never file contents. Mirror only; human deploys via scp +
systemctl restart autoinstall-agent.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

Idempotency (additive): `grep -n 'def render_dashboard' scripts/autoinstall-agent.py && grep -n 'if path == "/dashboard"' scripts/autoinstall-agent.py` — if both hit, the change is already applied; run the acceptance checks instead of re-applying. Rollback: `git revert` the single commit removes the route and render function; TASK-02/03 helpers and all JSON endpoints keep working, nothing persists behind the page, and the deployed server is unaffected until a human re-deploys the mirror; siblings unaffected.
