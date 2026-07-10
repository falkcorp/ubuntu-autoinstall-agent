<!-- file: docs/agent-tasks/install-server/TASK-03-list-configs-endpoint.md -->
<!-- version: 1.0.0 -->
<!-- guid: c319e163-1f41-4b2c-874f-e4fd257552a8 -->
<!-- last-edited: 2026-07-09 -->

# TASK-03 — GET /api/uaa-configs: per-host placed-config inventory (hexmac, hostname, mtime, placeholder-free bool) — read-only (todo:install-server-extras)

**Priority:** P2 · **Effort:** S · **Recommended subagent:** Haiku-class · python-services subagent · **Why:** read-only registry+filesystem join mirroring existing endpoint idioms; the only risk is leaking file contents, which this brief forbids twice · **Depends on:** TASK-02 (same-file serialization on `scripts/autoinstall-agent.py` — wave 3, start only after TASK-02's PR merges)

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/install-server-list-configs-endpoint" -b agent/install-server-list-configs-endpoint origin/main
cd "$REPO/.worktrees/install-server-list-configs-endpoint"
git rebase origin/main
```

(Protocol is also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

## Goal

Add a read-only `GET /api/uaa-configs` route to `scripts/autoinstall-agent.py` that inventories placed per-host configs: scan `CLOUD_INIT_BASE` (`/var/www/html/cloud-init`) for `<hexmac>/uaa.yaml`, and report per file `{hexmac, hostname, mtime, placeholder_free}` — **NEVER file contents, never any value from inside the file**: placed `uaa.yaml` files hold real secrets (LOCKED Decision 3 of `docs/specs/install-server-design.md`, design C3). Structure the scan as a reusable module-level collection function `collect_uaa_configs(base, registry)` so TASK-05's dashboard calls it instead of re-scanning. REUSE the existing `mac_to_hex(mac)` helper, the `CLOUD_INIT_BASE` constant, `load_registry()`, and `self.send_json(...)` — do NOT hard-code a second copy of the cloud-init path, do NOT write a new MAC normalizer.

## Background (verify before editing)

- Placed configs live at `/var/www/html/cloud-init/<hexmac>/uaa.yaml`, put there by `scripts/deploy-usb-configs.sh`. The single-host fetch `GET /autoinstall/uaa-config` ALREADY SHIPPED (PR #27) — its `resolve_cloud_init_dir` MAC-as-identity idiom is your reference for how `CLOUD_INIT_BASE` is used; do not re-plan or duplicate it.
- `CLOUD_INIT_BASE` also contains non-hexmac entries (`README.md`, `reporting.sh`, `scripts/`) — the scan must skip anything that is not a 12-char lowercase hex directory name, and skip hexmac dirs that have no `uaa.yaml` (netboot-only hosts with just user-data seeds).
- Hostname join: `registry.json` (via `load_registry()`) is keyed by colon-separated MAC; `mac_to_hex(mac)` converts to the hexmac directory name. An unregistered hexmac dir reports `hostname: None` — it is still listed, not skipped.
- `placeholder_free` = `b"REPLACE_AT_PLACE_TIME" not in <file bytes>` — a byte-scan, the file bytes are read for the boolean ONLY and never serialized.
- Collection-endpoint convention (design Decision 5): a missing/empty cloud-init root is an EMPTY inventory (`{"configs": [], "count": 0}`, HTTP 200), not an error.
- **HARD RULES in force:** `scripts/autoinstall-agent.py` is a REPO MIRROR — never touch 172.16.2.30; the deploy is a documented HUMAN step (`scp` + `sudo systemctl restart autoinstall-agent`), you only record it. No secrets enter git and no endpoint may return placed-config contents. Stay in your worktree; NEVER push/PR/merge — the coordinator owns all git.

**Re-verify these anchors** — line numbers drift, they are a starting point only. Zero hits = STOP and report:

```bash
grep -n 'def do_GET' scripts/autoinstall-agent.py                          # expect: 1 hit ~line 180
grep -n 'def resolve_cloud_init_dir' scripts/autoinstall-agent.py          # expect: 1 hit ~line 127
grep -n 'if path == "/autoinstall/uaa-config"' scripts/autoinstall-agent.py  # expect: 1 hit ~line 391
grep -n 'REGISTRY_FILE = ' scripts/autoinstall-agent.py                    # expect: 3 hits ~lines 36-38
# reuse targets (cite-by-symbol):
grep -n 'def mac_to_hex' scripts/autoinstall-agent.py                      # expect: 1 hit
grep -n 'CLOUD_INIT_BASE = ' scripts/autoinstall-agent.py                  # expect: 1 hit
grep -n 'if path == "/api/health"' scripts/autoinstall-agent.py            # DEPENDENCY-GATED anchor: 1 hit only AFTER TASK-02 merges to origin/main (0 hits at planning-time HEAD is expected). 0 hits at DISPATCH time = STOP: wrong wave, do not proceed (coordinator must confirm TASK-02 merged before dispatching this brief)
```

## Step-by-step

1. Run the anchor greps. The ONLY file you edit is `scripts/autoinstall-agent.py`. If the `/api/health` anchor is missing, STOP — TASK-02 has not merged and you are in the wrong wave.
2. Add the collection function **immediately above `def generate_certs`** (re-find: `grep -n 'def generate_certs' scripts/autoinstall-agent.py`). It must take `base` and `registry` as parameters and use only `os`, `re`, `datetime`, `mac_to_hex`, and builtins (a test extracts and `exec`s exactly this function together with `mac_to_hex`):

   ```python
   def collect_uaa_configs(base, registry):
       """Inventory placed <hexmac>/uaa.yaml files under base.

       METADATA ONLY — hexmac, hostname, mtime, placeholder_free. NEVER return
       or log file contents: placed uaa.yaml files hold real secrets.
       """
       hex_to_hostname = {}
       for mac, entry in registry.items():
           hex_to_hostname[mac_to_hex(mac)] = entry.get("hostname")
       configs = []
       try:
           names = sorted(os.listdir(base))
       except OSError:
           return configs          # missing root -> empty inventory, not an error
       for name in names:
           if not re.fullmatch(r"[0-9a-f]{12}", name):
               continue            # skip README.md, reporting.sh, scripts/, ...
           fpath = os.path.join(base, name, "uaa.yaml")
           try:
               st = os.stat(fpath)
               data = open(fpath, "rb").read()
           except OSError:
               continue            # hexmac dir without a placed uaa.yaml
           configs.append({
               "hexmac": name,
               "hostname": hex_to_hostname.get(name),
               "mtime": datetime.utcfromtimestamp(st.st_mtime).strftime("%Y-%m-%dT%H:%M:%SZ"),
               "placeholder_free": b"REPLACE_AT_PLACE_TIME" not in data,
           })
       return configs
   ```

   Edge semantics (do not guess): missing `base` dir → `[]`; non-hexmac entry → skipped; hexmac dir without `uaa.yaml` → skipped; unregistered hexmac → listed with `hostname: None` (NOT skipped); file still containing the placeholder → listed with `placeholder_free: False` (NOT skipped). Each dict has exactly those 4 keys — no `content`, no `path`, nothing read from inside the yaml besides the boolean byte-scan.
3. In `do_GET`, insert the route **immediately after the `/api/health` block** (anchor grep above):

   ```python
           # ── Placed-config inventory (metadata only — never file contents) ──
           if path == "/api/uaa-configs":
               configs = collect_uaa_configs(CLOUD_INIT_BASE, load_registry())
               self.send_json(200, {"configs": configs, "count": len(configs)})
               return
   ```

   Purely additive: do not modify `/autoinstall/uaa-config`, `resolve_cloud_init_dir`, or any existing route/helper/import.
4. Bump the mirror's file header: version minor-bump from its current post-rebase value, `last-edited: 2026-07-09`, guid unchanged.
5. Record (do NOT execute — HUMAN step) the deploy note:

   ```bash
   # HUMAN deploy (documentation only — never run by an agent):
   scp scripts/autoinstall-agent.py 172.16.2.30:/var/www/html/cloud-init/scripts/autoinstall-agent.py
   ssh 172.16.2.30 'sudo systemctl restart autoinstall-agent'
   curl -s http://172.16.2.30:25000/api/uaa-configs | python3 -m json.tool
   ```

## How to test

```bash
python3 -m py_compile scripts/autoinstall-agent.py
# Expected: exit 0, no output

python3 - <<'PY'
import os, re, tempfile
from datetime import datetime
src = open("scripts/autoinstall-agent.py").read()
def extract(name):
    start = src.index("def " + name)
    return src[start:src.index("\ndef ", start + 1)]
ns = {"os": os, "re": re, "datetime": datetime}
exec(extract("mac_to_hex"), ns)
exec(extract("collect_uaa_configs"), ns)
f = ns["collect_uaa_configs"]
base = tempfile.mkdtemp()
assert f(base, {}) == []                                    # empty root -> empty inventory
assert f(os.path.join(base, "missing"), {}) == []           # missing root -> empty inventory
os.makedirs(os.path.join(base, "scripts"))                  # non-hexmac dir
open(os.path.join(base, "README.md"), "w").write("x")       # non-hexmac file
d = os.path.join(base, "6c4b90bcf7f4"); os.makedirs(d)
open(os.path.join(d, "uaa.yaml"), "w").write("luks_key: REPLACE_AT_PLACE_TIME\n")
d2 = os.path.join(base, "aabbccddeeff"); os.makedirs(d2)    # hexmac dir, no uaa.yaml
reg = {"6c:4b:90:bc:f7:f4": {"hostname": "len-serv-003", "mac": "6c:4b:90:bc:f7:f4"}}
out = f(base, reg)
assert len(out) == 1, out                                   # hexmac+uaa.yaml listed (anti-over-suppression)
c = out[0]
assert c["hexmac"] == "6c4b90bcf7f4" and c["hostname"] == "len-serv-003"
assert c["placeholder_free"] is False and c["mtime"].endswith("Z")
assert sorted(c) == ["hexmac", "hostname", "mtime", "placeholder_free"]   # METADATA ONLY — no contents key
out2 = f(base, {})                                          # unregistered -> hostname None, still listed
assert out2[0]["hostname"] is None
print("collect_uaa_configs: 8 assertions passed")
PY
# Expected: collect_uaa_configs: 8 assertions passed

cargo test --lib --offline
# Expected: 237+ passed; 0 failed (untouched Rust stays green)
cargo build --offline
# Expected: exit 0
```

## Acceptance criteria

- [ ] Route present: `grep -n 'if path == "/api/uaa-configs"' scripts/autoinstall-agent.py` → 1 hit inside `do_GET`; `grep -n 'placeholder_free' scripts/autoinstall-agent.py` → ≥1 hit.
- [ ] Collection helper correct: the `python3` heredoc test above prints `collect_uaa_configs: 8 assertions passed`.
- [ ] Anti-over-suppression: in that test, the valid `6c4b90bcf7f4/uaa.yaml` entry IS listed (`len(out) == 1`) despite the non-hexmac skip filter — the skip path must not swallow real inventory entries.
- [ ] No content leak: the test's `sorted(c) == ["hexmac", "hostname", "mtime", "placeholder_free"]` assertion passes — exactly those 4 metadata keys, no `content`/`body`/`yaml` key in any returned dict.
- [ ] Reuse, not reinvention: `grep -c 'def mac_to_hex' scripts/autoinstall-agent.py` → 1 and `grep -c 'CLOUD_INIT_BASE = ' scripts/autoinstall-agent.py` → 1 (no duplicate helper/constant added).
- [ ] Only `scripts/autoinstall-agent.py` changed: `git diff --name-only origin/main` lists exactly that file.
- [ ] Tests green: `python3 -m py_compile scripts/autoinstall-agent.py` exits 0; `cargo test --lib --offline` shows 237+ passed, 0 failed; `cargo build --offline` exits 0.
- [ ] File header actually bumped BY THIS TASK (a date grep is vacuous — the file already carries today's date at HEAD): `git diff origin/main -- scripts/autoinstall-agent.py | grep -c '^+# version:'` → 1 AND `git diff origin/main -- scripts/autoinstall-agent.py | grep -c '^[+-]# guid:'` → 0.

## Commit message

```
feat(install-server): GET /api/uaa-configs placed-config inventory (metadata only)

Read-only scan of /var/www/html/cloud-init/<hexmac>/uaa.yaml via a reusable
collect_uaa_configs(base, registry) helper: hexmac, registry-joined hostname,
mtime, and placeholder_free (byte-scan for REPLACE_AT_PLACE_TIME). Never returns
file contents — placed configs hold real secrets. Missing root -> empty inventory,
200. Mirror only; human deploys via scp + systemctl restart autoinstall-agent.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

Idempotency (additive): `grep -n 'def collect_uaa_configs' scripts/autoinstall-agent.py && grep -n 'if path == "/api/uaa-configs"' scripts/autoinstall-agent.py` — if both hit, the change is already applied; run the acceptance checks instead of re-applying. Rollback: `git revert` the single commit removes the route and helper; nothing persists behind the endpoint, and the deployed server is unaffected until a human re-deploys the mirror; siblings unaffected (TASK-05 depends on this helper — reverting after wave 4 also requires reverting TASK-05's commit).
