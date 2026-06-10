# Tang Server Architecture

## Overview

Tang servers are the cryptographic backbone of the fleet's disk encryption system.
They implement the [Clevis/Tang](https://github.com/latchset/tang) protocol, which
allows servers to automatically unlock LUKS-encrypted disks at boot — without
storing keys on the servers themselves and without requiring human interaction —
as long as the tang servers are reachable on the network.

Tang servers run on Raspberry Pis (ARM64), making them low-power, always-on
key custodians with tamper-evident physical form factors.

---

## Security Model

### How Clevis + Tang Works

1. During initial setup, a server's LUKS volume is "bound" to tang via `clevis luks bind`
2. Clevis generates a random key, splits it using the tang server's public key, and stores
   the encrypted split in the LUKS LUKS keyslot metadata
3. At boot, the clevis initrd hook contacts the tang servers
4. Tang verifies the request cryptographically and responds with its half of the key
5. Clevis reconstructs the LUKS passphrase and unlocks the disk — automatically, no human needed

### Shamir's Secret Sharing (SSS) Threshold

Each disk is bound to **2-of-3 tang servers** using SSS. This means:

- Any single tang server going down does NOT prevent disk unlock
- An attacker who compromises one tang server cannot unlock any disks
- Two tang servers must cooperate (or be compromised) to unlock a disk
- This protects against a stolen/hacked tang server + off-site hardware theft

```
Client disk unlock requires:
  (tang-001 AND tang-002) OR
  (tang-001 AND tang-003) OR
  (tang-002 AND tang-003)
```

### Tang Servers' Own LUKS

The tang servers themselves are also LUKS-encrypted. Their disk unlock uses the
same SSS scheme, pointing at the OTHER tang servers. This means:

- A stolen RPI reveals nothing — LUKS is encrypted, keys live on the other RPIs
- Even the tang key material on disk is protected by LUKS

**Mutual dependency diagram:**

```
rpi-serv-001 LUKS ──unlocked by──> 2 of {rpi-serv-002, rpi-serv-003}
rpi-serv-002 LUKS ──unlocked by──> 2 of {rpi-serv-001, rpi-serv-003}
rpi-serv-003 LUKS ──unlocked by──> 2 of {rpi-serv-001, rpi-serv-002}

len-serv-* LUKS ──unlocked by──> 2 of {rpi-serv-001, rpi-serv-002, rpi-serv-003}
```

---

## Deployment Lifecycle

### Initial Deployment

1. Register each RPI with `register-rpi-tang.sh`
2. Approve each RPI in the autoinstall-agent
3. PXE boot each RPI → Ubuntu 26.04 ARM64 installs automatically
4. Tang service (`tangd.socket`) starts on first boot
5. Tang keys are generated automatically by tangd on first request

### Post-Install Binding (one-time)

After all tang servers are running and `curl http://<rpi>/adv` returns JSON:

```bash
bash tang-bind.sh
```

This SSHes into each tang server and runs `clevis luks bind` with the SSS policy.
The initial install passphrase remains as a fallback slot until explicitly removed.

### Ongoing Operation

- Tang servers run continuously as low-power RPIs
- No manual intervention required for client disk unlocks
- Backups run daily at 3 AM, encrypted to registered YubiKeys
- The autoinstall-agent tracks tang server status at `/api/tang/servers`

---

## Boot Sequence

### Normal Operation

```
1. Client server powers on
2. GRUB loads initrd (contains clevis hooks)
3. clevis-initrd contacts tang servers via HTTP:
   GET http://rpi-serv-001/adv   → tang public key advertisement
   GET http://rpi-serv-002/adv   → tang public key advertisement
4. With 2+ responses: clevis reconstructs LUKS passphrase via SSS
5. LUKS unlocks, kernel continues boot
6. Total unlock time: ~2-3 seconds after network available
```

### Tang Server Own Boot

```
1. RPI powers on
2. Grub loads initrd with clevis hooks
3. clevis contacts OTHER tang servers (not itself)
4. If 2+ peer tang servers are up → unlocks its own LUKS → boots
5. Tang service starts → available to serve other clients
```

### Cold Start (all tang servers down simultaneously)

See [Cold-Start Recovery](#cold-start-recovery) below.

---

## Cold-Start Recovery

If power is cut to ALL tang servers simultaneously (datacenter failure, planned
maintenance, disaster):

1. No server can auto-unlock (0 tang servers reachable)
2. Recovery requires the encrypted backup from Google Drive

**Recovery procedure:**

```bash
# On provisioning server or admin machine, with YubiKey plugged in:
bash tang-cold-start.sh
```

The script:
1. Downloads latest tang key backup from Google Drive
2. Decrypts it using your YubiKey (GPG touch required)
3. Starts a temporary tang instance on the admin machine (port 11697)
4. Guides you through console-unlocking the first 2 tang servers
5. Once 2 tang servers are up, the 3rd auto-unlocks via its clevis binding
6. Once all tang servers are up, client servers auto-unlock on next reboot

---

## Tang Key Backup System

Tang keys (`/var/db/tang/`) are the most critical data in the fleet.
Without them, LUKS-encrypted disks cannot be unlocked (except via manual passphrase).

### Backup Storage

| Layer | Location | Notes |
|---|---|---|
| Primary | Google Drive `gdrive:tang-backups/<hostname>/` | Encrypted GPG |
| Local fallback | `/var/backup/tang/` on each RPI | If rclone not configured |
| Retention | Last 30 backups per server | Older pruned automatically |

### Encryption

Backups are GPG-encrypted to **all approved YubiKey fingerprints** fetched from
the autoinstall-agent at backup time. This means:

- Only someone physically holding an approved YubiKey can decrypt the backup
- Adding a new YubiKey to the registry automatically includes it in future backups
- Revoking a YubiKey means future backups are not encrypted to it (old backups remain)

### Symmetric Fallback

If no YubiKeys are registered/approved, backups fall back to GPG symmetric
encryption using `/etc/tang/backup-passphrase`. This file must be manually
created and kept offline:

```bash
# Create a strong passphrase (keep this offline, e.g. on paper in a safe):
openssl rand -base64 32 > /etc/tang/backup-passphrase
chmod 600 /etc/tang/backup-passphrase
```

---

## Operational Commands

```bash
# Check tang server status
curl http://rpi-serv-001/adv | python3 -m json.tool

# View tang registry
curl http://provisioning-server:25000/api/tang/servers

# Manual backup
ssh jdfalk@rpi-serv-001 /usr/local/bin/tang-backup.sh

# Restore from Google Drive
bash tang-restore.sh rpi-serv-001

# Bind a newly installed tang server to the fleet
bash tang-bind.sh

# Emergency recovery
bash tang-cold-start.sh

# Check clevis bindings on a client server
ssh jdfalk@len-serv-001 sudo clevis luks list -d /dev/nvme0n1p3
```

---

## Files Reference

| File | Location | Purpose |
|---|---|---|
| `register-rpi-tang.sh` | `/var/www/html/cloud-init/scripts/` | Register new RPI tang server |
| `tang-bind.sh` | `/var/www/html/cloud-init/scripts/` | One-time LUKS+clevis SSS binding |
| `tang-backup.sh` | `/usr/local/bin/` (on each RPI) | Daily encrypted backup to Drive |
| `tang-restore.sh` | `/var/www/html/cloud-init/scripts/` | Restore from Drive backup |
| `tang-cold-start.sh` | `/var/www/html/cloud-init/scripts/` | Emergency cold-start recovery |
| `setup-gdrive.sh` | `/var/www/html/cloud-init/scripts/` | Configure rclone Google Drive |
| `/var/db/tang/` | On each RPI | Tang key material |
| `/var/log/cockroach-autoinstall/tang-registry.json` | Provisioning server | Tang server registry |

---

## Security Considerations

- **Network isolation**: Tang servers should only be reachable from the fleet's private network
- **TPM binding**: Tang servers also bind their checkin to TPM EK hash (via autoinstall-agent)
- **Physical security**: RPIs should be in a locked rack; someone with physical access + known passphrase could unlock LUKS directly
- **Key rotation**: Tang periodically rotates its keys (advertised keys change); old LUKS bindings continue to work as tang retains old keys
- **Revocation**: To force re-encryption of a disk, rebind with `clevis luks bind` after rotating tang keys
