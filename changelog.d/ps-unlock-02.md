<!-- file: changelog.d/ps-unlock-02.md -->
<!-- version: 1.0.0 -->
<!-- guid: 199990c6-4181-4e93-b485-174f8a97cd06 -->
<!-- last-edited: 2026-07-23 -->

### Added

#### Unlock-policy authoring sub-struct (PS-UNLOCK-02)

Added `UnlockPolicyPartial` (with nested `TangSssPartial` and `Tpm2PinPartial`)
under `crates/uaa-core/src/profile/components/unlock_policy.rs`, grouping the
profile-authoring fields for disk unlock (`tang`, `tpm2_pin`, `tpm2_clevis_peer`,
`fido2_expected`) that today live flat on `InstallationConfigPartial`. This is
authoring-types only — no wiring onto `InstallationConfigPartial` and no
merge/lower logic; a future brief wires it in and lowers it to the existing
flat wire fields (mapping documented in the new module). `TangServer` also now
derives `PartialEq`, unblocking `PartialEq` on any struct holding
`Vec<TangServer>`; the hand-written `tang_servers` comparison in
`profile/mod.rs` is unchanged.
