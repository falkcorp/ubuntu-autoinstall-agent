# todo.md — ubuntu-autoinstall-agent
# version: 1.8.0
# guid: todo0001-0000-0000-0000-000000000001
# last-edited: 2026-07-09

## Critical Bugs (blocking correct operation)

- [ ] **Autoinstall produces a broken `/boot` layout (ext4 instead of ZFS/bpool).** The autoinstall must
  create `/boot` as a ZFS dataset that is part of the `bpool` zpool, NOT as a standalone ext4 (or the
  vfat-shadow hack seen on len-serv-002). Concrete failure diagnosed 2026-07-09 on len-serv-002
  (172.16.3.94): the install left `/boot/grub` **bind-mounted from a vfat copy** (`/boot/efi/grub` →
  `/boot/grub`, via `/etc/fstab`) that *shadowed* the real ZFS-resident `grub.cfg` inside
  `bpool/BOOT/ubuntu_3pvepx@/grub`. GRUB's EFI stub (`/boot/efi/EFI/ubuntu/grub.cfg`) does
  `configfile ($root)/BOOT/ubuntu_3pvepx@/grub/grub.cfg` — i.e. it reads the **ZFS** file — but
  `update-grub` wrote the **vfat** shadow copy, so kernel upgrades never reached the real boot config.
  Result: the box booted a frozen install-time entry (kernel 6.11.0-19 + `ds=nocloud;s=http://172.16.2.30/...`)
  no matter how many times the kernel was upgraded or the on-disk (shadow) grub.cfg was hand-edited.
  Fixed by hand on len-serv-002 (removed bind mount, deleted `/boot/efi/grub`, re-ran `update-grub`
  against the real ZFS file). Compare working reference: len-serv-003 (direct 26.04 install) has a clean
  layout with no bind mount. Fix the installer so every host gets the correct bpool `/boot` layout and
  `update-grub` targets the file GRUB actually reads.

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

## Phase-selective re-run (designed 2026-07-09)

- [ ] **Run only specific install phases (idempotent partial re-run).** Install is 7 phases
  (0 vars, 1 pkgs, 2 disk-prep/WIPE, 3 zfs, 4 base, 5 sys-config incl. grub, 6 final).
  Add `--phases <spec>` / `--from-phase <n>` so e.g. a failed grub can be redone with
  `--phases 5` WITHOUT re-wiping (Phases 2-3). Requires a non-destructive "mount existing
  target" prep (assemble md, open LUKS, import rpool/bpool, mount in correct order: / then
  /boot then ESP, chroot binds) that runs when disk phases are skipped but later phases need
  a mounted target. Guard: skipping Phase 2/3 must never wipe. Config could also carry a
  default phase set. Motivation: we re-ran the whole ~7min install repeatedly just to retry
  grub.

## Post-install / boot productionization (designed 2026-07-09, install now 7/7)

- [ ] **Preflight: SUM read-only check of SATA/RAID controller OpROM mode (md targets).**
  IMSM arrays only boot in the OpROM mode they were created under. Before installing to an
  md/IMSM target, run SUM `GetCurrentBiosCfg` (READ ONLY — we can run SUM in-band from the
  live disk) and check the storage OpROM/controller mode token; if it would make the array
  unbootable (e.g. UEFI mode for a legacy-created array), ABORT the install and warn the
  user to fix the BIOS. Do NOT auto-change BIOS via SUM (keep firmware writes manual). This
  prevents completing a 7/7 install that then can't boot (exactly what happened on U1 —
  fixed by manually setting the controller to legacy). Need to identify the exact BIOS token.
- [ ] **efibootmgr boot order in chroot (post-grub).** Set UEFI BootOrder so
  **network #1, ubuntu #2** — firmware tries PXE first, falls through to the installed
  disk. grub-install currently makes `ubuntu` #1. Also flip the server's PXE target for
  the MAC to "boot local/proceed" so netboot doesn't reinstall. Prefer efibootmgr in the
  chroot over ipmitool-from-server for the EFI order.
