<!-- file: CHANGELOG.md -->
<!-- version: 1.0.0 -->
<!-- guid: chnglog1-0000-0000-0000-000000000001 -->
<!-- last-edited: 2026-06-20 -->

# Changelog

All notable changes to `ubuntu-autoinstall-agent` are documented here.

## [Unreleased] — feat/install-subcommands

### Added

- **Unified `install` subcommand** (`src/cli/args.rs`, `src/cli/commands.rs`, `src/main.rs`)
  - `ubuntu-autoinstall-agent install [--remote <host>] [--username user] [--config file.yaml]`
  - Without `--remote`: runs installation on the local live system
  - With `--remote <host>`: SSH into the target and run the full pipeline there
  - Dispatches to the existing SSH/local machinery; no duplicate logic

- **`--config <file>` for `ssh-install`** — pass a YAML `InstallationConfig` instead of always
  defaulting to the hard-coded `for_len_serv_003()` config

- **Dracut initramfs support** (`src/network/ssh_installer/config.rs`,
  `src/network/ssh_installer/system_setup.rs`)
  - `InitramfsType` enum (`Dracut` | `InitramfsTools`); default is `Dracut`
  - `initramfs_type.regenerate_cmd()` returns the correct shell command
  - `configure_zfs_in_chroot` and `setup_luks_key_in_chroot` call the right regeneration command
  - Installs `dracut dracut-network` in the target chroot when `Dracut` is selected

- **Tang/Clevis SSS enrollment** (`src/network/ssh_installer/system_setup.rs`)
  - `enroll_tang_clevis` builds the SSS JSON `{"t":N,"pins":{"tang":[…]}}` and runs
    `clevis luks bind` on the LUKS partition
  - Installs `clevis clevis-tang clevis-luks clevis-dracut` (or `clevis-initramfs`) in chroot
  - Failure is non-fatal — passphrase fallback keyslot remains in place

- **`rd.neednet=1 ip=dhcp` in GRUB** — `configure_grub_in_chroot` appends these kernel parameters
  when `initramfs_type == Dracut` and at least one Tang server is configured, enabling network
  access during initramfs boot for Clevis Tang unlock

- **`tang_servers`, `tang_threshold`, `ssh_authorized_keys` fields on `InstallationConfig`**
  - All fields are serde-deserializable; `for_len_serv_003()` seeds all three Tang server URLs

- **SSH authorized-key injection** — `configure_system_in_chroot` writes
  `config.ssh_authorized_keys` into `/mnt/targetos/root/.ssh/authorized_keys`

### Changed

- **`SshInstaller` refactored to `Box<dyn CommandExecutor>`** (`src/network/ssh_installer/installer.rs`)
  - Replaced the previous `ssh: SshClient + local: LocalClient + mode: ExecutionMode` triple
  - `connect()` creates and boxes an `SshClient`; `connect_local()` boxes a `LocalClient`
  - All six installation phases route through `self.runner`, eliminating the SSH-only bug in
    local mode

- **Sub-managers now accept `&mut dyn CommandExecutor`** (all four sub-manager files)
  - `DiskManager`, `PackageManager`, `SystemConfigurator`, `ZfsManager` all changed from
    `&mut SshClient` to `&mut dyn CommandExecutor`
  - `SystemInvestigator` generic type parameter gained `?Sized` to accept trait objects

- **SSH auth fallback** (`src/network/ssh.rs`)
  - After `userauth_agent()` failure, tries `~/.ssh/id_ed25519`, `~/.ssh/id_rsa`,
    `~/.ssh/id_ecdsa` before returning an error

- **`preflight_checks` fixed for local mode** — now uses `self.runner` (was `self.ssh` regardless
  of mode)

### Fixed

- `reqwest` pinned to `0.12` (0.13 dropped the `rustls-tls` feature flag)
- Missing struct fields in `create_local_installation_config` (`initramfs_type`, `tang_servers`,
  `tang_threshold`, `ssh_authorized_keys`)
- `investigation.rs` `SystemInvestigator<T>` → `SystemInvestigator<T: ?Sized>` to accept
  `dyn CommandExecutor`
- All stale `self.ssh` references replaced with `self.runner` across all sub-manager files

### Infrastructure context preserved in code

- Tang servers: `http://172.16.2.45`, `http://172.16.2.46`, `http://172.16.2.47` (SSS t=2)
- Servers: len-serv-001 (172.16.3.92), len-serv-002 (172.16.3.94), len-serv-003 (172.16.3.96)
- Initramfs: dracut + `rd.neednet=1 ip=dhcp`
