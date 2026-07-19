<!-- file: CHANGELOG.md -->
<!-- version: 1.3.0 -->
<!-- guid: chnglog1-0000-0000-0000-000000000001 -->
<!-- last-edited: 2026-07-10 -->

# Changelog

All notable changes to `ubuntu-autoinstall-agent` are documented here.

<!-- scriv-insert-here -->

<!-- scriv-end-here: releases below predate the changelog.d fragment system. -->

## [Unreleased] — constellation planning package (2026-07-10, docs only)

Planned (did NOT build) the rebuild into a Rust microservice constellation
(`uaa` + `uaa-control` + `uaa-web` + `uaa-pxe`, gRPC/tonic via protox, CRDB
registry, PKI agent enrollment, GitHub-OAuth RBAC, signed self-update): master
design spec (25 locked decisions, adversarial 3-lens judge panel), taskboard plan
with computed same-file collision matrix and 9 dependency waves, breakdown, roadmap,
and **42 weak-model-proof TASK briefs** across 10 workstreams (all passed cold-read
verification). Docs only — no code changes; the briefs are the execution interface.

## [Unreleased] — install-ops execution (2026-07-10)

All 20 briefs from the install-ops planning package merged across 6 dependency
waves (coordinator/worker orchestration). `cargo test --lib --offline` grew
237 → 311 passing, 0 failed at every merge. No hardware was touched — validation
was in-repo (cargo, `bash -n`, shellcheck) plus a Linux-only VM gate.

### Added

- **Suffix-aware `partition_path` helper** (`src/network/ssh_installer/partitions.rs`) —
  routes all 11 partition-path sites; appends `p` only when the device name ends in a
  digit, so QEMU/virtio `/dev/vda` and bare `/dev/sda` targets partition correctly (`7273286`).
- **`detect_primary_disk` via `lsblk --json`** (md/nvme/sd/vd, excludes loop/rom) (`d04567f`)
  and **`detect_network_config` via `ip -j addr`/`ip -j route`** instead of hardcoded
  eth0/dhcp (`44c0bca`).
- **Configurable netplan renderer** (networkd | NetworkManager) + `dhcp4: true` rendering
  when the address is `dhcp` (`519e721`).
- **`#[serde(deny_unknown_fields)]`** on `InstallationConfig` — typo'd YAML keys now fail
  loudly (`b9d710f`).
- **curtin in-target mode** — marker-file detection skips mount/debootstrap and runs
  Phase-5-only reconfiguration when already inside the target chroot (`6ffeae0`).
- **`--phases <spec>` / `--from-phase <n>` selective phase runs** with a compile-time
  `WipeAuthorization` token: a disk wipe is structurally impossible unless Phase 2 is
  selected; preflight refuses (does not wipe) on residual state in selective mode (`7d909e8`).
- **Non-destructive mount-existing-target prep** (assemble md → open LUKS → import
  rpool then bpool → mount `/` → `/boot` → ESP, the load-bearing order) for grub-only
  re-runs (`69263ed`).
- **efibootmgr BootOrder in chroot** after update-grub (network #1, ubuntu #2), non-fatal
  on legacy BIOS / missing efivars (`0cc3b3c`).
- **RESET partition (p2) staging** — recovery ISO + base tarball + a GRUB reset entry
  gated on the operator typing `nuke it`; the installer itself stays non-destructive (`3ef30b6`).
- **QEMU + swtpm VM validation harness** `scripts/vm-validate.sh` (virtio `/dev/vda` + TPM2),
  the gate that must pass on a Linux host before any hardware install (`e7a8eb7`).
- **LocalClient unit tests** exercising the `CommandExecutor` seam (`55ab93a`).
- **`uaa power <host> on|off|status`** subcommand — machine-class dispatch; the IPMI path
  runs `ipmitool` on the server (`ssh 172.16.2.30`), never locally; explicit off/on only,
  no reset/cycle (`f99dffa`).
- **Install-server endpoints (repo mirror, human-deployed):** webhook auto-flip on install
  `success` + tolerate missing iPXE file (`3c4b0c9`); `GET /api/health` with agent-binary
  presence + serving docs (`973b340`); `GET /api/uaa-configs` metadata inventory (`4ae949a`);
  `GET /dashboard` status page (`4900f93`); `deploy-usb-configs.sh --inject-from` place-time
  secret injection, refusing world-readable secrets and preserving the placeholder gate (`0e6d5a8`).
- **`docs/architecture-path-split.md`** documenting Path A (subiquity render) vs Path B
  (ssh_installer) with guardrails (`466e0b5`).

### Security

- **LUKS passphrase via a 0600 tempfile keyfile** (`--key-file`) — removed the
  `echo '<pass>' | cryptsetup …` command-line interpolation (visible in `ps`/history on the
  target) and the inert `LUKS_KEY` env export (`10fbb0f`).

### Known follow-up (not yet fixed)

- **IPMI password can leak under verbose logging.** `SshClient::execute_with_output`
  (`src/network/ssh.rs`) does `debug!("Executing command with output: {}", command)`, logging
  the full command string. `uaa power` passes `IPMI_PASSWORD='<pw>' ipmitool …` through the
  `CommandExecutor` seam, so running the binary with `-v`/debug would write the BMC password to
  local logs (the power module itself only ever logs a redacted twin). Fix pending: add a
  redaction seam (an optional "loggable command" override) to `SshClient`/`CommandExecutor`
  before `uaa power` is used with verbose logging. Tracked in `todo.md`.

## [Unreleased] — docs/plan-install-ops (2026-07-09)

### Added (planning only — no code changes)

- **install-ops planning package**: 6 design specs + 6 implementation plans
  (`docs/specs/<slug>-{design,plan}.md`), 20 weak-model-proof task briefs across 6
  workstreams (`docs/agent-tasks/`), coordinator protocol (`ORCHESTRATION.md`),
  computed same-file collision matrix + 6-wave schedule (`docs/agent-tasks/README.md`),
  and hardware-blocked deferrals (`DEFERRED.md`). Every brief passed a cold-read
  brief-verifier; the package passed a mechanical audit.
- **todo.md reconciliation**: idempotency (041982e), mdadm-initramfs (10599d8),
  is_live_environment/casper + config-driven local install (PR #27), SSH-key injection
  (already implemented), zsys (stale) closed with evidence; all remaining open items
  annotated with their task brief or deferral.

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
