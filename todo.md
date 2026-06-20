# todo.md — ubuntu-autoinstall-agent
# version: 1.1.0
# guid: todo0001-0000-0000-0000-000000000001
# last-edited: 2026-06-20

## Critical Bugs (blocking correct operation)

- [x] **CommandRunner trait** — all sub-managers (`DiskManager`, `PackageManager`, `SystemConfigurator`,
  `ZfsManager`) are hardwired to `&mut SshClient`. Local install mode is completely broken at Phase 4+
  because local execution is never plumbed through. Fix: add `CommandRunner` trait implemented by both
  `SshClient` and `LocalClient`, refactor sub-managers to use `&mut dyn CommandRunner`.
- [x] **`InstallationConfig` hardcoded** — `ssh_install_command` always calls
  `InstallationConfig::for_len_serv_003()`, ignoring CLI args. Fix: accept `--config <file>` and load
  from YAML; fall back to auto-detect or interactive prompts.
- [x] **SSH auth agent-only** — `SshClient::connect` only tries `userauth_agent()` and immediately
  fails if no agent is running. Fix: add fallback to `~/.ssh/id_ed25519` / `id_rsa` key files.
- [x] **`preflight_checks` always uses `self.ssh`** even when `mode == Local`. This crashes local mode.
  Fixed as part of the CommandRunner trait refactor.
- [x] **`install` subcommand missing** — users expect `install` (local) and `install --remote <host>`.
  Currently only `local-install` and `ssh-install` exist with different UX. Add unified `install`
  subcommand.

## Features to Implement

- [x] **`install` subcommand** (unified): `ubuntu-autoinstall-agent install [--remote <host>]
  [--username user] [--config file]`. Without `--remote`, runs locally; with `--remote`, SSH-installs.
  Compatible with `curtin in-target -- ubuntu-autoinstall-agent install --config <path>`.
- [x] **Dracut support** — code currently always calls `update-initramfs` (initramfs-tools). The actual
  servers use dracut. Add `initramfs_type` field to `InstallationConfig` (dracut | initramfs-tools).
  When dracut: call `dracut --regenerate-all --force` instead, add `rd.neednet=1 ip=dhcp` to GRUB
  cmdline for Tang network unlock.
- [x] **Tang/Clevis enrollment** — add post-LUKS-format step to enroll clevis-tang with SSS:
  `clevis luks bind -d <device> sss '{"t":2,"pins":{"tang":[{"url":"http://172.16.2.45"},
  {"url":"http://172.16.2.46"},{"url":"http://172.16.2.47"}]}}'`. Install `clevis-tang clevis-luks
  clevis-dracut` in the target chroot.
- [x] **`deploy` subcommand (embedded binary)** — `ubuntu-autoinstall-agent deploy [--config <file>]`
  packs the binary with an embedded config payload (appended to the ELF). At runtime the binary detects
  the payload and uses it as config without external files. Optional AES-256 encryption of the payload
  keyed to a passphrase for secret hiding.
- [ ] **Config file schema** — add proper serde deserialization for the full `InstallationConfig` YAML
  including tang_servers, initramfs_type, ssh_keys, user accounts.
- [ ] **SSH key injection** — after debootstrap, inject the operator's `~/.ssh/authorized_keys` into
  `/mnt/targetos/root/.ssh/authorized_keys`.
- [ ] **`curtin in-target` compatibility** — when invoked as `curtin in-target -- ubuntu-autoinstall-agent
  install`, the binary is already inside the chroot; skip mount setup and debootstrap; only do
  post-install configuration (GRUB, LUKS crypttab, dracut, Tang).

## Known Issues / Tech Debt

- [ ] **`is_live_environment()` heuristic is weak** — checks `/run/live`, `/lib/live`, or `boot=live` in
  cmdline. On Ubuntu Server live ISO this is correct, but on iPXE-netbooted live environments it may
  miss. Consider also checking for `casper` in `/proc/cmdline` or presence of `ubuntu` in overlay mounts.
- [ ] **`detect_primary_disk` is fragile** — parses `lsblk` text output with simple string matching.
  Should use `lsblk --json` for reliable parsing.
- [ ] **`detect_network_config` always returns DHCP** — never actually reads network info; returns
  hardcoded DHCP. Needs actual parsing of `ip addr` / `ip route` output.
- [ ] **`setup_network_configuration` uses `networkd` renderer** — some servers may prefer `NetworkManager`.
  Make renderer configurable.
- [ ] **`hold_on_failure` keepalive calls `self.ssh.execute`** even in local mode — would fail locally.
  Fixed as part of CommandRunner trait refactor.
- [x] **`SshInstaller` dual-mode is awkward** — refactored to `runner: Box<dyn CommandExecutor>`;
  no more separate `ssh`/`local` fields or mode enum.
- [x] **No dracut `rd.neednet` in GRUB** — `configure_grub_in_chroot` now appends `rd.neednet=1 ip=dhcp`
  to `GRUB_CMDLINE_LINUX` when `initramfs_type == Dracut` and Tang servers are configured.
- [x] **Tang servers hardcoded** — moved to `InstallationConfig.tang_servers`; fully configurable
  per-machine via YAML; `for_len_serv_003()` sets all three Tang server URLs.
- [ ] **LUKS passphrase in process env** — `setup_installation_variables` exports `LUKS_KEY` as a
  shell env var, visible in `/proc/<pid>/environ`. Use a tempfile-based keyfile instead.
- [ ] **No test for local install flow** — all tests use `SshInstaller` with SSH. Add unit tests for
  local mode using `LocalClient`.
- [ ] **`PackageManager` installs `zsys`** — zsys is deprecated/removed in Ubuntu 24.04+. Remove it
  from package lists when release >= noble.
- [ ] **Build doesn't produce a static musl binary by default** — for curtin in-target use, the binary
  must run in a minimal chroot. Add `--target x86_64-unknown-linux-musl` build target and CI step.
- [x] **No CHANGELOG.md** — CHANGELOG.md created for this branch.

## Infrastructure Context

- Tang servers: 172.16.2.45, 172.16.2.46, 172.16.2.47 (SSS t=2 of 3)
- Servers: len-serv-001 (172.16.3.92), len-serv-002 (172.16.3.94), len-serv-003 (172.16.3.96)
- nginx cloud-init at 172.16.2.30
- initramfs: dracut (NOT initramfs-tools); rd.neednet=1 ip=dhcp for Tang network unlock
