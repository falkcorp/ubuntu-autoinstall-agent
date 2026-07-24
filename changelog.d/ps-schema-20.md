<!-- file: changelog.d/ps-schema-20.md -->
<!-- version: 1.0.0 -->
<!-- guid: 2f8b4d1a-6c93-4e07-9a52-71d0e3c4b8f6 -->
<!-- last-edited: 2026-07-23 -->

### Added

#### schema_version row gate + component-aware control binary (PS-SCHEMA-20)

Implements the *expand* step of expand-then-migrate for the profile registry.
Adds a separate additive `schema_version: i64` column on `HostGroupRow` and
`HostProfileRow` (`#[serde(default)]`, so a row written before the field existed
reads back as `0`). Defines `SCHEMA_VERSION_MAX = 1` and a fail-loud
`ensure_schema_servable` guard: the control binary now refuses — with the fixed
message `schema version {n} exceeds binary max {MAX}`, and *without* deserializing
the stored blob — to **serve** (`profiles::convert`) or **roll back**
(`profiles::drift::revert_drift`) any row whose `schema_version` exceeds the
binary's max, so a shared-group component blob written by a newer binary can
never be mis-served by an older one. New rows written this phase are stamped
`schema_version = 1`; existing version-0 rows are served normally. Every binary
now recognizes the component keys (`disk_layout`/`unlock_policy`/`network`/
`base_image`/`arch`/`role`/`firmware_quirks`/`hooks`) via the
`deny_unknown_fields` `InstallationConfigPartial`, with unknown keys still
producing a group/host-scoped parse error. This phase migrates ZERO stored
blobs; `schema_version` is excluded from `content_hash`/drift, so the len-serv
`PlainLuks` path stays byte-identical. The no-rollback-below-the-fleet-floor
operational rule is documented on `SCHEMA_VERSION_MAX`.
