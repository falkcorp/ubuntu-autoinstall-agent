<!-- file: todo.d/2026-08-03-clevis-pkcs11-upstream-bugs.md -->
<!-- version: 1.0.0 -->
<!-- guid: 9e5b7c02-4a16-4d83-8f2b-70c1a9e34d65 -->
<!-- last-edited: 2026-08-03 -->

- [ ] clevis 23-1 `clevis_detect_pkcs11_device` never tries the URI's
      `module-path=` in **dracut mode**, so a token needing a non-default
      PKCS#11 module cannot be detected at root-unlock time. Report upstream /
      carry a patch. See
      `docs/research/2026-08-03-clevis-initramfs-bounded-failclosed.md`.
- [ ] dracut `50clevis-pin-pkcs11` installs Ubuntu's `libpcsclite.so.1` dlopen
      shim but not `libpcsclite_real.so.1`, so pcscd enumerates no readers in
      the initramfs. Add it via `install_items` on real-token hosts and verify
      against an actual YubiKey — the SoftHSM gate cannot prove this half.
- [ ] Decide the initramfs generator for rpi-serv (.45/.46/.47) before applying
      the `92uaa-unlock-deadline` module there: under `initramfs-tools` there is
      no `initrd.target` and none of the bounding mechanism applies.
