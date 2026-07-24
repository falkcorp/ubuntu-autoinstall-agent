<!-- file: changelog.d/ps-wire-partial-11.md -->
<!-- version: 1.0.0 -->
<!-- guid: 6d8f2f2c-4a3b-4a51-9b7e-2b6f0c6e5a3d -->
<!-- last-edited: 2026-07-23 -->

### Added

#### Wire component sub-structs onto InstallationConfigPartial (PS-WIRE-PARTIAL-11)

Added four nested authoring fields to `InstallationConfigPartial`
(`disk_layout: Option<DiskLayoutPartial>`, `unlock_policy: Option<UnlockPolicyPartial>`,
`network: Option<NetworkConfigPartial>`, `base_image: Option<BaseImagePartial>`), plus
the wire axes `arch: Option<Arch>`, `role: Option<HostRole>`,
`firmware_quirks: Option<Vec<FirmwareQuirk>>`, and `hooks: Option<Hooks>`. All
existing flat fields are retained unchanged; merge/lower reconciliation is
deferred to PS-MERGE-13/PS-LOWER-12. Also defines `DiskLayoutPartial`, the
tagged-select wrapper enum (`SingleLuks`/`ZfsNativeKeystore`) over the
per-variant partials PS-DISK-01 shipped, since PS-DISK-01 only shipped the
specs. The manual `PartialEq` impl and `test_partial_all_none_is_legal` are
extended to cover the new fields. Purely additive — the len-serv `PlainLuks`
path is unaffected.
