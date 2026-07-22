<!-- file: changelog.d/u1-phase2-native-pool.md -->
<!-- version: 1.0.0 -->
<!-- guid: 3c70b2bd-3a25-43de-a5d5-85c7e32deabc -->
<!-- last-edited: 2026-07-22 -->

### Added

#### U1 Phase 2: NativeKeystore multi-disk partitioner + native pool/keystore builder

Codifies the ZFS native-encryption install path (the future standard "server
profile") hand-validated on real U1 hardware, selected by
`storage_mode == NativeKeystore`:

- **`disk_native.rs`** — the multi-disk partitioner, the applier that turns
  Phase 1's pure `layout::plan_layout` into real `sgdisk`: wipes every disk in
  the roster, partitions the System (Optane) disks (`ESP + bpool + special`) and
  leaves the Data (SSD) disks whole-disk for the `rpool` mirror.
- **`zfs_native.rs`** — builds `bpool` (mirror of Optane p2, GRUB-compatible),
  `rpool` (data mirror + special metadata mirror, root **unencrypted**,
  `special_small_blocks=0`), the `rpool/keystore` zvol → LUKS2 → `system.key`,
  and the encrypted `rpool/ROOT`+`rpool/USERDATA` encryptionroots (keyed from the
  keystore file) plus the stock Ubuntu dataset tree. Preserves the
  `variables["UUID"]` contract so Phase 4/5 keep resolving
  `rpool/ROOT/ubuntu_<uuid>`.
- **`installer.rs`** — `phase_2_disk_preparation` / `phase_3_zfs_creation` now
  `match config.storage_mode`: `PlainLuks` is the unchanged single-disk Lenovo
  path; `NativeKeystore` runs the new managers.
- **`unimatrixone.yaml`** — rewritten from the dropped IMSM/mdadm single-disk
  layout to the NativeKeystore 4-disk roster (2 Optane system + 2 SSD data,
  by-id) with the D2-B policy flags (`enroll_tpm2: false`, `expect_fido2: false`,
  Tang t=2).

Unit-tested (sgdisk command strings match the validated hand-run; keystore key
paths agree; the example-config round-trip validates the new U1 config). The
Phase 5 boot wiring (clevis D2-B bind of the keystore LUKS, crypttab, the
`91uaa-keystore-wait` dracut hook) and the VM-gate boot-proof follow.
