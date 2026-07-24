### Added

#### Profile system: wire arch/role/firmware_quirks/hooks onto InstallationConfig

`InstallationConfig` now carries four additive fields — `arch`, `role`,
`firmware_quirks`, and `hooks` — each `skip_serializing_if` its type's default
so an omitting host (every committed host today) serializes byte-identically
to before this change. `Hooks` gained an `is_empty()` predicate for that
purpose. No installer behavior consumes these fields yet; this is
wire-integration only (PS-WIRE-AXES-10).
