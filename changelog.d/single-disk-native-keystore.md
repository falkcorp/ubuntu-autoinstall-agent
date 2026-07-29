<!-- file: changelog.d/single-disk-native-keystore.md -->
<!-- version: 1.0.0 -->
<!-- guid: 3f7a29c4-8b16-4e52-9d07-2c5e8a1f6b30 -->
<!-- last-edited: 2026-07-29 -->

### Added

#### NativeKeystore now supports single-disk hosts

`storage_mode: native-keystore` (ZFS **native** encryption keyed from a LUKS
keystore zvol, unlocked by clevis/Tang D2-B) was previously welded to the
four-disk U1 topology: `plan_layout` hard-required at least two `system` disks
**and** at least two `special` disks, so a host with one NVMe failed closed and
could only use `plain-luks` (LUKS *under* ZFS — a different architecture).

Pool topology is now derived from the disk roster instead of hardcoded:

- **1 `system` disk** → single (unmirrored) `bpool`/`rpool` vdevs; **2 or more**
  → mirrors, exactly as before.
- **0 `special` disks** → the `special` vdev is omitted entirely; **2 or more**
  → `special mirror`.
- **exactly 1 `special` disk** → still rejected, because an unmirrored metadata
  vdev makes a single device failure fatal to the whole pool.
- **0 `system` disks** → still rejected; there is nothing to install onto.

A single-disk host gets the same partition trio (ESP + bpool + data), the same
`rpool/keystore` LUKS2 zvol, the same native encryption on `rpool/ROOT` +
`rpool/USERDATA`, and the same Phase-5 clevis D2-B enrollment — it simply has no
redundancy. No config-schema change was needed: a single-disk roster is one
`disks:` entry with `role: system`.

The mirrored rendering is byte-identical to the previous code (`mirror
A-partN B-partN`, `… special mirror C-part1 D-part1`), so the hardware-validated
U1 install is unchanged; a dedicated test asserts that exact string. Emitting
the `mirror` keyword for a lone device would be a `zpool` syntax error, so the
single-vdev distinction is correctness rather than cosmetics.
