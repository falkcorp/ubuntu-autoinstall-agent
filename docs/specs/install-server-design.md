<!-- file: docs/specs/install-server-design.md -->
<!-- version: 1.0.0 -->
<!-- guid: 93efe217-06fc-4985-9b39-1324eaf25039 -->
<!-- last-edited: 2026-07-09 -->

# Install Server Operability (install-server) — Design Spec

**Status:** Approved — ready for implementation planning
**Scope:** Python HTTP-service mirror (`scripts/autoinstall-agent.py`) + server-local placement script (`scripts/deploy-usb-configs.sh`) + docs (`docs/netboot-autodeploy.md`). No Rust source changes. Explicit follow-up deferred: TLS/auth on :25000.
**Parent task:** plan-op install-ops — workstream `install-server` (5 tasks)

---

## Motivation

The autoinstall HTTP service on `172.16.2.30:25000` (repo mirror: `scripts/autoinstall-agent.py`, deployed at `/var/www/html/cloud-init/scripts/autoinstall-agent.py`) has four operability gaps confirmed by scout evidence:

1. **A successful uaa install never auto-flips the boot menu.** The `/api/webhook` handler auto-flips a host's iPXE menu-default to `boot-local-disk` only when the payload `status` is `"finished"` or `"complete"`. The Rust agent (`src/network/ssh_installer/status.rs` `post_status`, driven by `Installer::report` in `installer.rs`) posts `"running"`, `"failed"`, and `"success"` at 100% — **never** `"finished"` or `"complete"`. Today every successful SSH/USB install leaves the host set to netboot-reinstall until a human flips it manually.
2. **No health/inventory surface.** There is no way to ask the service "are you up, what's registered, is the agent binary (`/var/www/html/uaa/uaa-amd64`) actually present?" — `build-musl.sh` only *prints* the deploy command as a human hint, so the `/uaa/` directory may not exist on the server at all.
3. **No placed-config inventory.** Per-host `uaa.yaml` files live at `/var/www/html/cloud-init/<hexmac>/uaa.yaml` (served by `GET /autoinstall/uaa-config`, **already shipped in PR #27** — referenced here, not re-planned). Nothing reports which hosts have a placed config, when it was placed, or whether it still contains `REPLACE_AT_PLACE_TIME` placeholders.
4. **Secret placement is manual string surgery.** `scripts/deploy-usb-configs.sh` correctly *refuses* to place a config still carrying `REPLACE_AT_PLACE_TIME`, but offers no assisted way to fill those slots at place time — operators hand-edit copies, which risks stray secret-bearing files.

### Current behavior (grep-verified anchors)

Run these from the repo root; every claim above is pinned to an anchor (line numbers are approximate — the grep is authoritative):

```bash
# Auto-flip fires ONLY on "finished"/"complete" (~line 489):
grep -n 'status in ("finished", "complete")' scripts/autoinstall-agent.py     # expect 1 hit
# Rust reports final success as status="success" progress=100 (~line 235):
grep -n '"success", 100' src/network/ssh_installer/installer.rs               # expect 1 hit
# Rust webhook payload builder (~line 23):
grep -n 'pub async fn post_status' src/network/ssh_installer/status.rs        # expect 1 hit
# flip_ipxe rewrites `set menu-default` in the per-MAC .ipxe file (~line 101):
grep -n 'def flip_ipxe' scripts/autoinstall-agent.py                          # expect 1 hit
# GET routing block (~line 180) and POST routing block (~line 421):
grep -n 'def do_GET' scripts/autoinstall-agent.py                             # expect 1 hit
grep -n 'def do_POST' scripts/autoinstall-agent.py                            # expect 1 hit
# MAC-as-identity resolution: client IP -> ip neigh -> hexmac dir (~line 127):
grep -n 'def resolve_cloud_init_dir' scripts/autoinstall-agent.py             # expect 1 hit
# /autoinstall/uaa-config already shipped (PR #27), hard-404 convention (~line 391):
grep -n 'if path == "/autoinstall/uaa-config"' scripts/autoinstall-agent.py   # expect 1 hit
# Placement refusal gate (~line 36):
grep -n 'PLACEHOLDER="REPLACE_AT_PLACE_TIME"' scripts/deploy-usb-configs.sh   # expect 1 hit
# Per-host configs carry 3 placeholder secrets + 1 comment:
grep -c 'REPLACE_AT_PLACE_TIME' examples/configs/install/len-serv-003.yaml    # expect count = 4
# Rust load-time gate mirrors the refusal (~line 588):
grep -n 'REPLACE_AT_PLACE_TIME placeholders' src/cli/commands.rs              # expect 1 hit
# Canonical binary deploy target (~lines 17, 46):
grep -n '/var/www/html/uaa/uaa-amd64' scripts/build-musl.sh                   # expect 2 hits
# USB bootstrap curls the binary from that URL (~line 33):
grep -n 'UAA_AGENT_URL:-http' installer-image/nocloud/uaa-usb-bootstrap.sh    # expect 1 hit
# Registry/state files under /var/log/cockroach-autoinstall (~lines 36-38):
grep -n 'REGISTRY_FILE = ' scripts/autoinstall-agent.py                       # expect 3 hits
# registry.json entry schema incl. tpm_ek (~452 register dict, ~581 seed):
grep -n '"tpm_ek"' scripts/autoinstall-agent.py                               # expect 6 hits
# nginx web-root layout incl. ipxe/boot/mac-<hexmac>.ipxe (~lines 80, 267):
grep -n 'ipxe/boot/mac-' docs/netboot-autodeploy.md                           # expect 2 hits
```

**Goal:** a successful uaa install auto-flips its host to local-disk boot, and the install server exposes read-only health / config-inventory / dashboard surfaces — without adding any HTTP path that writes or transports secrets.

## Goals

- Auto-flip fires when the Rust agent reports `status="success"` (and keeps firing for cloud-init's `finished`/`complete`), and tolerates USB-only hosts that have no `mac-<hexmac>.ipxe` file.
- `GET /api/health` — service liveness, registry counts, and a `stat`-based check that `/var/www/html/uaa/uaa-amd64` is present (never assuming the directory exists).
- `GET /api/uaa-configs` — per-hexmac inventory of placed `uaa.yaml` files: hostname, mtime, placeholder-free boolean. **Never file contents** — those files hold real secrets after placement.
- `GET /dashboard` — a single-page, display-only HTML aggregation of the above.
- `deploy-usb-configs.sh --inject-from <secrets.yaml>` — fill `REPLACE_AT_PLACE_TIME` slots at place time, server-locally, human-run, never logging values, with the existing refusal gate preserved as the final backstop.
- Every mirror change documents the human deploy step (`scp` + `sudo systemctl restart autoinstall-agent`).

## Non-goals (v1)

- **HTTP secret-write / config-placement API** — rejected outright (see Decision 1) — not deferred, refused.
- TLS or authentication on :25000 — deferred to a future wave; nothing in this workstream widens the trust placed in the plain-HTTP channel.
- Changing the Rust status vocabulary (`running`/`failed`/`success`) — the server side widens instead (Decision 2).
- Removing or refactoring existing endpoints; Path A/Path B installer work (other workstreams own that).
- Re-planning `GET /autoinstall/uaa-config` — already shipped in PR #27.

## Decisions (locked)

These were locked by the operator during planning. Do **not** reopen them during implementation.

1. **NO HTTP secret-write/placement API.** Scout-confirmed risk: `:25000` is plain HTTP; MAC/ARP identity is spoofable on-subnet; `GET /autoinstall/uaa-config` already serves secret-injected `uaa.yaml` to anyone who spoofs a registered MAC's IP, and a POST that *writes* secrets would amplify that exposure. Placement stays the server-local, human-run script: `deploy-usb-configs.sh` gains `--inject-from <secrets.yaml>` — values are filled into the `REPLACE_AT_PLACE_TIME` slots at place time, never logged, and the existing refusal gate stays as the final check. **Rejected alternatives:** `POST /api/place-config` (secret amplification over plain HTTP); TLS + auth on :25000 to make such an API safe (out of scope for this wave).
2. **Widen the webhook auto-flip tuple to include `"success"`** rather than changing the Rust vocabulary. The Rust agent posts `running`/`failed`/`success` — never `finished`/`complete`. Widening the server tuple is the low-blast-radius fix: cloud-init's `reporting.sh` path (which sends `finished`) keeps working, and the Rust events-feed vocabulary is untouched. The flip must also **tolerate hosts with no `mac-<hexmac>.ipxe` file** (USB-only installs that never netboot-registered): log and continue, never error the webhook response. **Rejected alternative:** making the Rust installer post `"finished"` (breaks the status vocabulary used in the events feed and conflates per-phase semantics).
3. **New py-mirror endpoints are strictly read-only.** `/api/health` (service up, registry counts, `uaa-amd64` presence via `stat`/`os.path.isfile` — do NOT assume `/var/www/html/uaa` exists), `/api/uaa-configs` (per-hexmac `uaa.yaml` inventory: hostname, mtime, placeholder-free bool — **never file contents**), `/dashboard` (single-page HTML, display-only). No new POST handlers, no state writes. **Rejected alternative:** a config-content endpoint for debugging (leaks placed secrets).
4. **Every py-mirror task documents the human deploy step.** `scripts/autoinstall-agent.py` is a REPO MIRROR — edits do nothing until a human runs `scp scripts/autoinstall-agent.py 172.16.2.30:/var/www/html/cloud-init/scripts/autoinstall-agent.py && ssh 172.16.2.30 'sudo systemctl restart autoinstall-agent'`. Briefs include this as documentation; no task executes it.
5. **New GET endpoints follow the hard-404 convention** established by `/autoinstall/uaa-config` (missing resource → 404), not the empty-200 convention of the cloud-init seed files.

## Data model

New JSON response shapes (Python dicts serialized with the module's existing `json.dumps` idiom):

```python
# GET /api/health -> 200 application/json
{
    "status": "ok",                 # literal; the endpoint answering at all is the liveness signal
    "registry_hosts": 4,            # len(registry) from /var/log/cockroach-autoinstall/registry.json
    "registry_approved": 3,         # count of entries with entry["status"] == "approved"
    "yubikeys": 0,                  # len of yubikey-registry.json (0 if file absent)
    "tang_servers": 0,              # len of tang-registry.json (0 if file absent)
    "agent_binary": {
        "path": "/var/www/html/uaa/uaa-amd64",
        "present": False,           # os.path.isfile() — MUST NOT assume /var/www/html/uaa exists
        "size": None,               # os.stat().st_size when present, else None
        "mtime": None               # ISO-8601 UTC from st_mtime when present, else None
    }
}

# GET /api/uaa-configs -> 200 application/json
{
    "configs": [
        {
            "hexmac": "aabbccddeeff",       # directory name under /var/www/html/cloud-init/
            "hostname": "len-serv-003",     # from registry.json MAC match; None if unregistered
            "mtime": "2026-07-09T18:00:00Z",# uaa.yaml st_mtime, ISO-8601 UTC
            "placeholder_free": True        # True iff b"REPLACE_AT_PLACE_TIME" not in file bytes
        }
    ],
    "count": 1
}
# NEVER include file contents, key names' values, or any yaml body — placed files hold real secrets.

# POST /api/webhook auto-flip condition (widened tuple — Decision 2):
#   before: status in ("finished", "complete")
#   after:  status in ("finished", "complete", "success")
# flip_ipxe() failure (no mac-<hexmac>.ipxe file) is caught and logged; webhook still returns 200.
```

### Persistence

No new persistent state. All three endpoints read existing files:

- `/var/log/cockroach-autoinstall/registry.json` → host counts + MAC→hostname join
- `/var/log/cockroach-autoinstall/{yubikey-registry,tang-registry}.json` → counts
- `/var/www/html/cloud-init/<hexmac>/uaa.yaml` → mtime + placeholder scan (bytes, never echoed)
- `/var/www/html/uaa/uaa-amd64` → `stat` only

`deploy-usb-configs.sh --inject-from` writes only its existing target (`/var/www/html/cloud-init/<hexmac>/uaa.yaml`) plus a `mktemp`'d intermediate that is `rm`'d on every exit path (`trap ... EXIT`) and created `umask 077`.

## Components

### C1. Webhook flip widening (`scripts/autoinstall-agent.py`) — TASK-01

In the `/api/webhook` POST handler, change the flip condition tuple from `("finished", "complete")` to `("finished", "complete", "success")`. Wrap the `flip_ipxe(...)` call so a missing `mac-<hexmac>.ipxe` file (USB-only host — `flip_ipxe` already reports "No iPXE file found") is **logged as an event and swallowed**: the webhook response stays 200 with the flip outcome noted in the JSON body. Fail-open for the flip, fail-closed for nothing — a webhook must never 500 because a host has no iPXE file. Reuse the existing `flip_ipxe(hostname, target="boot-local-disk")` helper (verify: `grep -n 'def flip_ipxe' scripts/autoinstall-agent.py`); do not write a second flip path.

### C2. `GET /api/health` + binary-serving docs (`scripts/autoinstall-agent.py`, `docs/netboot-autodeploy.md`) — TASK-02

New read-only GET route in `do_GET`, following the existing JSON-response idiom of `/api/registry`. Presence check is `os.path.isfile("/var/www/html/uaa/uaa-amd64")` — **default assumption: absent** (`build-musl.sh` only prints the install command as a human hint). Registry counts default to 0 when a registry file is missing/unreadable (fail-open to a degraded-but-honest response; the endpoint itself only 500s if JSON serialization fails). Docs gain a section stating: nginx `:80` serves `/uaa/uaa-amd64`; `uaa-usb-bootstrap.sh` curls `UAA_AGENT_URL` (default `http://172.16.2.30/uaa/uaa-amd64`); deploy is human-run.

### C3. `GET /api/uaa-configs` (`scripts/autoinstall-agent.py`) — TASK-03

New read-only GET route: iterate `/var/www/html/cloud-init/*/uaa.yaml` (directory names are hexmacs; skip non-hexmac entries like `README.md`, `scripts/`), join hostname from `registry.json` by MAC, report mtime and `placeholder_free` (byte-scan for `REPLACE_AT_PLACE_TIME`). Missing cloud-init root → 200 with `"configs": [], "count": 0` (inventory of nothing is an empty inventory, not an error). **Never** return file contents (Decision 3). Reuse `resolve_cloud_init_dir`'s base-path constant/idiom rather than re-hardcoding paths.

### C4. `deploy-usb-configs.sh --inject-from <secrets.yaml>` (`scripts/deploy-usb-configs.sh`) — TASK-04

Server-local, human-run only (Decision 1). New optional flag: read a non-committed `secrets.yaml` (per-host keys `luks_key`, `root_password`, `tpm2_pin`), substitute each `REPLACE_AT_PLACE_TIME` slot in a `mktemp` copy (`umask 077`, `trap 'rm -f "$tmp"' EXIT`), then hand the filled copy to the **existing** placement path so the existing gates still run: unknown-host refusal (case-lookup of the 4 fleet MACs), missing-source refusal, and the `PLACEHOLDER="REPLACE_AT_PLACE_TIME"` refusal — which now acts as the backstop catching any slot the secrets file failed to fill. Values are never echoed, never logged, never in argv of external commands visible to `ps` (use `awk`/shell-internal substitution reading from the file, not `sed "s/…/$SECRET/"` with the secret in the command line where avoidable — and if a tool must receive the value, pass it via file or stdin). Without `--inject-from`, behavior is byte-identical to today. The secrets file must never land in git: refuse to run if `<secrets.yaml>` is inside the repo working tree, and document keeping it in `~/` on the server with mode 0600.

### C5. `GET /dashboard` (`scripts/autoinstall-agent.py`) — TASK-05

Single-page, display-only HTML (no external assets, no JS frameworks — inline `<style>` only) aggregating: registry table (hostname, MAC, status, last_seen), last N events from `events.jsonl` (reuse the `/api/events` tail idiom), placed-config inventory (reuse the C3 collection function — do not duplicate the scan), and agent-binary presence (reuse the C2 check function). All values HTML-escaped via `html.escape`. No forms, no POST, no links that mutate state.

## Migration / integration

No callers migrate. All changes are additive routes/flags:

- Existing webhook senders (`reporting.sh` with `finished`, Rust `post_status` with `success`) both now trigger the flip — Before: only `finished`/`complete`; After: `finished`/`complete`/`success`.
- `deploy-usb-configs.sh` invoked without `--inject-from` is byte-identical to today.
- **Human deploy step (documented in every py-mirror task, never executed by an agent):**

  ```bash
  scp scripts/autoinstall-agent.py 172.16.2.30:/var/www/html/cloud-init/scripts/autoinstall-agent.py
  ssh 172.16.2.30 'sudo systemctl restart autoinstall-agent'
  # verify: curl -s http://172.16.2.30:25000/api/health | python3 -m json.tool
  ```

  `deploy-usb-configs.sh` runs **on the server** (it writes `/var/www/html/cloud-init/<hexmac>/`); the human copies the updated script there the same way.

## Milestones

Milestones map 1:1 to the workstream's global waves (see `docs/specs/install-server-plan.md`). `scripts/autoinstall-agent.py` is touched by TASK-01/02/03/05, which is why they serialize across waves.

- **M1 — flip fix + injection flag (wave 1).** TASK-01 (webhook tuple + missing-iPXE tolerance) and TASK-04 (`--inject-from`) in parallel — disjoint files. Additive; no existing behavior changes except the intended flip widening.
- **M2 — health surface (wave 2).** TASK-02: `/api/health` + binary-serving docs. Additive route.
- **M3 — config inventory (wave 3).** TASK-03: `/api/uaa-configs`. Additive route.
- **M4 — dashboard (wave 4).** TASK-05: `/dashboard`, reusing M2/M3 collection functions. Additive route.

Every milestone is independently shippable (each is a separate human `scp` + restart). The only behavior-changing milestone is M1's flip widening; it is intentionally un-flagged because firing the flip on success **is** the fix, and rollback is a one-line revert.

## Files modified

| File | Change |
|---|---|
| `scripts/autoinstall-agent.py` | TASK-01: widen flip tuple + tolerate missing iPXE; TASK-02: `/api/health`; TASK-03: `/api/uaa-configs`; TASK-05: `/dashboard`. Version header bumped each task. |
| `scripts/deploy-usb-configs.sh` | TASK-04: `--inject-from <secrets.yaml>` place-time injection; refusal gate preserved. |
| `docs/netboot-autodeploy.md` | TASK-02: document `/var/www/html/uaa/` nginx path, `UAA_AGENT_URL` bootstrap flow, human deploy step. |

Cross-workstream collision note: `scripts/autoinstall-agent.py` is exclusive to this workstream, but its four tasks collide with each other — hence strict wave serialization (1→2→3→4) with rebase before each.

## Testing

There is no Python test harness in this repo; the gates are compile/parse checks plus the Rust baseline (which proves no accidental Rust drift), and grep-based acceptance:

| Test / check | Asserts |
|---|---|
| `python3 -m py_compile scripts/autoinstall-agent.py` | mirror still parses after every py task |
| `bash -n scripts/deploy-usb-configs.sh` | placement script still parses (TASK-04) |
| `cargo test --lib --offline` | 237+ passed, 0 failed — no Rust drift |
| `cargo build --offline` | exits 0 |
| `grep -n 'status in ("finished", "complete", "success")' scripts/autoinstall-agent.py` | flip tuple widened (TASK-01) |
| `grep -c 'REPLACE_AT_PLACE_TIME' examples/configs/install/len-serv-003.yaml` = 4 | committed configs still placeholder-bearing (TASK-04 must not touch them) |
| `grep -n 'def api_health\|/api/health' scripts/autoinstall-agent.py` | route present (TASK-02) |
| anti-over-suppression (TASK-04) | a fully-injected temp config **passes** the placeholder gate and places; a half-injected one is **refused** |
| anti-over-suppression (TASK-01) | a host **with** an iPXE file still flips on `success` (the tolerance wrapper must not swallow the happy path) |

## Failure modes

| Failure | Behavior (specified) |
|---|---|
| Webhook `success` for USB-only host (no `mac-<hexmac>.ipxe`) | flip attempt logged, webhook returns 200 — never 500 (Decision 2) |
| `/var/www/html/uaa/` directory absent on server | `/api/health` reports `present: false` — never crashes, never assumes the dir (Decision 3) |
| Registry file missing/corrupt | `/api/health` reports counts as 0; endpoint still 200 |
| Cloud-init root missing | `/api/uaa-configs` returns empty inventory, 200 |
| `secrets.yaml` missing a key for a host | that slot stays `REPLACE_AT_PLACE_TIME` → the existing refusal gate rejects placement for that host (backstop, per-host exit-1 semantics preserved) |
| `secrets.yaml` inside the repo tree | `--inject-from` refuses to run (keeps secrets out of git) |
| Operator edits mirror but forgets deploy | documented `curl /api/health` verification exposes the stale service (Decision 4) |
| Dashboard rendering hostile hostnames | all interpolated values pass through `html.escape` |

## Rollback

- Each task is a single conventional commit on its own branch; `git revert <sha>` fully restores prior behavior — no schema, no persistent state, no data written by any endpoint.
- Server-side rollback = re-`scp` the previous mirror revision + `sudo systemctl restart autoinstall-agent` (human step, same as deploy).
- M2–M4 are dormant until a human deploys them; M1's flip widening reverts to today's never-flips-on-success behavior with a one-line revert.
- TASK-04: reverting removes the flag; placement returns to manual editing. No placed `uaa.yaml` is retroactively affected.

## Open questions (resolved — recorded for the plan)

1. ~~Fix the status mismatch in Rust or in the server?~~ → **Server-side tuple widening** (Decision 2); Rust vocabulary untouched.
2. ~~Should placement move to an HTTP API now that injection is scripted?~~ → **No — locked** (Decision 1); server-local human-run script only.
3. ~~Should `/api/uaa-configs` expose config bodies for debugging?~~ → **Never** — metadata only (Decision 3); placed files hold real secrets.
4. ~~Empty-200 or hard-404 for new endpoints' missing resources?~~ → Hard-404 convention for single resources; empty inventory = 200 for collection endpoints (Decision 5).
