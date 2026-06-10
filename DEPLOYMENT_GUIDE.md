<!-- file: DEPLOYMENT_GUIDE.md -->
<!-- version: 2.0.0 -->
<!-- guid: g8h9i0j1-k2l3-4567-8901-234567ghijkl -->

# Ubuntu AutoInstall Agent - Deployment Guide

Automated bare-metal Ubuntu 26.04 provisioning via iPXE netboot with cloud-init.  
Supports AMD64 and ARM64, single disk and RAID1, with a registry-based approval flow and TPM identity binding.

---

## Architecture Overview

```
Machine powers on
  └─ iPXE (TFTP from provisioning server)
       └─ boot.ipxe → dispatches by MAC address
            └─ mac-<hexmac>.ipxe  (per-machine: hostname + menu default)
                 └─ menu.ipxe     (3-item arch-aware menu)
                      └─ autoinstall (amd64 or arm64)
                           └─ cloud-init /cloud-init/<hexmac>/user-data
                                └─ installs Ubuntu 26.04
                                     └─ chroot setup (users, rsyslog, CockroachDB)
                                          └─ POST /api/flip/<hostname> → boot-local-disk
```

---

## Prerequisites

- Provisioning server running Ubuntu with:
  - `dnsmasq` (proxy DHCP + TFTP)
  - `nginx` (HTTP for ISO + cloud-init)
  - `python3` (autoinstall-agent)
  - `cockroach` v25.3.0 binary (for cert generation)
- Ubuntu 26.04 ISO(s) extracted to web root
- CockroachDB CA key available on provisioning server

---

## Initial Setup (One-Time)

### 1. Run the setup script
```bash
sudo bash /var/www/html/cloud-init/scripts/setup-autoinstall-agent.sh
```

This creates:
- `cockroach-autoinstall` system user
- `/var/lib/cockroach-autoinstall/` — agent + CA certs
- `/var/log/cockroach-autoinstall/` — events, registry, uploaded logs
- `/etc/systemd/system/autoinstall-agent.service` — runs on port 25000
- `/etc/sudoers.d/autoinstall-agent` — passwordless service management

### 2. Verify
```bash
systemctl status autoinstall-agent
curl http://localhost:25000/api/registry
```

---

## Registering a New Server

### Single-disk server (Lenovo, NUC, etc.)
```bash
bash /var/www/html/cloud-init/scripts/register-len-server.sh <hostname> <mac> <ip>
# Example:
bash /var/www/html/cloud-init/scripts/register-len-server.sh my-server-004 aa:bb:cc:dd:ee:ff 192.168.1.10
```

### Dual-disk server with RAID1 (Supermicro, etc.)
```bash
bash /var/www/html/cloud-init/scripts/register-len-server.sh <hostname> <mac> <ip> amd64 raid1
# Example:
bash /var/www/html/cloud-init/scripts/register-len-server.sh my-supermicro-001 aa:bb:cc:dd:ee:ff 192.168.1.11 amd64 raid1
```

### ARM64 server
```bash
bash /var/www/html/cloud-init/scripts/register-len-server.sh <hostname> <mac> <ip> arm64
```

The script generates:
- `/var/www/html/cloud-init/<hostname>/user-data` — autoinstall config
- `/var/www/html/cloud-init/<hostname>/meta-data`
- `/var/www/html/cloud-init/scripts/<hostname>-chroot-setup.sh` — post-install config
- `/var/www/html/ipxe/boot/mac-<hexmac>.ipxe` — per-machine iPXE file
- Registers MAC in agent registry as `pending`

### Approve the machine
```bash
curl http://provisioning-server:25000/api/approve/<mac>
```

### Power on and monitor
```bash
curl http://provisioning-server:25000/api/events
# Install takes ~5-10 min, then machine reboots into installed OS
# SSH in during the 15-min sleep window: ssh jdfalk@<ip>
```

---

## Storage Options

| Option | Description | Use When |
|--------|-------------|----------|
| `direct` (default) | Single disk, Ubuntu direct layout | Single NVMe/SSD |
| `raid1` | mdadm RAID1 across 2 largest disks, separate EFI/boot/root | Dual-disk servers |

The `raid1` layout:
- Finds the two largest disks automatically via `match: {size: largest}`
- EFI partition on disk0 (GRUB also installs to disk1 via late-commands)
- `/boot` and `/` each on separate RAID1 md devices
- `mdadm` package included automatically

