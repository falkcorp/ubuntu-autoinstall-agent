<!-- file: changelog.d/ps-cockroach-16.md -->
<!-- version: 1.0.0 -->
<!-- guid: 9d3f7a21-5c6b-4e8a-9b2d-1f0e6c4a8d72 -->
<!-- last-edited: 2026-07-24 -->

### Changed

#### Cockroach advertise/join now derived from the group roster, not a hardcoded constant (PS-COCKROACH-16)

`InstallationConfig` gains `cockroach_members: Vec<String>` (`skip_serializing_if`
empty, so every host without a Cockroach application — the entire committed
fleet today — serializes byte-identically). `ApplicationInstaller::install_cockroach`
now derives the CockroachDB advertise/join strings from `config.cockroach_members`
via the existing `derive_cockroach_endpoints`, instead of the hardcoded
`host_spec::LENSERV_MEMBER_IPS` constant, which is retired. `uaa-control`'s
`resolve_from_registry` populates `cockroach_members` from the target host's
group's currently-active hostname allocations (resolving each sibling's own
`network_address` via `merge()`, so a per-host static-IP override is
respected) whenever the resolved config carries a Cockroach application.

`HostSpec::for_lenserv` now takes an explicit `members: &[&str]` roster
parameter instead of defaulting it internally; all `place`/`verify`/
`render-user-data` CLI callers and test callsites were updated accordingly.

A new gate test proves the roster-derived (advertise, join) for each of the
3 len-serv nodes equals the exact literal join strings the retired constant
produced, and an integration test proves `uaa-control` populates the roster
correctly from a synthetic 3-host group allocation, end to end through
`derive_cockroach_endpoints`.
