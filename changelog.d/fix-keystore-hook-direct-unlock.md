<!-- file: changelog.d/fix-keystore-hook-direct-unlock.md -->
<!-- version: 1.0.0 -->
<!-- guid: 2a6f9c41-8d03-4b7e-9f15-3c8a1e6b7d20 -->
<!-- last-edited: 2026-07-26 -->

### Fixed

- **NativeKeystore now unlocks the keystore itself in the dracut hook instead of
  relying on the fragile systemd ask-password path.** On Ubuntu the keystore is
  opened via `systemd-cryptsetup attach` + `clevis-luks-askpass` answering an
  ask-password prompt in the initramfs — which does not fire reliably, so the
  boot hangs silently waiting for a passphrase nobody supplies. The
  `91uaa-keystore-wait` hook (pre-mount 89, before zfs-load-key.sh at 90) now
  runs the proven manual sequence directly: `clevis luks unlock` → mount the
  keystore → `zfs load-key`. Stock `zfs-load-key.sh` then sees the key already
  loaded and no-ops the askpass path. The keystore `/etc/crypttab` entry is
  removed (it only generated a competing, equally-broken `systemd-cryptsetup@`
  unit that could win the race and hang first).
- **`/etc/hostid` is now written correctly.** The old step ran `zgenhostid -f
  /etc/hostid` inside the target chroot — malformed, and its fallback wrote the
  ASCII text `00000000`, never matching the pool's hostid. It now runs on the
  live host and writes the live-env hostid (the value the pools were created
  under) as a proper 4-byte binary file.
