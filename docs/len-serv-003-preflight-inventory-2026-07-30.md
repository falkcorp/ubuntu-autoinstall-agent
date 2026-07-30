<!-- file: docs/len-serv-003-preflight-inventory-2026-07-30.md -->
<!-- version: 1.0.0 -->
<!-- guid: 7d3f9b21-6c48-4e05-a19f-2b8c5d40e6a7 -->
<!-- last-edited: 2026-07-30 -->

# len-serv-003 pre-wipe inventory and drift audit

Captured 2026-07-30 while len-serv-003 was still running its old install, so the
automated redeploy can be checked against what the machine actually had.

## ⛔ Do not netboot it with the server's current seed

The live PXE path on the server (172.16.2.30) still points at the **old
PlainLuks/LVM** autoinstall:

| path | state |
|---|---|
| `/var/www/html/ipxe/boot/mac-6c4b90bcf7f4.ipxe` | `set menu-default boot-local-disk`, writable by `jdfalk` (no sudo) |
| `/var/www/html/cloud-init/6c4b90bcf7f4/user-data` | 9167 bytes, 2026-06-20, `storage: layout: {name: lvm ...}` |

Flipping that default to `autoinstall` and rebooting **reinstalls the exact
non-standard layout this rebuild exists to remove.** The intended config is
[`examples/configs/install/len-serv-003-native-keystore.yaml`](../examples/configs/install/len-serv-003-native-keystore.yaml).

`docs/netboot-autodeploy.md` is stale on this point: it calls len-serv-003's
`user-data` "the known-good template — do not fix it, it works" and states that
001 and 002 were regenerated *from* it. The live machines show the reverse.

## Drift: len-serv-003 is the outlier

| | len-serv-001 | len-serv-002 | **len-serv-003** |
|---|---|---|---|
| root fs | `rpool/ROOT/ubuntu_s2otct` (zfs) | `rpool/ROOT/ubuntu_3pvepx` (zfs) | `/dev/mapper/ubuntu--vg-ubuntu--lv` (ext4) |
| `--listen-addr` | `:36357` | `:36357` | `172.16.3.96:36357` |
| `--sql-addr` | `:36257` | `:36257` | `172.16.3.96:36257` |
| `--store` | `path=/var/lib/cockroach/cockroach-data,attrs=ssd,size=.5` | same | `/var/lib/cockroach/data` |
| `--http-addr` | `:38080` | `:38080` | `:38080` |
| `--locality` | `region=us,cluster-unit=lenovo` | same | same |

Two consequences worth naming:

- The IP-bound `--sql-addr` is why `cockroach sql --host=127.0.0.1:36257` is
  refused on 003 but works on 001/002. The port *numbers* are consistent
  fleet-wide; the **bind form** is the drift.
- 003 has `zfs-dracut` installed and the zfs-import/mount/share/zed units enabled
  but **no zpool** — the standard profile's ZFS tooling sitting on an LVM install.

**Standardize the redeploy on the 001/002 flag form**, not 003's.

## Inventory

Ubuntu 26.04 LTS, kernel 7.0.0-27-generic, hostname `len-serv-003`.

**Disk** — single 238.5G `nvme0n1`; p1 1G vfat `/boot/efi`, p2 2G ext4 `/boot`,
p3 235.4G crypto_LUKS → LVM2 `ubuntu-vg/ubuntu-lv` ext4 `/`.
by-id `nvme-WDC_PC_SN730_SDBQNTY-256G-1001_193547802479`.

**crypttab** — `dm_crypt-0 UUID=210735c1-4b9d-45ff-a954-8d3648e17e1a none luks`

**clevis (slot 1)** — `sss t=2` over Tang
`http://172.16.2.45`, `.46`, `.47`. All three answered `/adv` 200 on 2026-07-30.

