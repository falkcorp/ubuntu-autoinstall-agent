<!-- file: changelog.d/clevis23-pkcs11-pinning.md -->
<!-- version: 1.0.0 -->
<!-- guid: 4e81b0d7-2a5c-4f19-8b63-9d70c4a1e582 -->
<!-- last-edited: 2026-08-02 -->

### Added

#### Opt-in clevis 23 from the 26.10 pocket, for the pkcs11 (YubiKey PIV) pin

New `clevis_pkcs11_pin` flag on `InstallationConfig`, **off by default**. Off is
byte-identical to previous behaviour — len-serv-001/002 are unaffected, and an
off flag is not even serialized into a resolved config.

The clevis pkcs11 pin (a YubiKey PIV applet as a LUKS unlock factor) does not
exist in the clevis Ubuntu 26.04 ships: `20-1ubuntu2` from `resolute/universe`
answers `'pkcs11' is not a valid pin!`, and clevis is not in
`resolute-backports`. The first clevis with the pin is `23-1build1`, in 26.10.

When the flag is on, the installer writes a deb822 source for the 26.10 pocket
plus a deliberately narrow `/etc/apt/preferences.d/99-uaa-clevis23-pkcs11` — an
explicit allowlist (the five lockstep-versioned clevis binaries, `libssl4`, and
`openssl-provider-legacy`) at priority 501, and a `Package: *` catch-all for the
same release at -1 so the base system can never be dragged to 26.10. Both files
are written *before* `apt update`, on the live host (where `clevis luks bind`
runs) and inside the target chroot (which needs `clevis-dracut`'s
`50clevis-pin-pkcs11` module and `clevis-systemd`'s askpass unit to unlock at
boot). `opensc` and `pcscd` are installed from plain 26.04 universe, unpinned.

Both generated files come from pure functions in `packages.rs` with unit tests,
including one asserting the pin matches nothing outside the intended set.

**Open risk, not solved:** `libssl4` pulls the 26.10 `openssl-provider-legacy`,
which is a single package name and therefore displaces the 26.04 one the
installed OpenSSL 3.5 stack uses. Documented with the exact checks that would
settle it in `docs/research/2026-08-02-clevis23-pkcs11-pinning-risk.md`. This is
why the flag is off by default.
