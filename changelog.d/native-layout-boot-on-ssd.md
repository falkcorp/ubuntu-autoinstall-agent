### Changed

#### NativeKeystore layout: boot moves to the SATA SSDs, Optanes become half-disk special

The X10DSC+ firmware cannot boot from NVMe (its UEFI boot manager has no NVMe
entry — proved with a clean ext4 test install that would not boot off the
Optane). The `StorageMode::NativeKeystore` layout is revised accordingly:

- **System role** is now the bootable SATA SSDs, carrying `ESP + bpool + rpool
  data` (p1/p2/p3). Boot lives on a disk the firmware can enumerate.
- **Special role** is the Optanes, each contributing a **half-disk** `special`
  (metadata) vdev member (p1). The other half is left unpartitioned, reserved
  for a future spinning-disk array's special vdev.
- `special_small_blocks=0` stays (metadata only) — the data pool is itself SSD,
  so there is no small-file latency to offload onto the Optane.

`DiskRole::Data` is renamed to `DiskRole::Special` (YAML role value `data` →
`special`); the `unimatrixone.yaml` roster and the design spec are updated to
match. The Lenovo PlainLuks path is untouched.
