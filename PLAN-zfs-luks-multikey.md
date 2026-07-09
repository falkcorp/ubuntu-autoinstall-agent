<!-- file: PLAN-zfs-luks-multikey.md -->
<!-- version: 1.0.0 -->
<!-- guid: 9f2a1c7e-3b4d-4e6a-8c1f-7d0e5a2b9c34 -->
<!-- last-edited: 2026-07-09 -->

# Autoinstall → LUKS FDE + ZFS (rpool/bpool) + 3-method unlock

> Supersedes the approach in the older `PLAN.md` (the "autoinstall renderer pivot"), which
> produced the LUKS+LVM+ext4 layout we are now replacing. That file is left intact for history.

## Goal

Make the autoinstall produce **LUKS2 full-disk encryption with a ZFS root** (`rpool`) and
boot pool (`bpool`) instead of the current LUKS+LVM+ext4, and enroll **multiple independent
LUKS2 unlock methods** so no method auto-decrypts a stolen, off-network machine:

1. **clevis Tang SSS** (t=2 of 3: 172.16.2.45/46/47) — headless auto-unlock on-LAN
2. **clevis TPM2 + PIN** — local auto-unlock bound to this machine, PIN required
3. **YubiKey FIDO2 + PIN** via `systemd-cryptenroll --fido2-with-client-pin` — manual/recovery

Reuse the already-proven ZFS+LUKS recipe in `src/network/ssh_installer/{disk_ops,zfs_ops,system_setup}.rs`.

### Scope additions (2026-07-09)
- **Target release pinned to Ubuntu 26.04 LTS (Resolute Raccoon)** everywhere — `debootstrap_release =
  "resolute"`, netboot ISO/casper = 26.04, no stray 25.x. Verify no 25.x paths leak into the seed.
- **unimatrixone (u1) must install with this same tool.** Its Supermicro X10DSC+ built-in RAID now
  presents a **single volume**, so u1 uses the **identical single-disk LUKS+ZFS layout** — only a new
  host config differs (hostname `unimatrixone`, IP 172.16.2.35, MAC ac:1f:6b:40:fc:e2, disk device
  likely `/dev/sda` for the RAID volume, power/console via **IPMI 172.16.3.150** not AMD DASH).
  Add `examples/configs/unimatrixone.yaml` (data-driven, preferred over another hardcoded `for_*()`).

### YubiKey topology (method 3, per host)
- **Primary:** a micro (YubiKey 5 Nano) that stays plugged into the machine. Theft is still
  blocked because FIDO2 unlock requires the **client PIN** (`--fido2-with-client-pin=yes`).
- **Secondaries (removable backups, shared across the fleet):** one kept locked up, one on the
  owner's keychain.
- Each host's LUKS header therefore enrolls **3 FIDO2 credentials** (primary micro + 2 backups).
  FIDO2 non-resident credentials are per-disk (the credential ID lives in each disk's LUKS2
  header), so one physical key enrolls on unlimited machines and never exhausts the key's slots.

---

## DECISION (2026-07-09): Option 2 — custom installer image, iPXE-triggered

**Chosen:** a purpose-built installer image (not subiquity). iPXE boots a custom
kernel/initrd/rootfs and passes kernel boot options (e.g. `uaa.config=<url>`) that
point the image at its per-host config; the image auto-starts the agent, which runs
the full ZFS-on-LUKS install, then powers off / reboots. No subiquity, no curtin —
a single install path.

Boot-time flow:
1. PXE → iPXE serves the custom installer kernel+initrd+rootfs with a cmdline that
   carries the per-host config URL.
2. A boot-time `uaa-autoinstall.service` in the image reads `uaa.config=` from
   `/proc/cmdline`, fetches the YAML, runs `uaa install --config`, reports status,
   then powers off (loop-safe) or reboots.
3. Next boot → local disk → first-boot TPM2 enrollment → done.

Open sub-decision — HOW to build the image (see AskUserQuestion): (a) overlay the
Ubuntu 26.04 live-server squashfs already extracted on the server with the static
agent + a systemd unit; (b) build a minimal image from scratch (mkosi/debootstrap)
carrying agent + debootstrap/zfs/cryptsetup/gdisk/clevis/tpm2 tools; (c) extend the
existing `src/image/builder` machinery + `create-image` command.

