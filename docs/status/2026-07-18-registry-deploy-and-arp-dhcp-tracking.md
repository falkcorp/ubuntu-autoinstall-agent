<!-- file: docs/status/2026-07-18-registry-deploy-and-arp-dhcp-tracking.md -->
<!-- version: 1.2.0 -->
<!-- guid: 7c3d9a12-5e84-4b6f-9c1a-2e8f0d4b6a37 -->
<!-- last-edited: 2026-07-19 -->

# Registry deploy + ARP/DHCP device tracking — state of play (2026-07-18)

Operational status report. Terse, file:line-anchored. Companion to the
`deploy-system` execution notes and the machine-plane code under
`crates/uaa-control/src/machine_plane/`.

## What got deployed

- Server **172.16.2.30** (hostname `unimatrixzero`), repo at
  `/home/jdfalk/ubuntu-autoinstall-agent`, now on `main @ 15be746`
  (DS-OPS-05 reify/backfill/shadow-registration included).
- `uaa-control` rebuilt (`cargo build --release`, which runs
  `crates/uaa-control/build.rs` → `npm ci && npm run build`, embedding the
  operator SPA into the binary via `rust_embed` — see
  `crates/uaa-control/src/operator/web_ui.rs:24`). Staged to
  `/usr/local/bin/uaa-control`, `uaa-control.service` restarted, healthy.
- Deploy mechanism: `scripts/server-deploy.sh` (no args = pull + build +
  stage + restart). It stages `uaa-control` and the `uaa` package's binary —
  which is **named `ubuntu-autoinstall-agent`** (`crates/uaa/Cargo.toml`
  `[[bin]]`), staged at `/usr/local/bin/ubuntu-autoinstall-agent`. That single
  binary carries the `config` CLI (`place`, `backfill`). There is no
  `target/release/uaa`.
- Recurring gotcha: `npm run build` deletes `web/dist/.gitkeep`, which makes
  the next `git pull --ff-only` bail ("local changes"). Restore with
  `git checkout -- web/dist/.gitkeep` before re-deploying.

## The two registries (they are different things)

| | Profile registry | Machine registry |
|---|---|---|
| **What** | Host-group profiles, host profiles, hostname allocations | Machines seen on the wire / enrolled |
| **Store** | `/var/lib/uaa` snapshot (`ProfileStore`, `crates/uaa-control/src/profiles/store.rs`) | same snapshot file, `MachineRow` (`crates/uaa-control/src/db/mod.rs:79`) |
| **Seeded by** | `uaa config backfill` (DS-OPS-05) | reactive capture (below) + `/api/register` |
| **Operator API** | `GET /api/profiles`, `/api/groups` (:15000) | `GET /api/machines` (:15000), `/api/registry` (:25000) |
| **SPA page** | Profiles | Machines |

The **Profiles** page is empty until `uaa config backfill` reifies the
committed `examples/configs/install/<host>.yaml` fleet into the profile
registry. Backfill is idempotent and non-destructive (writes registry rows,
never touches placed configs). It writes to `/var/lib/uaa`, so it must run as
the `uaa` user:

```bash
# on 172.16.2.30, needs sudo (jdfalk password). The CLI is the `uaa` package's
# binary, which is named `ubuntu-autoinstall-agent` (crates/uaa/Cargo.toml
# [[bin]]) and IS staged at /usr/local/bin — NOT `target/release/uaa`, which
# does not exist on the server:
sudo -u uaa /usr/local/bin/ubuntu-autoinstall-agent \
    config backfill --src /home/jdfalk/ubuntu-autoinstall-agent/examples/configs/install
```

## Viewing the registry in the web app

- URL: **https://172.16.2.30:15000** (self-signed leaf minted from the install
  CA; accept the cert).
- No GitHub OAuth configured yet, so login uses the single-use bootstrap
  token minted on every restart (15-min TTL):
  ```bash
  sudo cat /var/lib/uaa/operator-bootstrap-token   # on the server
  ```
  POST it to `/api/auth/bootstrap` (the SPA login screen does this).
- Machines already populated: `unimatrixone` (MAC `ac:1f:6b:40:fc:e2`, last IP
  172.16.2.35, status `seen`).

## ARP / DHCP device tracking — how it actually works

**Requirement:** track *everything* that ARPs or does DHCP on the segment.

### The "gets everything" source: dnsmasq in proxy-DHCP mode

`/etc/dnsmasq.d/ubuntu-netboot.conf` on the server:

```
listen-address=172.16.2.30
dhcp-range=172.16.2.0,proxy      # proxy-DHCP: dnsmasq assigns NO IPv4 addresses
log-dhcp                          # every DHCP transaction is logged
```

In **proxy-DHCP mode** the segment's real DHCP server (the router) hands out
addresses; dnsmasq only supplements PXE/boot options. It does *see* every
client's broadcast — the journal shows `client provides name: Watch/Mac/…` per
transaction — **but, verified live, it does NOT log the client MAC for non-PXE
clients** (0 MAC-bearing lines in 24h of journal). Only the DHCP xid, the
subnet, and the client-supplied name are logged. So the dnsmasq journal is
**NOT** a usable "gets everything" MAC source, and neither is the lease file
(proxy mode assigns no IPv4 leases, so it stays ~empty and a `dhcp-script=`
hook never fires).

**The actual "gets everything" source is the kernel neighbor (ARP/NDP)
table.** The server is on 172.16.2.0/23, so `ip neigh` accumulates an entry —
with the resolved `lladdr` MAC — for every device that communicates on the
segment. Confirmed live: **60 unique MACs** across 172.16.2.x/172.16.3.x,
including U1 (`ac:1f:6b:40:fc:e2` @ 172.16.2.35). That is the capture source.

### What the code captures today (reactive only)

The machine registry is populated **reactively**, at the moment a device makes
an HTTP request to the machine plane (`:25000`):

- `crates/uaa-control/src/machine_plane/seeds.rs:92` — on an `/autoinstall/*`
  fetch, the server runs `ip neigh show <client_ip>` to translate the request's
  source IP into a MAC (`mac_from_neighbor_output`, `seeds.rs:56`).
- `record_seen_mac` (`seeds.rs:206`) upserts a `MachineRow` with `status =
  Seen`. Its only caller is `resolve_or_deny` (`seeds.rs:170`), invoked from the
  five `/autoinstall/*` GET handlers.
- Also: `/api/register` (self-reported MAC+hostname, `lifecycle.rs:278`) and
  `backfill_placed_configs` (a synthetic `Seen` row for any pre-placed
  cloud-init dir, `operator/handlers.rs:156`).

### The gap vs. "track everything"

A device that ARPs / DHCPs / PXE-boots but **never issues an autoinstall HTTP
fetch is never recorded**. Additional narrowings:

1. Capture fires only from `/autoinstall/*` (`seeds.rs:182,187`). DHCP alone
   registers nothing.
2. No live ARP neighbor entry at fetch time ⇒ dropped (`seeds.rs:177-180`).
3. `/api/checkin` rejects unknown MACs (`lifecycle.rs:396`).
4. This narrow reactive capture feeds the **Machines** page (the fleet).

### Passive "track everything" — now BUILT (the Discovery inbox)

Implemented 2026-07-19 (`feat(discovery): passive ARP/NDP discovery inbox`,
PR #120). Separate from the reactive Machines path above; lands on the SPA
**Discovery** page, not Machines, so the fleet list stays clean:

1. **Scanner** — `scripts/arp-discovery-scan.sh` (systemd
   `uaa-arp-discovery.service`) polls `ip neigh` every 30s and POSTs each MAC
   to the ingest, with a per-MAC re-POST throttle.
2. **Ingest** — `POST /api/discovered` on the machine plane (`:25000`,
   unauthenticated, localhost), `crates/uaa-control/src/discovered.rs`. Upserts
   into `/var/lib/uaa/discovered-macs.json` (its own file — never the fleet
   registry snapshot, so the bursty scanner cannot race the autoinstall
   handlers). Verified live: valid MAC → 204, malformed → 400.
3. **Surface** — `GET /api/discovered` + `POST /api/discovered/:mac/dismiss` on
   the operator plane (`:15000`) now read/mutate that file (the
   `not_implemented`/empty-vec stubs are gone), rendered by
   `web/src/pages/Discovery.tsx`.

**Operator step to activate the scanner** (needs sudo, one-time):

```bash
sudo cp crates/uaa-control/systemd/uaa-arp-discovery.service /etc/systemd/system/
sudo systemctl daemon-reload && sudo systemctl enable --now uaa-arp-discovery.service
```

Once enabled, the Discovery page fills with every device on the segment
(~60 today) for approve/dismiss triage. The dnsmasq-journal follower and the
`crates/uaa-pxe` crate in `docs/agent-tasks/uaa-pxe/README.md` are NOT used —
that design assumed the journal logged MACs, which on this box it does not.