- [x] **USB auto-bootstrap like netboot** (code shipped; deploy checklist below). USB live env,
  on boot with the `uaa.autoinstall` cmdline token, fetches the static agent + its config
  BY MAC from the web util (172.16.2.30 autoinstall-agent.py, same as netboot) and
  auto-runs `uaa install`, reports back, best-effort efibootmgr (network #1, ubuntu #2),
  then powers off (loop-safe). Shipped: `/autoinstall/uaa-config` endpoint (repo mirror of
  autoinstall-agent.py), `scripts/deploy-usb-configs.sh` (refuses REPLACE_AT_PLACE_TIME),
  `installer-image/nocloud/uaa-usb-bootstrap.sh` + user-data runcmd gate,
  `make-ssh-ready-iso.sh --autoinstall`. Without the token the same USB stays
  SSH-ready-only. SCOPE: OS install only — NO cockroach post-install/join.

  **Deploy checklist for the human (USB auto-bootstrap):**
  1. Deploy the endpoint: `scp scripts/autoinstall-agent.py 172.16.2.30:/var/www/html/cloud-init/scripts/`
     then `ssh 172.16.2.30 'sudo systemctl restart autoinstall-agent'`.
  2. Inject real secrets into staging copies of `examples/configs/install/<host>.yaml`
     (replace every REPLACE_AT_PLACE_TIME), then on the server run
     `scripts/deploy-usb-configs.sh --src <staging-dir>` (places `<hexmac>/uaa.yaml`;
     refuses any file still holding the placeholder).
  3. Build + copy the static agent: `scripts/build-musl.sh` on a Linux box (or grab the
     `uaa-amd64` CI artifact), then
     `sudo install -D -m 0755 <bin> /var/www/html/uaa/uaa-amd64` on the server.
  4. Rebuild the USB: `scripts/make-ssh-ready-iso.sh --autoinstall <stock.iso>`, then
     `sudo dd if=<out.iso> of=/dev/sdX bs=4M status=progress conv=fsync`.
  5. Test the full flow on one host: boot USB → agent+config fetched by MAC →
     `uaa install` 7/7 → webhook report → poweroff; verify BootOrder + next boot from disk.
- [ ] **Populate the RESET partition (p2, 4GiB ext4 — subiquity reset/recovery partition).**
  Currently created + formatted but empty. Idea (user 2026-07-09): put a copy of the
  bootable USB + the debootstrap tarball on it, and a GRUB "reset/recover" entry. The reset
  flow must GATE on explicit confirmation — load the recovery env, prompt that it will
  DELETE EVERYTHING, require typing `nuke it` (or similar) before wiping. Ref:
  subiquity autoinstall reset-partition.
- [ ] **Installed OS didn't boot on first try (2026-07-09):** with `ubuntu` already #1 in
  BootOrder + BootNext=0000, U1 still booted the USB live env. So either the installed
  loader (shim/grub on IMSM md126p1 ESP) failed and fell through to the USB, or the BIOS
  prefers removable/USB. Isolate by pulling the USB + reboot; if it still fails, watch IPMI
  SOL (from server) for where it dies (shim/grub vs initramfs md-assembly vs Tang unlock).

## Known Issues / Tech Debt

- [ ] **Installer not idempotent over a prior install (disk busy on re-run).** Found on
  unimatrixone 2026-07-09: a first clean install ran 5/6 phases; re-running the installer over
  the resulting disk failed at Phase 2 `wipefs -a /dev/md126` with "Device or resource busy"
  because a pre-existing **rpool was still imported** (and its LUKS mapper open) holding the md
  device. `destroy_existing_zfs_pools` only handles *imported* pools via `zpool list`, and does
  not force-export/`zpool labelclear`/close-luks/kill-holders before wiping. Fix: before wipe,
  `zpool export -f` any pool on the target disk (or `fuser -mk` the target mount, `cryptsetup
  close`, `zpool labelclear -f` each partition). Until fixed, re-runs need a manual clean or a
  reboot of the live env. First (clean-disk) run wiped fine — this only bites re-installs.
- [x] **bpool not GRUB-readable (fixed 297a49e).** `build_bpool_create_command` mixed
  `compatibility=grub2` with explicit `feature@livelist/zpool_checkpoint=enabled`, enabling
  modern features (block_cloning, log_spacemap, …) GRUB can't read → grub-install "unknown
  filesystem". Now uses compatibility=grub2 alone. (Validated only in unit tests — the U1
  re-run to confirm end-to-end was blocked by the idempotency bug above + lab network loss.)

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
- [x] **Static musl binary** (shipped) — `scripts/build-musl.sh` + CI
  `.github/workflows/musl-build.yml` build `x86_64-unknown-linux-musl` release and verify it
  is static (artifact `uaa-amd64`). Human deploys it to the server at
  `/var/www/html/uaa/uaa-amd64` (the `UAA_AGENT_URL` default the USB bootstrap curls).
- [ ] **unimatrixone needs mdadm assembly in the target initramfs** — u1's disk is Intel IMSM/BIOS
  fake-RAID assembled by mdadm as `/dev/md126` (single ~885 GiB volume; NOT `/dev/sda`, which is a
  RAID *member*). The installer neither adds `mdadm` to the target package set (`packages.rs` only
  installs into the live env) nor configures a dracut `mdraid` module, so `/dev/md126` would not
  re-assemble in the installed initramfs — it must assemble *before* LUKS/ZFS unlock. Add `mdadm` to
  the target packages + dracut `mdraid` module, gated on the target disk being an md device. Validate
  on the QEMU/mdadm path before any u1 hardware attempt. (The `{}p1` suffix scheme is already correct
  for md126.)
