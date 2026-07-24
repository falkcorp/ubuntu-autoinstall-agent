<!-- file: changelog.d/ps-app-09.md -->
<!-- version: 1.0.0 -->
<!-- guid: 7b6f0a3d-1c9e-4f2a-9d5b-3e8c6a4f0d21 -->
<!-- last-edited: 2026-07-23 -->

### Added

#### `TangServer` application variant (PS-APP-09)

`ApplicationSpec` gains a `TangServer(TangServerSpec)` variant alongside
`Cockroach`, so a host/group profile can author `kind: tang-server` with a
`port` (default `80`, the fleet's standing Tang port) and a required
`key-directory` (e.g. `/etc/tang/keys`). This is expressibility-only for
now: `ApplicationInstaller`'s dispatch treats `TangServer` as a no-op skip
(`Ok(())` with a `tracing::warn!` naming the host) rather than an error or
panic — no `tang-server` applier exists yet, since rpi Tang hosts are
provisioned outside this installer today. `reject_duplicates` extends the
same duplicate-kind guard used for `cockroach` to `tang-server`. The
existing `Cockroach` path, len-serv `PlainLuks` behavior, and every other
installer phase are unchanged.
