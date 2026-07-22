<!-- file: changelog.d/nativekeystore-recovery-and-timezone.md -->
<!-- version: 1.0.0 -->
<!-- guid: 9e2c7a41-6b83-4d50-a12f-7c0e3d8b5a64 -->
<!-- last-edited: 2026-07-22 -->

### Fixed

#### NativeKeystore re-install recovery hang + non-fatal live-env timezone

Two issues found running the U1 install on real hardware:

- **Recovery hang:** the preflight residual-recovery + `cleanup_existing_mounts`
  ran `zpool export` before closing the `keystore-rpool` mapper, which holds
  `/dev/zvol/rpool/keystore` open — so the export blocked forever, wedging a
  re-install of a partially-installed NativeKeystore host. Now the keystore is
  unmounted + closed before every pool export, and the mapper-close sweep matches
  `keystore` too. No-op on PlainLuks.
- **Live-env timezone:** `timedatectl set-timezone` / `set-ntp` are now
  best-effort — they set only the ephemeral live-env clock (the installed
  system's TZ is written in-chroot), but timed out on U1's live ISO and failed
  the whole install. No longer fatal.
