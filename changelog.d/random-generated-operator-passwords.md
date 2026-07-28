<!-- file: changelog.d/random-generated-operator-passwords.md -->
<!-- version: 1.0.0 -->
<!-- guid: 9a1e4c76-2b83-4f5d-8c10-6d92f0e7b34a -->
<!-- last-edited: 2026-07-27 -->

### Added

#### Installer can generate random per-host passwords instead of storing them

The SSH/native install path can now generate a strong random password at install
time rather than reading a cleartext one from the config. Write the sentinel
`!random` in place of any `root_password` or operator-user `password` in the
install YAML; the install driver replaces it with a freshly generated,
per-host password (20 chars from an unambiguous charset, drawn from `ring`'s
`SystemRandom` with rejection sampling — no modulo bias), applies it in the
target via the existing base64→`chpasswd` path, and records the generated value
in a `0600` file at `/var/lib/uaa/credentials/<host>.txt` on the machine that
ran the install, as well as printing it to that terminal.

The generated value is written to its `0600` file **before** the risky install
phases run (write-ahead), so a password that gets applied and then lost to a
later-phase failure is still recoverable from disk. Resolution only happens when
the password-applying phase (Phase 5) will actually run, so a partial/phased run
never reports a password it did not set.

Why: a password in a checked-in/placed config is a secret at rest that can leak,
and one literal password reused across a fleet means a single leak compromises
every host. Generating unique per-host passwords whose only at-rest copy is a
root-only file on the driver removes both problems. The webhook status feed is
deliberately **not** used as a sink (cleartext HTTP into a display feed would be
worse than the config).

Root's password now also flows through the base64→`chpasswd` path (previously it
was interpolated raw into the chroot command), which is required for a generated
password to apply safely and fixes a latent shell-injection/breakage bug for any
root password containing shell metacharacters. An empty `root_password` now locks
the root account (key/console-only) instead of setting a passwordless root.

The feature is strictly opt-in per field: a config with no `!random` sentinels is
byte-for-byte unchanged, so the len-serv cloud-init path and all literal-password
configs are unaffected.