**apt manual** — bash, clevis, clevis-dracut, clevis-luks, cryptsetup, dash,
debootstrap, diffutils, dosfstools, efibootmgr, ethtool, findutils, gdisk, git,
grep, grub-efi-amd64, grub-efi-amd64-signed, gzip, hostname, htop, init,
landscape-client, linux-generic, lshw, lvm2, ncurses-base, ncurses-bin,
openssh-server, parted, prometheus-node-exporter, rsyslog, rsyslog-relp, screen,
shim-signed, tpm2-tools, tree, ubuntu-minimal, ubuntu-server,
ubuntu-server-minimal, ubuntu-standard, unzip, util-linux, zfs-dracut, zip, zsh

**snaps** — canonical-livepatch v10.16.2, core22, snapd

**enabled units beyond stock** — `cockroach.service`,
`cockroach-rollout-agent.service` (installed 2026-07-28, disabled),
`prometheus-node-exporter.service` + 5 collector timers, `landscape-client`,
`chrony`, `clevis-luks-askpass.path`, `tpm-udev.path`, `smartmontools`,
`sysstat`, `kdump-tools`, `open-vm-tools`/`vgauth`, `ufw` (inactive),
`unattended-upgrades`, `snap.canonical-livepatch.canonical-livepatchd`

**users (uid ≥ 1000)** — only `jdfalk` (1000), shell `/usr/bin/zsh`

**netplan** — `enp1s0f0`, static `172.16.3.96/23`, gw `172.16.2.1`, nameservers
`172.16.2.1` and `1.1.1.1`. No `jf.local` search domain is set on the box,
despite the netboot doc listing one.

**`/usr/local/bin`** — `cockroach`, `cockroach-rollout-agent`, `report-status.sh`
**`/opt`** — empty
**`/etc/cron.d`** — `e2scrub_all`, `zfsutils-linux` (both package-supplied)

## Applications the redeploy must reproduce

Candidates for `ApplicationSpec` variants. `Cockroach` already exists;
`TangServer` was added by PS-APP-09. The rest are unmodelled today.

| # | application | what it needs |
|---|---|---|
| 1 | **cockroach** | binary, `cockroach.service`, `cockroach` user/group, `/var/lib/cockroach/certs`, store path, and the listen/sql/http/advertise/locality/cache flags — in the **001/002 form** |
| 2 | **cockroach-rollout-agent** | binary, `/etc/cockroach-rollout-agent.env`, certs dir, sudoers fragment, systemd unit |
| 3 | **prometheus-node-exporter** | package, service, collector timers |
| 4 | **landscape-client** | package + service, enrolment |
| 5 | **canonical-livepatch** | snap + service |
| 6 | **report-status.sh** | `/usr/local/bin/report-status.sh` webhook reporter |
| 7 | **zsh + oh-my-zsh** | login shell for `jdfalk`; omz is a documented late-command |
| 8 | **DASH provisioning** | so a rebuilt host returns with working out-of-band power (see below) |

## Cluster safety — the box is free to wipe

CockroachDB **node 8 is `decommissioned`**, which is terminal: it can never
rejoin under that ID. That is the real reason its service runs while logging
"unable to contact the other nodes" and SQL returns SQLSTATE 57P01. It is not a
network fault — TCP/36357 is open both directions, no firewall, the node cert is
valid against the right CA, and clock skew is 0.

Cluster is 4 active (U0 n4, len-serv-002 n5, len-serv-001 n6, U1 n9),
**0 under-replicated, 0 unavailable, 290 ranges** at `num_replicas = 3`.
len-serv-003 holds no cluster data.

After reinstall it rejoins as a **new node ID**; peers join by address, so no
`--join` list anywhere needs editing.

## Remaining blocker: no out-of-band power on .96

These are Lenovo ThinkCentre M715q (MT 10VH) with **AMD DASH**, not IPMI — there
is no `/dev/ipmi*` and probing 16992/16993 is Intel AMT and always misleads.
DASH listens on **623/664**, is true out-of-band, and is already live on
len-serv-001/002. On len-serv-003 those ports are **closed** — the firmware
listener was never provisioned. BIOS `DASH Support` already reads `Enabled` via
ThinkLMI with no BIOS admin password, so this is provisioning, not a BIOS toggle.

Until that is fixed, a remote wipe that misses PXE means a physical trip.
