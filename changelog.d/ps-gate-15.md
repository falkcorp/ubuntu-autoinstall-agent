<!-- file: changelog.d/ps-gate-15.md -->
<!-- version: 1.0.0 -->
<!-- guid: 9a2e6c4b-1f3d-4b7a-8e0c-5d2f1a9b6c47 -->
<!-- last-edited: 2026-07-24 -->

### Added

#### Merge-blocking equality gate: hand-authored component fixture -> `merge()` == committed config, all 5 hosts (PS-GATE-15)

`crates/uaa-core/tests/component_equality_gate.rs` adds a merge-blocking test
that, for each of the 5 committed hosts (`len-serv-001/002/003`,
`unimatrixone`, `vm-test`), deserializes a hand-authored
`crates/uaa-core/tests/fixtures/components/<host>.yaml` (a `HostGroupProfile`
defaults blob + a `HostProfile` overrides blob) and runs it through
`uaa_core::profile::merge::merge` (which internally lowers via
`profile::lower::lower`), asserting the result equals the host's committed
`InstallationConfig` via canonical-YAML serialization equality — the same
comparison the M2 gate (`test_resolved_equals_committed_by_struct_equality`)
uses. This is deliberately NOT `lower(raise(committed)) == committed`
(tautological): the fixtures are independently hand-authored, mostly as
group-shared defaults plus a thin per-host override.

A second test proves `unimatrixone`'s fixture-authored
`unlock_policy.tpm2_clevis_peer` (an authoring/validate-only nested leaf) is
accepted by the authoring schema but never leaks into the lowered wire
config — the D2-B clevis TPM2 peer share is installer-derived from
`storage_mode`, not a profile input. Both tests run under
`cargo test -p uaa-core`.
