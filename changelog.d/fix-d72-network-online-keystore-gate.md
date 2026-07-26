<!-- file: changelog.d/fix-d72-network-online-keystore-gate.md -->
<!-- version: 1.0.0 -->
<!-- guid: 4d81f0a3-9b27-4c56-8e1a-7f2c6b0d5e93 -->
<!-- last-edited: 2026-07-26 -->

### Fixed

- **NativeKeystore boot no longer races the network before the Tang unlock
  (design item D7.2).** On Ubuntu the keystore LUKS is opened directly by
  `zfs-dracut` (`90zfs/zfs-load-key.sh` → `systemd-cryptsetup attach`, pre-mount
  90), *not* through the crypttab-generated systemd-cryptsetup unit — so the
  `_netdev` crypttab option cannot gate it, and `clevis-luks-askpass` carries no
  `network-online` ordering. The result was the exact intermittent failure seen
  on real hardware: the clevis SSS `t=2` unlock queried Tang before DHCP had
  leased, every share failed, `zfs load-key` never ran, and `sysroot.mount`
  dropped the boot to the dracut emergency shell (virtio's instant DHCP masked it
  in the VM gate). The `91uaa-keystore-wait` dracut hook (pre-mount **89**, which
  dracut runs to completion before the unlock at 90) now also blocks until the
  network is up — `network-online.target` active or a global IPv4 present — when
  `rd.neednet` is set. Name-independent (no dependency on the clevis unit name)
  and provably ordered before the unlock. Verified against the installed
  `90zfs`/`60clevis` dracut modules.
