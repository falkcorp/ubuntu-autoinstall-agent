<!-- file: docs/netboot-autodeploy.md -->
<!-- version: 1.0.0 -->
<!-- guid: 9a1b2c3d-4e5f-6071-8293-a4b5c6d7e8f9 -->
<!-- last-edited: 2026-06-20 -->

# Netboot Autodeploy — Source of Truth & Findings

This documents the **actual working deployment system** for the Lenovo/RPi server
fleet, and how it relates to this Rust tool (`ubuntu-autoinstall-agent`).

> **TL;DR:** The Rust tool was NOT used to deploy any current server. Every running
> machine was installed by the **native Ubuntu installer (subiquity) driven by a
> cloud-init autoinstall `user-data`** served over HTTP via iPXE netboot from
> **the server (172.16.2.30)**. The known-good config is **len-serv-003's `user-data`**.
> The strategic direction (see "Tool Direction" below) is to make the Rust tool
> *generate and drive that proven autoinstall flow* rather than reimplement an
> installer from scratch.

---

## Naming Convention (now saved globally in ~/.claude/CLAUDE.md)

- **"the server"** = `172.16.2.30` (hostname `unimatrixzero`). Always. No exceptions.
- Everything else is referred to by role name: `lenserv1`/`len-serv-001`, `lenserv2`,
  `lenserv3`, `rpiserv1`/`rpi-serv-001`, etc.
- **rpiserv\*** = arm64. **lenserv\*** = amd64.

---

## Fleet Facts (verified live this session, 2026-06-20)

All three Lenovo servers are running and were SSH-checked directly:

| Host        | IP (confirmed) | NIC        | MAC (dir key)  | OS            | initramfs |
|-------------|----------------|------------|----------------|---------------|-----------|
| len-serv-001| 172.16.3.92/23 | enp1s0f0   | 6c4b90bc39b3   | Ubuntu 26.04  | dracut    |
| len-serv-002| 172.16.3.94/23 | enp1s0f0   | 6c4b90bcf8a3   | Ubuntu 26.04  | dracut    |
| len-serv-003| 172.16.3.96/23 | enp1s0f0   | 6c4b90bcf7f4   | Ubuntu 26.04  | dracut    |

Key corrections to prior assumptions (the Rust `for_len_serv_003()` had these WRONG):
- NIC is **`enp1s0f0`**, NOT `eno1`.
- Release is **`resolute` (Ubuntu 26.04 LTS)**, NOT `plucky`/`oracular`.
  **Every 26.04 system uses dracut** — Ubuntu has moved off initramfs-tools.
- DNS search domain is **`jf.local`**.
- Nameservers: `172.16.2.1`, `1.1.1.1`, `8.8.8.8`.
- len-serv-003 live disk layout: GPT → p1 ESP(vfat) / p2 /boot(ext4) /
  p3 crypto_LUKS → LVM2 (`ubuntu-vg/ubuntu-lv`, ext4 root). i.e. **LUKS+LVM**,
  NOT the ZFS bpool/rpool design the Rust tool implements. crypttab:
  `dm_crypt-0 UUID=... none luks`. clevis packages installed: clevis, clevis-dracut,
  clevis-luks, clevis-systemd (v20-1ubuntu2). dracut 110-11.

> NOTE: The older `repos/scratch/server_setup/*.yaml` and `len-serv-003-setup.sh`
> describe a **ZFS bpool/rpool** design with `oracular`/`enp1s0f0`/`172.16.3.98`.
> That is an OBSOLETE approach. The current fleet is **LUKS+LVM on 26.04**, per the
> live machines and the server's current `user-data`. Trust the server, not scratch.

---

## The Deployment System (on the server, 172.16.2.30)

