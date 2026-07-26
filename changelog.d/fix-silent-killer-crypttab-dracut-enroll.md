<!-- file: changelog.d/fix-silent-killer-crypttab-dracut-enroll.md -->
<!-- version: 1.0.0 -->
<!-- guid: 9c4e1b62-3a75-4f80-9d12-6e0a7b5c3d18 -->
<!-- last-edited: 2026-07-26 -->

### Fixed

- **Three more "silent killer" swallowed failures on the encrypted-install path
  are now fatal.** Each one previously let the installer report success on a box
  that could not decrypt at boot:
  - The `/etc/crypttab` write (both the NativeKeystore keystore path and the
    PlainLuks root path) was `let _ = …` — a failed write left the target with no
    unlock unit at all.
  - The `/etc/dracut.conf.d/90-uaa-crypt.conf` write was `let _ = …` — a failed
    write shipped an initramfs with none of the crypt/tpm2/zfs/network unlock
    modules (and no forced NIC driver for Tang).
  - Clevis Tang enrollment skipped silently (`return Ok(())`) if the passphrase
    tempfile could not be created or written, bypassing the already-fatal bind and
    leaving the keystore with no unattended-unlock binding.