Superseded (kept for reference): the two ways below.


### Option A — Drive the proven imperative installer from the netboot live env  ⭐ recommended
`ssh_installer` (Path B) already does a full ZFS-on-LUKS install end-to-end (`sgdisk` →
`cryptsetup` → `zpool create rpool/bpool` → `debootstrap` → clevis/Tang → dracut → grub).
The autoinstall live environment runs `ubuntu-autoinstall-agent install --config <host>`
locally, then reboots.
- **Pros:** reuses the most-proven code; sidesteps curtin's weak ZFS-on-LUKS support;
  collapses the two divergent install paths (root cause of this whole incident) into one.
- **Cons:** bigger change to the netboot flow (live env fetches binary + per-host config, triggers it);
  subiquity becomes a thin shell.

### Option B — Full curtin `storage:` config inside the subiquity user-data
Replace `layout: name: lvm` with a hand-written curtin storage-v2 block (ESP + bpool partition
+ `dm_crypt` LUKS holding the rpool zpool).
- **Pros:** keeps the subiquity/curtin flow; smaller change to the netboot side.
- **Cons:** curtin ZFS-on-LUKS + separate bpool + clevis is fragile/underdocumented; high
  trial-and-error; likely still needs late-commands to finish ZFS/clevis anyway.

**Recommendation: Option A.** Steps below assume A. Say the word and I'll re-scope for B.

---

## Affected files (Option 2 — custom installer image)

- `installer-image/uaa-autoinstall.sh` — boot-time wrapper: parse `uaa.config=` from
  /proc/cmdline, fetch per-host YAML, run `uaa install --config`, report, poweroff (loop-safe). ✅ done
- `installer-image/uaa-autoinstall.service` — oneshot unit gated on
  `ConditionKernelCommandLine=uaa.autoinstall`; conflicts with subiquity. ✅ done
- `scripts/build-installer-image.sh` — overlay the 26.04 live-server squashfs with the
  static agent + service, mask stock installer autostart, re-squash. ✅ done (2 VERIFY-ON-VM markers)
- Per-host config: `examples/configs/<host>.yaml` served from
  `172.16.2.30/cloud-init/<host>.yaml` (the `uaa.config=` target). Add len-serv-00{1,2,3} + unimatrixone.
- Server iPXE per-MAC files: add `uaa.autoinstall uaa.config=<url>` + point at the overlaid squashfs.
  (Server-side; do during VM/first-host bring-up.)
- The old subiquity template (`len-serv.user-data.tmpl`) + host_spec/render/goldens become dead for
  installs (kept only if a subiquity fallback is still wanted) — verify.rs stays (post-install checks).

### Superseded plan (Option A internals — kept for reference)

### Rust — installer core
- `src/network/ssh_installer/system_setup.rs` — add `enroll_tpm2_pin_clevis` (clevis tpm2 w/ PIN)
  and `enroll_fido2_systemd` (`systemd-cryptenroll --fido2-device=auto --fido2-with-client-pin=yes`,
  loop to enroll primary + N backup keys). Ensure dracut pulls **both** `clevis`/`clevis-pin-*`
  **and** `sd-cryptsetup` modules. Keep existing `enroll_tang_clevis`.
- `src/network/ssh_installer/config.rs` — extend `InstallationConfig`: `enroll_tpm2: bool`,
  `tpm2_pcr_ids`, `enroll_fido2: bool`, `fido2_key_count: u8` (primary + backups). Update `for_len_serv_003()`.

### Rust — autoinstall path (make it invoke Option A)
- `src/autoinstall/templates/len-serv.user-data.tmpl` — replace `storage: layout: name: lvm`
  (lines 67-73) with a minimal live-env layout; change commands to fetch + run
  `ubuntu-autoinstall-agent install --config <host>` (or curtin-in-target hand-off, todo.md:40-42).
  Drop the LUKS-on-LVM `password:` assumption.
