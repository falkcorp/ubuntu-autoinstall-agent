<!-- file: changelog.d/operator-tables-stop-lying.md -->
<!-- version: 1.0.0 -->
<!-- guid: 2c78e0b5-9d41-4a63-b8f2-5e10a7c34d96 -->
<!-- last-edited: 2026-08-05 -->

### Fixed

- **The Machines table no longer claims every machine is "consistent".**
  `handlers::to_view` hardcoded `consistent: true` on every row, so the SPA
  rendered a green "consistent" badge for every machine — including ones that
  had never checked in even once. Cross-layer consistency checking was never
  implemented, so the UI was asserting a property nothing had ever verified. A
  field that is always `true` carries no information while looking like it
  does, which is worse than showing nothing.

  Replaced with `agent`, derived at read time from `last_app_status_at` by the
  existing `machine_plane::staleness::freshness` — which already had exactly
  the right three-way semantics and was simply never surfaced:
  `reporting` (checked in within 15 minutes), `stale` (has reported, but not
  recently), `never` (has never reported). Only `reporting` renders green:
  `stale` and `never` mean the server does not know, which is neither healthy
  nor unhealthy, and colouring them either way would repeat the original
  mistake in the opposite direction.

  Real drift data was never missing — it has always been on `GET /api/drift`.
  Nothing was wired to it.

- **Both operator tables show the IP address.** `MachineRow` dropped `ip` and
  `last_ip` entirely, and the Discovery table never rendered the `ip` field its
  rows already carried. A MAC alone does not tell an operator which physical box
  a row refers to.

### Changed

- **Discovery triage is phrased as "is this an install target?", which is the
  actual question.** The table now leads with hostname, IP, MAC and vendor; the
  category chip explains *why* a device was auto-classified (naming the OUI
  vendor for the phones, watches, speakers and IoT that are never install
  targets); and "Dismiss" is now "Not a target", which is what it always meant.
  The marking is persisted server-side, and a marked device that keeps appearing
  on the segment still updates its last-seen time — waving a device off does not
  make it invisible if it starts behaving unexpectedly.
