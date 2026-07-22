### Changed

#### Discovery now identifies fleet machines and surfaces them in Machines

The ARP/NDP discovery scanner (`scripts/arp-discovery-scan.sh`) now records each
device's **IP** and resolves its **hostname** server-side (`getent hosts <ip>` →
`/etc/hosts`, DNS, dnsmasq), so `172.16.2.45` becomes `rpi-serv-001`. Named
devices — the fleet the server can see on the wire (rpi-servs, len-servs, …) —
are promoted into the **Machines** list (a new `backfill_discovered_named` in the
operator plane, parallel to the placed-config backfill) where they're approvable;
**unidentified** consumer devices (phones/IoT, no resolved hostname) stay out of
the fleet list instead of flooding it. `DiscoveredMacRow` gains optional
`ip`/`hostname`, refreshed on later scans without clobbering a known name.

#### Machines UI: hide Approve once approved, readable Last seen

The Machines page no longer shows the **Approve** button for a machine already
`approved` (Reinstall still shows), and renders `Last seen` as a local date-time
instead of a raw unix epoch.