Web root `/var/www/html/`, served by nginx. Full reference in
`/var/www/html/cloud-init/README.md` (read it — it's good).

### Flow
```
Machine powers on
  └─ iPXE (TFTP from the server)
       └─ boot.ipxe → mac-<hexmac>.ipxe (per-machine: hostname + menu default)
            └─ menu.ipxe (arch-aware: amd64 | arm64)
                 └─ subiquity autoinstall, seed = http://172.16.2.30/cloud-init/<hexmac>/
                      └─ user-data installs Ubuntu unattended
                           └─ late-commands: curtin in-target chroot setup
                                └─ clevis/Tang bind, dracut rebuild, cockroach, omz
                                     └─ curl /api/flip/<hostname> → next boot = local disk
```

### Layout (key paths)
```
/var/www/html/
├── ipxe/boot/mac-<hexmac>.ipxe     # per-machine iPXE
├── ubuntu/                          # amd64 26.04 casper (kernel/initrd/squashfs)
├── ubuntu-arm64/                    # arm64 26.04 casper (EFI/boot/casper present)
├── isos/                            # ubuntu-26.04-live-server-{amd64,arm64}.iso
└── cloud-init/
    ├── README.md                    # authoritative system docs
    ├── reporting.sh                 # upload_logs / send_status_update / send_final_report
    ├── <hexmac>/                    # per-machine seed dir
    │   ├── user-data                # THE autoinstall config
    │   ├── meta-data                # instance-id + local-hostname
    │   └── network-config           # empty = DHCP
    ├── len-serv-00N -> <hexmac>     # symlink aliases
    └── scripts/
        ├── register-len-server.sh   # register new server (wraps register-gen.py)
        ├── register-gen.py          # generates user-data + chroot script
        ├── register-rpi-tang.sh     # RPi/Tang registration helper
        ├── setup_cockroachdb.sh     # arch-aware first-boot cockroach install
        ├── report-status.sh         # webhook reporter for installed system
        ├── ohmyzsh-install.sh       # omz unattended installer
        └── tang-*.sh                # tang backup/restore/cold-start/bind
```

### autoinstall-agent HTTP service (port 25000 on the server)
- `GET /api/registry` — all machines + status
- `GET /api/events` — last 50 webhook events
- `GET /api/approve/<mac>` — approve a pending machine
- `GET /api/flip/<hostname>[?target=custom-autoinstall]` — set next boot target
  (default flips to local disk; `target=custom-autoinstall` forces reinstall)
- `POST /api/checkin` — first-boot check-in (hostname/mac/ip/tpm_ek)
- Logs: `/var/log/cockroach-autoinstall/{events.jsonl,registry.json,files/}`
- CockroachDB CA: `/var/lib/cockroach-autoinstall/.cockroach-ca/{ca.crt,ca.key}`

### Force a reinstall of an existing host
```bash
curl "http://172.16.2.30:25000/api/flip/len-serv-001?target=custom-autoinstall"
# then reboot the machine
```

---

## The Known-Good Template: len-serv-003 user-data

Path on server: `/var/www/html/cloud-init/6c4b90bcf7f4/user-data` (9167 bytes,
last good edit 2026-06-20 12:37). **This is the reference. Do not "fix" it — it works.**

Structure (subiquity autoinstall v1):
- `identity` — hostname, user `jdfalk`, sha512 password hash
- `ssh` — install-server, allow-pw:false, 3 authorized ed25519 keys
- `packages` — incl. `clevis clevis-dracut clevis-luks zfs-dracut cryptsetup lvm2`
- `storage: layout: {name: lvm, match: {path: /dev/nvme0n1}, sizing-policy: all,
  password: "TANG_INITIAL_PASSPHRASE_REPLACE_WITH_CLEVIS"}`
  — i.e. subiquity creates LUKS+LVM with a *temporary* passphrase, later replaced by
  the clevis/Tang binding in late-commands.
- `early-commands` — start ssh, set installer password for debugging
- `error-commands` — pull reporting.sh, upload installer logs on failure
- `late-commands` — writes an **inline** `/target/tmp/chroot-setup.sh` heredoc, then
  runs it via `curtin in-target -- bash /tmp/chroot-setup.sh`. The chroot script:
  - sets up jdfalk user + SSH keys + sudo + zsh + oh-my-zsh
  - timezone, /etc/hosts fleet entries, rsyslog→172.16.2.30:2514 (relp)
  - writes `/root/variables.sh` (DISK, HOSTNAME, NET_ET_*, COCKROACH_*, TANG_URL*)
  - installs report-status.sh + setup_cockroachdb.sh, rc.local (cockroach + TPM checkin)
  - **clevis Tang bind**: finds crypto_LUKS dev via blkid, binds SSS
    `{"t":2,"pins":{"tang":[{.45},{.46},{.47}]}}` using the temp passphrase
  - **dracut network unlock**: `/etc/dracut.conf.d/clevis.conf` adds `network` module +
    `rd.neednet=1 ip=dhcp`; grub.d cfg adds same to `GRUB_CMDLINE_LINUX`; `update-grub`
    + `dracut --regenerate-all --force`
  - final report → `report-status.sh finished 100`
- tail: upload logs, `send_final_report`, `curl /api/flip/<host>`, `sleep 900`

Tang servers: `http://172.16.2.45`, `.46`, `.47` — SSS **t=2 of 3**.

---

## What Was Changed This Session (2026-06-20)

### 1. len-serv-001 & len-serv-002 user-data — regenerated from 003 ✅
Both were on the OLD broken approach: `storage: layout: {name: direct}` (**no
encryption at all**) + external downloaded chroot script with manual bind-mounts.
Replaced with copies of the 003 template, changing ONLY per-host fields. Verified via
`diff` — only these lines differ from 003:

| Field                | 001                          | 002                          | 003 (template)               |
|----------------------|------------------------------|------------------------------|------------------------------|
| hostname / messages  | len-serv-001                 | len-serv-002                 | len-serv-003                 |
| NET_ET_ADDRESS       | 172.16.3.92/23               | 172.16.3.94/23               | 172.16.3.96/23               |
| COCKROACH_ADVERTISE  | 172.16.3.92:36357            | 172.16.3.94:36357            | 172.16.3.96:36357            |
| COCKROACH_JOIN       | .30,.94,.96                  | .30,.92,.96                  | .30,.92,.94                  |
| api/flip path        | /api/flip/len-serv-001       | /api/flip/len-serv-002       | /api/flip/len-serv-003       |

Everything else (LUKS+LVM storage, clevis/Tang, dracut, packages, keys, password) is
byte-identical to 003. Originals backed up on the server as
`<hexmac>/user-data.bak-pre-003sync-20260620-211050`. **003 was not touched.**

Effect: if 001/002 are netboot-reinstalled now, they come up encrypted + Tang-bound
exactly like 003.

### 2. Naming convention saved globally ✅
Added an "Infrastructure Naming (MANDATORY)" block to `~/.claude/CLAUDE.md`.

### 3. Rust `for_len_serv_003()` corrected (in this repo, src/network/ssh_installer/config.rs) ✅
Changed `eno1`→`enp1s0f0`, `plucky`→`resolute`, added `1.1.1.1` nameserver, `jf.local`
search, and the 3 SSH keys. Test updated. **156 tests pass.** NOTE: this only fixes the
hardcoded fallback; see Tool Direction — the bigger issue is the tool's whole approach.

### 4. examples/configs/*.yaml — updated (NON-003 ones only)
basic-server / production-server / production-cluster / arm64-server were rewritten to
the current flat `InstallationConfig` schema (resolute + dracut). **`examples/configs/
len-serv-003.yaml` is OFF-LIMITS — the user manages it; do not edit it.** (It currently
holds an old nested schema and is NOT what deployed 003 — the real config is the
server's user-data, not this YAML. The YAML never went through the Rust tool.)

---

## Tool Direction (for next session — the actual goal)

The Rust tool "doesn't work / is shit" per the user — repeated attempts never complete a
full install (connection issues, partial installs, a thousand papercuts). Decision:

1. **Stop reimplementing an installer.** The tool currently does its own
   debootstrap + ZFS(bpool/rpool) + chroot pipeline. That diverges from the proven
   LUKS+LVM subiquity flow and is the source of the pain.
2. **Pivot to: generate + drive the proven autoinstall.** The tool should produce the
   same `user-data` that works (the 003 template, parameterized per host) and hand it to
   the native Ubuntu installer — locally or remotely. Keep the ability to run as a
   "do the last steps" / post-install step too (it can call into the installer for these
   systems rather than doing everything itself).
3. **Then shift focus from *making changes* to *validating* everything that's done** —
   verify each step (partitions, LUKS, clevis binding, dracut cmdline, network unlock)
   rather than fire-and-forget.

Concrete starting points when picking this up:
- The generator already exists on the server: `cloud-init/scripts/register-gen.py`
  (generates user-data + chroot script). Consider whether the Rust tool should wrap/
  replace it, or just emit a 003-shaped user-data directly.
- Reconcile the Rust `InstallationConfig` (ZFS-oriented, flat YAML) with the real
  autoinstall shape (subiquity v1, LUKS+LVM, inline chroot heredoc). The struct may
  need to model "render a user-data template" instead of "ZFS pool options".
- The remaining todo.md items (LUKS key in env, musl static binary, curtin in-target
  mode, weak detect_*) are mostly moot if the tool pivots to driving subiquity, EXCEPT
  the musl/in-target work, which becomes relevant if the tool runs inside the target.

---

## RPi / arm64 — NOT DONE (needs user input)

There are **no rpiserv netboot configs** on the server yet — only the 3 lenservs are
registered (by MAC). The arm64 casper tree exists (`/var/www/html/ubuntu-arm64/`) and the
documented way to create an arm64 host config is:
```bash
bash /var/www/html/cloud-init/scripts/register-len-server.sh <hostname> <mac> <ip> arm64
```
To bring rpiservs onto this system, we need their **MACs and target IPs**. The RPis are
currently the **Tang servers** (172.16.2.45/.46/.47) and are typically SD-card based, so
confirm intent before netboot-reinstalling them (reinstalling a Tang server breaks the
t=2-of-3 unlock for everything until it's re-set-up — see tang-cold-start.sh / tang
backup-restore scripts on the server).

---

## How to Verify / Useful Commands

```bash
# Live server facts
ssh jdfalk@172.16.3.96 "ip -br a; cat /etc/os-release; dracut --version; dpkg -l | grep clevis"
ssh jdfalk@172.16.3.96 "sudo cat /etc/netplan/00-installer-config.yaml; sudo cat /etc/crypttab"

# The known-good template + the deployed config it produced
ssh jdfalk@172.16.2.30 "cat /var/www/html/cloud-init/6c4b90bcf7f4/user-data"
ssh jdfalk@172.16.3.96 "sudo cat /var/log/installer/autoinstall-user-data"   # what actually ran

# Compare a host's served config to the 003 template
ssh jdfalk@172.16.2.30 "diff /var/www/html/cloud-init/6c4b90bcf7f4/user-data /var/www/html/cloud-init/6c4b90bc39b3/user-data"

# System docs + API
ssh jdfalk@172.16.2.30 "cat /var/www/html/cloud-init/README.md"
curl http://172.16.2.30:25000/api/registry

# NOTE: sudo on the SERVER (172.16.2.30) requires a password (not passwordless).
#       sudo on the lenservs (172.16.3.9x) IS passwordless for jdfalk.
#       The server's /var/www/html/cloud-init/<hexmac>/ files are group-writable via
#       ACL, so user-data can be rewritten WITHOUT sudo (don't chown/chmod them).
```
