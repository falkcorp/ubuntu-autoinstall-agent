<!-- file: changelog.d/clevis-boot-bounded-failclosed.md -->
<!-- version: 1.0.0 -->
<!-- guid: 2f0c8a41-77b5-4f0e-9a3e-1c6d5b0e4a92 -->
<!-- last-edited: 2026-08-03 -->

### Fixed

- `scripts/vm-gate/verify-initramfs.sh` no longer reports a false
  `MISSING module 50clevis-pin-pkcs11 ... DO NOT REBOOT` for an initramfs that
  actually contains the pin. `lsinitrd -m` prints dracut module names without
  their numeric directory prefix, so the old `grep -qx "50clevis-pin-pkcs11"`
  could never match. Module names are now compared with the leading digits
  stripped from both sides. The old fallback (`grep -q "$needle" "$LISTING"`)
  was an unanchored substring search over the whole file listing and could
  report a module present purely because a hook path mentioned it; it is now
  used only when `lsinitrd -m` yields nothing, and is anchored to a
  `modules.d/` path component.

### Added

- `verify-initramfs.sh --pin pkcs11` now requires `libpcsclite_real.so.1`.
  Ubuntu's `libpcsclite.so.1` is a dlopen shim; dracut's `50clevis-pin-pkcs11`
  installs only the shim, so the initramfs printed
  `loading "libpcsclite_real.so.1" failed` / `No slots.` and pcscd could
  enumerate no readers. Measured 2026-08-03 in the root-LUKS gate VM.
