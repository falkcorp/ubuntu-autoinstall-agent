<!-- file: docs/status/2026-07-11-u1-autoinstall-debug-session.md -->
<!-- version: 1.0.0 -->
<!-- guid: 3a7c9e12-4f6b-4d81-9e3a-1c7f0b8d2a56 -->
<!-- last-edited: 2026-07-11 -->

# U1 (unimatrixone) Autoinstall Debug Session — Status

## TL;DR

Attempted to bring unimatrixone (U1) up end-to-end through the new
`uaa-control` constellation system via the USB autoinstall path (netboot on
this board is separately paused, see `unimatrixone-pxe-boot-status.md`).
Found and fixed three real, previously-invisible bugs in the pipeline (PR
#79, #80) plus a deploy checklist gap (agent binary was never staged on the
server) and an nginx routing gap (fixed live, not yet captured in a commit —
see below). U1 itself never reached a confirmed successful install — the
session ended paused mid-diagnosis, at the user's explicit direction, for
the night. **Next session's job is a UI build (porting the existing
`/dashboard`), not resuming U1 hardware work — do not power on U1 without
new operator direction.**

## Shipped this session

| PR | Area | What |
|----|------|------|
| #79 | tooling-port | `make-ssh-ready-iso.sh`: bake `console=tty0 console=ttyS0,115200n8` into the installer kernel cmdline — without this, IPMI SOL showed nothing even on a live, reachable host |
| #80 | control | `uaa-control`: `tracing_subscriber_init()` was a literal empty function — **no subscriber was ever installed**, so every `tracing::info!` in the entire binary (including the machine-plane AUTOINSTALL/UAA-CONFIG served/DENIED logs) was silently dropped. Now wired to `tracing_subscriber::fmt()` on stdout, `RUST_LOG`-overridable, default `info`. Confirmed live: `journalctl -u uaa-control` now shows real request traces. |
| #81 | docs | `todo.md`: recorded the three items below as explicit follow-ups |

## Manual server-side fixes (not yet in a commit)

- **Deployed the static agent binary** to `/var/www/html/uaa/uaa-amd64` — it
  had never been staged on the server post-cutover (part of `todo.md`'s own
  USB-bootstrap deploy checklist, step 3, apparently missed during the
  constellation cutover). Built from CI artifact `uaa-amd64` off `main`
  (workflow run 29163677140), verified as a static ELF, installed via the
  jdfalk ACL write access on `/var/www/html` (no sudo needed).
- **Added an nginx `location /uaa { root /var/www/html; }` block** to
  `/etc/nginx/sites-available/media.jdfalk.com.conf` on the server. Without
  it, `/uaa/uaa-amd64` silently fell through to the SPA catch-all's
  `index.html` (200 OK, wrong content) instead of a 404 or the real file —
  the same documented gotcha as `/ipxe`, `/ubuntu`, `/isos`, `/cloud-init`
  (see `unimatrixone-pxe-boot-status.md`). This is server config, not repo
  code, so there's nothing to PR — just noting it here so it isn't
  rediscovered from scratch. **Neither of these two fixes is captured in
  version control anywhere except this doc** — if the server is ever rebuilt
  from scratch, both need to be redone.
