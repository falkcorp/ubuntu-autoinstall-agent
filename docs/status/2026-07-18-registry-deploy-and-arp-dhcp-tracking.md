<!-- file: docs/status/2026-07-18-registry-deploy-and-arp-dhcp-tracking.md -->
<!-- version: 1.0.0 -->
<!-- guid: 7c3d9a12-5e84-4b6f-9c1a-2e8f0d4b6a37 -->
<!-- last-edited: 2026-07-18 -->

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
  stage + restart). Note it **only stages `uaa-control` and the legacy
  agent**, not the `uaa` CLI — the CLI runs from `target/release/uaa`.
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
# on 172.16.2.30, needs sudo (jdfalk password):
sudo -u uaa /home/jdfalk/ubuntu-autoinstall-agent/target/release/uaa \
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
addresses; dnsmasq only supplements PXE/boot options. But because it must
decide whether to answer each client, **dnsmasq sees every DHCP DISCOVER from
every device on 172.16.2.0/23** and, with `log-dhcp`, writes each to the
journal (`journalctl -u dnsmasq`). Confirmed live: e.g. an Apple Watch
DHCP-ing shows up there. **The journal is the source that "gets everything."**

Consequence: because proxy mode assigns no IPv4 leases, `/var/lib/misc/
dnsmasq.leases` stays ~empty and a `dhcp-script=` hook would **not** fire for
IPv4. Passive capture must **follow the journal**, not the lease file.
(IPv6 is different — `ipv6-netboot.conf` runs a real stateful DHCPv6 pool.)

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
4. The **passive discovery inbox is unbuilt**: `GET /api/discovered` is a stub
   returning `[]` (`operator/handlers.rs:786`), `handle_dismiss_discovered` is
   `not_implemented` (`:793`), and the SPA **Discovery** page
   (`web/src/pages/Discovery.tsx`) therefore always shows empty. The design for
   it lives at `docs/agent-tasks/uaa-pxe/README.md` (TASK-03: journald dnsmasq
   follow → unknown-MAC extraction → `UpsertDiscoveredMac` →
   `StreamDiscoveredMacs`), but the `crates/uaa-pxe` crate does not exist yet.

### To make "track everything" true (the remaining work)

Wire the dnsmasq journal into the registry. Minimal shape:

1. A follower on the server: `journalctl -u dnsmasq -f -o cat` → parse
   `DHCPDISCOVER(...) <mac>` / `DHCPREQUEST` lines → dedupe.
2. A machine-plane ingest endpoint that upserts each MAC as a discovered/`Seen`
   row (extends `record_seen_mac`; today no route accepts a bare "I saw this
   MAC").
3. Surface via the already-present-but-stubbed `GET /api/discovered` →
   Discovery SPA page.

This is the `uaa-pxe` discovery-inbox feature. Until it lands, "tracking" =
any device that PXE-boots or fetches an autoinstall seed (plus explicit
registrations and pre-placed configs). The **data source** for full coverage
is confirmed present and correct (dnsmasq proxy-mode journal); only the
consumer is missing.