- [ ] **Partition-name suffix is hardcoded `{}p1..p4`** — correct for every current target (NVMe
  `nvme0n1`, md `md126` — both end in a digit → take `p`) but wrong for bare `/dev/sda` / `/dev/vda`
  (SATA/virtio → `sda1`, no `p`). Route the ~9 call sites (disk_ops, zfs_ops, system_setup, installer)
  through one helper that appends `p<N>` only when the device name ends in a digit. Needed before the
  QEMU gate if that VM uses a virtio `/dev/vda` disk. NOTE: `zfs_ops.rs` test asserts `/dev/sdap3` —
  that bakes in the bug and must be corrected with the fix.
- [x] **reqwest `Cargo.toml` bound was `^0.13` but lock/intent is `0.12.28`** — dependabot commit
  5f48844 (2026-06-23) set `version = "0.13"` while its own message + `Cargo.lock` say 0.12.28;
  `^0.13` can't match 0.12.28 and `reqwest 0.13.x` dropped the `rustls-tls` feature, so the crate did
  not build. Reverted the bound to `"0.12"` to match the lock; `cargo test --lib` green again.
- [x] **No CHANGELOG.md** — CHANGELOG.md created for this branch.

## New Machines / Pending Registration

- [ ] **unimatrixone** — new server (hardware TBD, may be different class than lenservs).
  Suspected two drives — unknown if hardware RAID, mdadm, or two independents. Must be
  booted and SSH'd into to determine disk layout before generating user-data. Not yet
  registered in the netboot tree (`/var/www/html/cloud-init/` on 172.16.2.30). Steps:
  1. Get it powered on (IPMI or physical).
  2. SSH in and run `lsblk -o NAME,SIZE,TYPE,FSTYPE,MOUNTPOINT` + `cat /proc/mdstat` +
     `lspci | grep -i raid` to determine disk topology.
  3. Decide storage layout (LUKS+LVM on one disk, or RAID1+LUKS, etc.).
  4. Register via `register-len-server.sh <hostname> <mac> <ip> [arch]` on the server.
  5. Generate user-data (possibly a new template variant if disk layout differs from lenserv).

## Remote Power Control (IPMI / AMD DASH / Intel ME)

- [ ] **Lenovo M715q (len-serv-001/002/003) — AMD DASH via Realtek**.
  The M715q uses AMD DASH (NOT Intel AMT — AMD Ryzen Pro, no MEBx). Remote power via
  `wsman` tool calling CIM_PowerManagementService on port 16992.
  Status: BIOS DASH enabled, RTL8111EPP NIC enabled, but Realtek DASH driver + DASHConfigRT
  credentials NOT yet installed on any lenserv. Driver from:
  pcsupport.lenovo.com → M715q → Networking: LAN.
  Steps per host:
  1. Install Realtek DASH driver (`DashDriver/autorun.sh`) + reboot.
  2. Configure credentials with `DASHConfigRT -xf:config1.xml`.
  3. Start `clienttool <nic>` as a systemd unit.
  4. Test: `wsman invoke -h <ip> -P 16992 -u Administrator -p <pass> -a RequestPowerStateChange ... -k PowerState=2`
  DASH PowerState values: 2=on, 6=graceful off, 8=hard off, 10=hard reset.

- [ ] **unimatrixone — IPMI (if it has a BMC) or Intel AMT (if Intel CPU)**.
  Machine class unknown as of 2026-06-30. Once booted:
  - Check for BMC: `ipmitool bmc info` or look for IPMI port in BIOS.
  - Check CPU vendor: `lscpu | grep Vendor` — if Intel, check MEBx (Ctrl+P at boot) for AMT.
  - If IPMI: `ipmitool -I lanplus -H <bmc-ip> -U admin -P <pass> chassis power on/off/reset`.
  - If Intel AMT: use `wsmancli` or `amtterm` targeting port 16992.
  - If neither: fall back to Wake-on-LAN (`wol <mac>`) for power-on (not power-off).

- [ ] **Wire remote power into the tool** — once credentials are known, add a
  `ubuntu-autoinstall-agent power <hostname> on|off|reset` subcommand that dispatches
  to the right mechanism (DASH/AMT/IPMI/WoL) based on machine class. This enables fully
  automated: place → flip → power-cycle → wait-for-ssh → verify.

## Infrastructure Context

- Tang servers: 172.16.2.45, 172.16.2.46, 172.16.2.47 (SSS t=2 of 3)
- Servers: len-serv-001 (172.16.3.92), len-serv-002 (172.16.3.94), len-serv-003 (172.16.3.96)
- unimatrixzero (the server): 172.16.2.30 — nginx + autoinstall-agent HTTP (port 25000)
- unimatrixone: IP/MAC TBD — not yet in netboot registry
- nginx cloud-init at 172.16.2.30
- initramfs: dracut (NOT initramfs-tools); rd.neednet=1 ip=dhcp for Tang network unlock
- M715q = AMD Ryzen Pro → AMD DASH (Realtek), NOT Intel AMT