- **Placed U1's install config** at `/var/www/html/cloud-init/ac1f6b40fce2/
  uaa.yaml` via `scripts/deploy-usb-configs.sh --inject-from
  ~/uaa-secrets.yaml unimatrixone`. Secrets (`luks_key`, `root_password`,
  `tpm2_pin`) were freshly generated (`openssl rand`) and live in
  `~/uaa-secrets.yaml` on the server (mode 0600, outside any git tree) — not
  recorded anywhere else, by design (see `todo.md`'s secrets-automation item
  for why this should eventually be automated + encrypted-at-rest instead).

## Blocked / deferred — U1 hardware state

U1's actual install outcome is **unconfirmed**. Timeline of what's known:

1. First USB boot attempt (before PR #79/#80): stalled indefinitely,
   pingable but no SSH/nginx/SOL activity. Root cause turned out to be a
   combination of missing serial console (couldn't see anything) and the
   missing agent binary (fetch would have failed even if we could see it).
2. After PR #79 (serial console) + deploying the agent binary + the nginx
   fix: a later boot attempt (with a **bent USB-A connector**, worked via a
   USB-C→A adapter) showed real progress in the nginx log — the agent binary
   fetched successfully (200, correct 18MB size) and `reporting.sh` was
   fetched (meaning `report_status()` ran at least once) — but the
   `uaa-control` config-fetch endpoint (`:25000/autoinstall/uaa-config`)
   logged nothing in that window. This was **before** PR #80 landed, and at
   the time was genuinely uninformative either way (no subscriber was
   installed at all, so both "request never arrived" and "request arrived
   and was denied" looked identical: silence). Post-#80 this is no longer
   ambiguous — a denied request now logs `AUTOINSTALL DENIED - no ARP/NDP
   neighbor entry` — but that fix landed after this particular attempt, so
   we can't retroactively tell which case this was.
3. User observed on their own SOL session: **"crashed in dracut."** Whether
   this was (a) a stale prior install attempt on local disk being booted
   instead of the USB (boot-order fell through because the one-time IPMI
   `bootdev floppy` override wasn't re-applied before that particular
   power-on), or (b) a fresh install that actually ran and then failed
   post-reboot in the newly-installed system's dracut initramfs (LUKS/
   mdadm/zpool unlock failure) was **never conclusively determined**.
4. A clean retry was started (PR #80/logging fix + `bootdev floppy
   options=efiboot` re-applied + fresh power-on) with live log monitors on
   both `uaa-control` and nginx. Mid-attempt, the user observed it **trying
   to PXE boot** despite the `floppy` override — turns out this USB stick
   enumerates as "USB Hard Disk" in this board's boot-device list, not
   "Floppy/removable," so the IPMI boot-type override doesn't match it and
   it falls through to PXE first (which fails/times out, since netboot is
   separately paused) before eventually reaching the USB. **Learning for
   next attempt: there may be no direct IPMI `chassis bootdev` value that
   correctly force-selects this specific USB stick on this board; may need
   `chassis bootdev disk` tested, or manual BIOS boot-menu selection.**
5. Before that boot attempt resolved, the user asked to pause for the night.

**Current physical/power state (as of session end):** IPMI boot-device
override set to `bios` (forces entry into BIOS Setup on next power-on, does
NOT power anything on). Chassis power: **off**. The USB stick is physically
in U1, connected via a USB-C→A adapter (the stick's native USB-A connector
is bent ~90° — do not force it back in without the adapter; risk of
snapping pins in U1's port). **Do not power on U1 without explicit new
operator direction** — this session ended paused, not resolved, at the
user's explicit instruction ("DO NOT START IT... late and the kids are
asleep").

## Next steps

**Primary: port `/dashboard` from the retired Python service to Rust
`uaa-control`.** This is a port, not new design — full context:

- `crates/uaa-control/src/machine_plane/dashboard.rs` is currently a 9-line
  stub (`// STUB — Filled exclusively by install-plane IP-04`), never
  implemented, never wired into `listeners.rs`.
- A **fully working** implementation already exists and is already on
  `main` — but in the retired `scripts/autoinstall-agent.py`
  (`autoinstall-agent.service`, stopped post-cutover): `render_dashboard()`
  + the `/dashboard` GET route (commit `4900f93`). It renders a
  display-only HTML page (inline CSS, zero JS/forms, every value
  HTML-escaped) covering: agent-binary presence, the machine registry table,
  placed-config inventory (metadata only, never file contents), and the
  last 20 events.
- Port that shape against the Rust `Registry`/`crate::db::store` data model
  instead of the Python JSON-file registry, and wire the router into
  `listeners.rs` (see how other machine-plane routers are merged there,
  e.g. `seeds.rs::router()`).
- **Pair with `todo.md`'s "Record every MAC" item**: right now
  `seeds.rs::resolve_or_deny` denies unrecognized MACs without ever
  touching the `Registry` (confirmed this session — `lifecycle.rs` already
  has the right model, `MachineStatus::Pending`, `/api/register`, but
  `seeds.rs` never calls into it). The user's explicit ask: every MAC that
  has ever contacted the machine plane should be visible in the dashboard —
  approved, rejected, or newly "seen but not yet approved" — so an operator
  can match a MAC to physical hardware and approve/reject from the console,
  without needing SSH+journalctl access. The dashboard port and this
  registry-recording work are two halves of one feature; do them together
  or in that order (recording first makes the dashboard immediately useful).
- Full `todo.md` entries (with more detail/anchors) are under "Port
  /dashboard from Python to Rust uaa-control" and "Record every MAC that
  contacts the machine plane."

**Secondary / deferred, not blocking:**
- `todo.md`'s "Boot-attempt diagnostics: unfiltered DHCP capture" item — a
  broader (not MAC-filtered) `tcpdump` capture during boot attempts, since
  `dnsmasq`'s own journal has blind spots. Needs either an interactive
  `sudo -v` each time or a `cap_net_raw`/NOPASSWD grant for non-interactive
  use — noted but not set up.
- `todo.md`'s secrets-automation item (Tang-quorum-encrypted secrets instead
  of a plaintext `~/uaa-secrets.yaml` on the server) — future work, not
  blocking anything today.
- Resuming U1 hardware debugging itself — explicitly NOT next session's job
  unless the user redirects.

## Operational note: deploy/merge authorization

Repeatedly hit an auto-mode classifier block on `gh pr merge --admin` and
remote `ssh`+`sudo`/`scp` deploy actions this session, even when the
handoff brief stated "standing merge authority" and even after an
`AskUserQuestion` tool call where the user explicitly selected "yes,
proceed." The classifier did not treat that structured tool answer as
sufficient — it only unblocked after the user gave a raw, explicit
in-chat message authorizing the specific action. If the next session hits
the same wall: don't keep retrying identically — surface it plainly and
ask for a fresh, explicit chat-text authorization (not just a tool-mediated
answer).
