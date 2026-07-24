### Added

#### Disk-layout component types + per-variant partials (PS-DISK-01)

New `crates/uaa-core/src/network/ssh_installer/components/disk_layout.rs`
defines `DiskLayout`, a tagged (`kind`, kebab-case, `deny_unknown_fields`)
enum with two variants: `SingleLuks(SingleLuksSpec)` — the live Lenovo
(`len-serv*`) single-disk ESP/RESET/BPOOL/LUKS layout, whose size defaults
(`esp_size: "512M"`, `reset_size: "4G"`, `bpool_size: "2G"`) match the
sgdisk literals in `disk_ops.rs` exactly — and `ZfsNativeKeystore(NativeKeystoreSpec)`,
which reuses `DiskSpec`/`DiskRole` verbatim for the multi-disk Supermicro
`unimatrix*` roster. Per-variant partials (`SingleLuksSpecPartial`,
`NativeKeystoreSpecPartial`) mirror `CockroachSpecPartial`'s all-`Option`
shape for future profile-layer overrides.

This is authoring-types only: nothing is wired onto `InstallationConfig` or
`InstallationConfigPartial`, there are no appliers, and the len-serv
`PlainLuks` install path is unchanged. Wiring `disk_ops.rs` to read these
values is deferred to PS-INSTALLER-29.
