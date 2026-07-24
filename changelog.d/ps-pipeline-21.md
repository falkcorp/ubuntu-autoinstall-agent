<!-- file: changelog.d/ps-pipeline-21.md -->
<!-- version: 1.0.0 -->
<!-- guid: 8f4a1c6e-2d9b-4e37-9c1a-7b5d3f0e6a92 -->
<!-- last-edited: 2026-07-24 -->

### Added

#### Wire validate_resolved into resolve_from_registry (PS-PIPELINE-21)

`resolve_from_registry` now calls `uaa_core::profile::validate::validate_resolved`
on the merged `InstallationConfig` at both resolution sites (the indexed
hostname-allocation path and the `hostname_override` fallback) before
returning it — a config that is legal field-by-field but an illegal
combination once flattened (e.g. `NativeKeystore` with an empty disk roster)
now fails resolution instead of reaching `config_place`. Added tests proving
a component-authored group+host fixture (nested `network`/`base_image`
blocks) resolves through the full registry path to the same
`InstallationConfig` the underlying `merge()` call produces and passes
`validate_resolved`; that an illegal resolved combination is rejected with
the `validate_resolved` message; and that existing flat-authored resolution
is unaffected by the new gate.

### Changed

#### uaa-control

- `crates/uaa-control/src/profiles/resolve.rs` — post-merge
  `validate_resolved` gate + 3 new tests (component fixture, illegal
  combination, flat-authored regression).