- `src/autoinstall/host_spec.rs` — add disk/encryption fields (disk device, tang servers/threshold,
  enroll flags, fido2 key count) so seeds are per-host, not hardcoded `/dev/nvme0n1`.
- `src/autoinstall/render.rs` — `.replace(...)` for new placeholders (guarded by `find_placeholder`).
- `src/autoinstall/verify.rs` — add ZFS checks (`zpool list rpool bpool`, `zfs list rpool/ROOT/...`,
  `/boot` on bpool, **no LVM**, clevis list shows sss+tpm2, a fido2 keyslot present). Fix LVM-baked
  fixture at verify.rs:434-435.

### Golden fixtures / tests
- `tests/fixtures/golden/len-serv-00{1,2,3}.user-data` — regenerate (`REGEN_GOLDEN=1`).

### Server scripts (172.16.2.30:/var/www/html/cloud-init/scripts/)
- `len-serv-00X-chroot-setup.sh` — add TPM2+PIN + FIDO2 enrollment; keep Tang; write
  `/etc/dracut.conf.d/` with `add_dracutmodules+=" clevis sd-cryptsetup "` + `rd.neednet`.
- New `register-fido2-luks.sh` — enroll primary + backup YubiKey FIDO2 creds into a host's LUKS
  header (distinct from the GPG-only `register-yubikey.sh`).

---

## Steps (ordered, each independently committable)

1. Installer core: TPM2+PIN enrollment (`system_setup.rs` + config) + command-builder unit test.
2. Installer core: FIDO2 enrollment loop (primary + backups) + config + unit test.
3. dracut integration: add both `clevis` and `sd-cryptsetup` modules; verify crypttab.
4. `verify.rs`: ZFS/pool/dataset + multi-method unlock checks; fix LVM fixture.
5. Autoinstall template + host_spec + render: switch Path A to Option A hand-off; parameterize.
6. Regenerate goldens; `cargo test` green.
7. Server scripts: chroot-setup updates + `register-fido2-luks.sh`.
8. VM validation (the gate).

## Test strategy

- **Unit:** `cargo test` — tpm2/fido2 command builders, render goldens, verify logic.
- **VM end-to-end (gate before ANY real host):** boot a throwaway QEMU VM **with a virtual TPM**
  (swtpm) through the same netboot/user-data path. Success criteria after install + reboot:
  - `lsblk`: p1 ESP, p2 RESET, p3 bpool, **p4 crypto_LUKS**, **no LVM**.
  - `zpool list` → `rpool` (on `/dev/mapper/luks`) + `bpool`; `findmnt /` = `rpool/ROOT/...`,
    `findmnt /boot` = `bpool/BOOT/...`.
  - `clevis luks list -d p4` → **sss(tang)** + **tpm2**; `cryptsetup luksDump p4` → a **fido2** slot.
  - Reboot on-LAN → unlocks headless (Tang). Tang blocked → TPM2 prompts PIN. Both blocked →
    YubiKey+PIN unlocks.
  - `update-grub` writes the **ZFS-native** `/boot/grub/grub.cfg` (no vfat bind-mount shadow —
    the exact bug that started this).
- Only when every VM criterion passes do we go near len-serv-003.

## Safe rollout for len-serv-003 (ONLY after VM validation)

1. Decommission node 8 (172.16.3.96) from CockroachDB — drain completes; **3 nodes remain (tight;
   watch flaky 002)**.
2. Clean up **ghost node 3** (`is_live=false`, no address): `cockroach node decommission 3`.
3. Wipe + netboot-reinstall 003 with the validated installer.
4. Rejoin 003; confirm `node status` shows it live + rebalancing.
5. Enroll primary micro + 2 backup YubiKeys into 003's LUKS via `register-fido2-luks.sh`.

## Rollback

- All work on branch `feat/autoinstall-zfs-luks-multikey` in a worktree; `main` untouched.
- Server scripts edited with timestamped `.bak-*` copies (existing convention) first.
- **len-serv-003 is not touched until VM validation passes** — no destructive dev step to undo.
  If a real-host reinstall fails, 003's data lives in CockroachDB replicas on the other 3 nodes
  (decommission is graceful) and the box can be re-netbooted.
