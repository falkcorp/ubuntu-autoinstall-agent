<!-- file: changelog.d/ps-quirk-05.md -->
<!-- version: 1.0.0 -->
<!-- guid: 8b1a4e2d-3f6c-4a9b-8e1d-2c5f7a0b9d3e -->
<!-- last-edited: 2026-07-23 -->

### Added

#### Firmware-quirks closed enum + Vec type (PS-QUIRK-05)

Added `FirmwareQuirk`, a new closed tagged enum in a new
`ssh_installer::components` module tree
(`crates/uaa-core/src/network/ssh_installer/components/firmware_quirks.rs`),
part of the profile-system authoring-types conversion. It models per-board
firmware workarounds — `GrubRemovableFallback`, `ForceNicDriver { driver }`,
`WatchdogStaggered { slot, interval_secs }` — as a variant-select
(union-by-kind) component intended to be carried as `Vec<FirmwareQuirk>` on a
future profile/host type. Deliberately excludes serial-console (arch-gated
installer default, see PS-SERIAL-18) and nvme-cant-boot (stays modeled via
`DiskRole::System`); the watchdog-staggering params are greenfield stubs for
the rpi Tang watchdog and gate no behavior yet (see PS-MIG-RPI-24). This is
purely additive: no `InstallationConfig` wiring, no host-behavior change, and
the len-serv PlainLuks path is untouched.
