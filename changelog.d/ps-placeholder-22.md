<!-- file: changelog.d/ps-placeholder-22.md -->
<!-- version: 1.0.0 -->
<!-- guid: 8c2e4a7f-3d1b-4f6a-9e0c-5b7d1a2f9c4e -->
<!-- last-edited: 2026-07-24 -->

### Added

#### Placeholder-survival test harness for parse->merge (PS-PLACEHOLDER-22)

New integration test `crates/uaa-core/tests/placeholder_survival.rs` proves
that the literal `REPLACE_AT_PLACE_TIME` secret placeholder survives
`parse(YAML) -> merge()` unchanged for every current secret-bearing flat
`InstallationConfig` field: `tpm2_pin`, `luks_key`, `root_password`,
`install_ca_cert`. Exposes a reusable
`assert_placeholder_survives(field_name, group, host, extract)` helper so
future per-field migration briefs (e.g. UNLOCK-27, DISK-28) can reuse it for
their own secret field once it moves into a nested component, instead of
re-deriving the assertion.
