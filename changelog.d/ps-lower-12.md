### Added

#### Profile system: pure authoring->flat-wire bridge (`lower()`)

New `crates/uaa-core/src/profile/lower.rs` with `lower(&InstallationConfigPartial) -> InstallationConfig`:
a pure, total function that flattens a resolved (post-merge) authoring
partial's nested component blocks (`network`, `base_image`, `unlock_policy`,
`disk_layout`) onto the flat wire fields the installer pipeline consumes,
falling back to the existing flat fields when a nested component (or leaf) is
absent so a purely flat-authored host — every len-serv host today — still
lowers byte-identically. Preserves the `tpm2_pin`/`debootstrap_release`/
`debootstrap_mirror` double-Option inherit-vs-explicit-none distinction and
leaves `REPLACE_AT_PLACE_TIME` secret placeholders untouched. Drops the
authoring-only `disk_layout` size fields, `base_image.fallback_mirror`, and
`unlock_policy.tpm2_clevis_peer`, none of which have a wire-field home yet
(PS-INSTALLER-29).