---

## autoinstall-agent API

Running on port 25000 of the provisioning server.

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/api/registry` | GET | All registered machines + status |
| `/api/events` | GET | Last 50 webhook events |
| `/api/approve/<mac>` | GET | Approve a pending machine |
| `/api/flip/<hostname>?target=<boot>` | GET | Flip iPXE default (approved only for reinstall) |
| `/api/certs/<hostname>?ip=<ip>` | GET | Generate CockroachDB node cert (approved only) |
| `/api/register` | POST | Register new machine (called by register script) |
| `/api/checkin` | POST | Machine identity check — binds MAC + TPM EK hash |
| `/api/webhook` | POST | Install status updates + log file upload |
| `/api/finalreport` | POST | Final hardware report |

### Security model
- **Pre-registration required** — unknown MACs get 403
- **Manual approval** — new machines start as `pending`
- **TPM EK binding** — first boot sends TPM EK hash; future boots with mismatched TPM are rejected
- **Reinstall gated** — flipping to `custom-autoinstall` requires `approved` status
- **Success auto-flip** — install success triggers automatic flip to `boot-local-disk`

---

## iPXE Menu

Three items, auto-detects AMD64 vs ARM64:

| Key | Item | Notes |
|-----|------|-------|
| `i` | Install Ubuntu 26.04 | Routes to amd64 or arm64 path |
| `d` | Boot local disk | Default for installed servers |
| `l` | Live / diagnostics | Ubuntu live env |

Per-MAC files control `menu-default`:
- `boot-local-disk` — normal boot (set automatically on install success)  
- `custom-autoinstall` — trigger reinstall

> **Important:** Never edit the global `isset ${menu-default} ||` fallback in `menu.ipxe`.  
> Only edit per-machine MAC files in `ipxe/boot/`.

---

## What Gets Installed

Every server gets:
- Ubuntu 26.04 LTS
- `jdfalk` as UID 1000 (primary user, zsh, NOPASSWD sudo, SSH keys)
- rsyslog with RELP forwarding to provisioning server port 2514
- CockroachDB (arch-aware: amd64 or arm64 binary auto-selected)
- Timezone: America/New_York
- Standard package set: git, zsh, tmux, htop, jq, ethtool, prometheus-node-exporter, tpm2-tools, etc.
- TPM checkin on first boot (binds MAC + TPM EK to registry)

---

## Reinstalling a Server

```bash
# Flip iPXE to reinstall mode
curl "http://provisioning-server:25000/api/flip/<hostname>?target=custom-autoinstall"

# Reboot the server (SSH in and reboot, or physically)
ssh jdfalk@<ip> "sudo reboot"
```

---

## Troubleshooting

### Machine boots local disk instead of netbooting
BIOS boot order has local disk first. Press **F12** at POST for one-time network boot, or go into BIOS and move network above the local disk.

### "malformed autoinstall" error on screen
YAML syntax error in `user-data`. Validate:
```bash
python3 -c "import yaml; yaml.safe_load(open('user-data').read().replace('#cloud-config','',1))"
```
Common cause: unquoted `: ` (colon-space) inside a YAML scalar. Always use single-quoted `bash -c '...'` for commands containing colons.

### early-commands fail immediately
`chpasswd` uses `ubuntu-server:ubuntu` — `jdfalk` doesn't exist yet during early-commands (created by autoinstall identity section during install).

### CockroachDB won't join cluster
- Join addresses must use **RPC port** (e.g. 36357), not SQL port (36257)
- `--listen-addr` must match `--advertise-addr` port
- Wipe stale store data: `sudo rm -rf /var/lib/cockroach/data/* && sudo systemctl restart cockroach`
- Verify cert SANs include node IP: `openssl x509 -in /var/lib/cockroach/certs/node.crt -noout -text | grep -A2 "Subject Alt"`

### Logs from a failed install
```bash
ls /var/log/cockroach-autoinstall/files/   # uploaded by error-commands
curl http://provisioning-server:25000/api/events  # webhook events
```

---

## ARM64 Support

Ubuntu 26.04 arm64 ISO is pre-extracted at `/var/www/html/ubuntu-arm64/`.  
`setup_cockroachdb.sh` auto-detects `uname -m` and downloads the correct binary.  
Register with `arm64` as the 4th argument to `register-len-server.sh`.

