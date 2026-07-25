<!-- file: changelog.d/fix-zfs-list-cache-and-serial-com2.md -->
<!-- version: 1.0.0 -->
<!-- guid: 8c3f1d92-4a6b-4e17-9f2c-6b0e1a5d3f74 -->
<!-- last-edited: 2026-07-25 -->

### Fixed

- **NativeKeystore install no longer boots to the systemd maintenance shell.**
  The installer populated `/etc/zfs/zfs-list.cache/{rpool,bpool}` with a
  `timeout 5 zed -F` that was a no-op (zed only writes the cache on a zpool
  *event*, and it ran inside the chroot blind to the host-imported pools), so the
  cache files were **empty** — the zfs-mount-generator then produced no mount
  units and `/var`, `/var/log`, … never mounted, dropping boot to maintenance
  even though the D2-B unlock succeeded. Now generated deterministically via
  `zfs list` in the exact cacher format. Confirmed on U1: empty cache was the
  maintenance cause.
- **Serial console now lands on the port IPMI SOL actually reads.** The grub
  drop-in used `console=ttyS0` / `--unit=0` (COM1), but Supermicro X10 BMC SOL is
  **COM2 / ttyS1** — so boot, LUKS-unlock and emergency output were invisible over
  SOL. Now emits `console=ttyS0` **and** `console=ttyS1` (ttyS1 last = primary
  /dev/console) with GRUB serial `--unit=1`.
