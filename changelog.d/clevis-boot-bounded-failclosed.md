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

### Added

- `dracut/92uaa-unlock-deadline` — a dracut module that puts a hard deadline on
  the whole initramfs (`JobTimeoutSec=` + `JobTimeoutAction=reboot-force` as a
  drop-in on `initrd.target`) so a clevis unlock that cannot be satisfied
  **reboots and retries** instead of sitting at an interactive prompt forever.
  A 902 s unbounded hang was measured previously with both
  `x-systemd.device-timeout=90` and `rd.timeout=120` set; neither governs an
  ask-password wait. Requires `rd.shell=0 rd.emergency=reboot` on the kernel
  cmdline — without it dracut's emergency shell cancels the `initrd.target` job
  and the deadline never fires. Boot-proven in a root-on-LUKS VM.
- `docs/research/2026-08-03-clevis-initramfs-bounded-failclosed.md` — the
  measurements behind that module, plus proof that interactive PKCS#11 PIN
  entry **does** work for the root device in the initramfs, and two upstream
  clevis bugs found on the way.

### Measured

- `clevis-luks-pkcs11-askpin` calls `systemd-ask-password` with no `--timeout`,
  so each PIN query dies after systemd's 90 s default and clevis counts the
  empty answer against `too_many_errors=3`. With zero keystrokes sent, a boot
  fails at 99 s / 191 s / 283 s and prints `Too many errors !!!`, then falls
  through to the unbounded plain-passphrase prompt. An operator therefore has
  ~90 s per prompt, not the length of the outer deadline.
- Both the PKCS#11 PIN query and the plain LUKS passphrase query are live on the
  same console during cold recovery; keystrokes land on whichever systemd is
  currently displaying.
