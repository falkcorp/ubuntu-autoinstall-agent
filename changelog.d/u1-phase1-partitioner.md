<!-- file: changelog.d/u1-phase1-partitioner.md -->
<!-- version: 1.0.0 -->
<!-- guid: 6f0a2c14-8b3d-4e57-9a1c-2d4e6f8b0a12 -->
<!-- last-edited: 2026-07-22 -->

### Added

#### U1 Phase 1: NativeKeystore storage schema + pure partition planner

First implementation phase of the U1 ZFS-native-encryption plan. Adds a
`storage_mode` discriminator (`plain-luks` default | `native-keystore`) and a
role-tagged `disks` roster (`DiskSpec { id, role }`, by-id devices only) to
`InstallationConfig`. Both fields are `skip_serializing_if`-guarded, so every
stock Lenovo (`PlainLuks`) host serializes byte-for-byte as before — a
rolled-back control binary with a `deny_unknown_fields` config still parses the
placed file. Serde guard tests assert both directions (PlainLuks omits the keys;
NativeKeystore emits them).

Adds `network/ssh_installer/layout.rs` — a **pure** partition planner (no I/O):
given the disk roster it computes `ESP (1 GiB) + bpool (2 GiB) + special (rest)`
on each Optane (`system`) disk and whole-disk data-vdev members on each SSD
(`data`) disk, with load-bearing partition numbers (p1/p2/p3), GPT typecodes
matched to the proven fleet path (`EF00`/`BE00`, plus `BF00` for the native-ZFS
`special` member), and per-disk-unique labels. Fail-closed validation rejects any
roster that cannot form the design's mandated mirrors (fewer than two disks of a
role) or that carries an empty/duplicate device id.

The `partitions.rs` applier that turns a plan into `sgdisk` calls is deferred to
a later phase — it needs a bootable artifact the VM gate can exercise, so Phase
1 is planner + assertions only, no VM.
