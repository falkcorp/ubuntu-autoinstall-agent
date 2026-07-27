<!-- file: changelog.d/provision-operator-user-ssh-installer.md -->
<!-- version: 1.0.0 -->
<!-- guid: 7b2c9e14-3d6a-4f81-9c02-5a8e1f0d4b6c -->
<!-- last-edited: 2026-07-27 -->

### Added

#### SSH/native installer now provisions operator user accounts

The SSH/native install path (the one U1/native-keystore uses) previously
provisioned **root only** — every login account had to be created by hand after
install. It now honors an optional `users:` block in `InstallationConfig`, the
native-install analogue of the `users:` stanza the len-serv cloud-init path
already applies, so both paths yield the same operator account.

Each entry creates the account in the chroot (idempotent `useradd -m`), sets a
password via `chpasswd` (empty password locks the account for SSH-key-only
login), adds it to supplementary groups — default `adm, sudo, cdrom, dip, lxd,
docker`, each guarded by `getent` so a group the target lacks is skipped rather
than aborting — and seeds its own `~/.ssh/authorized_keys`. Membership in `sudo`
grants password-prompted `sudo`; no NOPASSWD sudoers file is written. The field
is `skip_serializing_if = "Vec::is_empty"`, so a user-free config stays
byte-identical on the wire and parses on an older `uaa` binary, matching the
forward-compat contract `applications` uses.
