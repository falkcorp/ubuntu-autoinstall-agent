# YubiKey Management

## Overview

YubiKeys serve two roles in this fleet:

1. **Encryption keys** — Tang backup archives are GPG-encrypted to registered YubiKeys,
   ensuring only holders of approved physical keys can decrypt them
2. **Authentication** — SSH access via FIDO2/PIV-stored resident SSH keys (hardware-bound,
   non-exportable)

All YubiKey public keys are centrally managed via the `autoinstall-agent` registry,
with an approval workflow before any key gains access to encrypted data.

---

## Registration Workflow

```
YubiKey holder                    Admin
     │                               │
     │── export GPG + SSH pubkeys ──>│
     │── POST /api/yubikeys/register │
     │   status: "pending"           │
     │                               │── review key fingerprint + comment
     │                               │── GET /api/yubikeys/approve/<fp>
     │                               │   status: "approved"
     │                               │
     │<── automatically included ────│
         in next tang backup
```

Once approved:
- Tang backups are encrypted to the new key automatically
- SSH public key (if provided) is available via `/api/yubikeys/ssh-keys`

---

## Initial YubiKey Setup

### 1. Set Up GPG on YubiKey

Your YubiKey needs a GPG key loaded for encryption. This is a one-time process.

```bash
# Check if your YubiKey already has GPG keys:
gpg --card-status

# If not, generate keys (on the card for maximum security):
gpg --edit-card
# At gpg/card prompt:
admin
generate    # generates on-card keys: sign, encrypt, auth
# Choose key size (RSA 4096 or ED25519) and expiry
# Set a PIN (different from admin PIN)
quit

# Verify:
gpg --card-status | grep -E "Signature|Encryption|Authentication"
```

### 2. Set Up SSH Key on YubiKey (Optional but Recommended)

For hardware-bound SSH authentication:

```bash
# Option A: FIDO2 resident key (YubiKey 5 series, firmware 5.2.3+)
ssh-keygen -t ed25519-sk -O resident -O application=ssh:fleet-admin \
    -C "jdfalk@$(hostname)-yubikey" -f ~/.ssh/id_ed25519_yubikey
# The private key never leaves the YubiKey

# Option B: PIV slot (works with older YubiKeys)
ykman piv keys generate 9a /tmp/yk-pubkey.pem
ykman piv certificates generate 9a /tmp/yk-pubkey.pem \
    --subject "CN=jdfalk,OU=Fleet Admin"
ssh-keygen -i -m PKCS8 -f /tmp/yk-pubkey.pem > ~/.ssh/yk_piv.pub
```

---

## Registering a YubiKey

With your YubiKey plugged in and GPG card accessible:

```bash
# Run the registration script (auto-exports keys and submits to agent):
bash /var/www/html/cloud-init/scripts/register-yubikey.sh "My YubiKey 5 NFC"

# Outputs a fingerprint and approval command. Admin runs:
curl http://provisioning-server:25000/api/yubikeys/approve/<fingerprint>
```

### Manual Registration

If the script doesn't work in your environment:

```bash
# Get fingerprint:
FINGERPRINT=$(gpg --list-secret-keys --with-colons | grep '^fpr' | head -1 | cut -d: -f10)

# Export pubkey:
GPG_PUBKEY=$(gpg --armor --export "$FINGERPRINT")
SSH_PUBKEY=$(cat ~/.ssh/id_ed25519_yubikey.pub)

# Submit:
curl -X POST http://provisioning-server:25000/api/yubikeys/register \
    -H "Content-Type: application/json" \
    -d "{
        \"fingerprint\": \"$FINGERPRINT\",
        \"gpg_pubkey\": \"$(echo "$GPG_PUBKEY" | python3 -c "import sys,json; print(json.dumps(sys.stdin.read()))" | tr -d '"')\",
        \"ssh_pubkey\": \"$SSH_PUBKEY\",
        \"comment\": \"My YubiKey serial 12345678\",
        \"serial\": \"12345678\"
    }"
```

---

## API Reference

All endpoints on `http://provisioning-server:25000/`.

### List All Keys

```bash
curl http://provisioning-server:25000/api/yubikeys
```

Returns JSON object: fingerprint → `{status, comment, serial, registered_at, approved_at}`.

### Approve a Key

```bash
curl http://provisioning-server:25000/api/yubikeys/approve/<FINGERPRINT>
```

Fingerprint must be uppercase hex (e.g. `ABC123DEF456...`).

### Revoke a Key

```bash
curl http://provisioning-server:25000/api/yubikeys/revoke/<FINGERPRINT>
```

Revoked keys are excluded from future backups but existing backups remain encrypted
to them (old encrypted archives are not re-encrypted).

### Get SSH Keys

```bash
# Returns all approved SSH public keys — useful for authorized_keys updates:
curl http://provisioning-server:25000/api/yubikeys/ssh-keys
```

### Get GPG Public Key

```bash
# Returns armored GPG public key for a specific fingerprint:
curl http://provisioning-server:25000/api/yubikeys/<FINGERPRINT>/pubkey
```

---

## Propagating SSH Keys to Servers

To add a newly-approved YubiKey's SSH key to all fleet servers:

```bash
# Get approved SSH keys from agent:
SSH_KEYS=$(curl -sf http://provisioning-server:25000/api/yubikeys/ssh-keys | \
    python3 -c "import json,sys; print('\n'.join(json.load(sys.stdin)['keys']))")

# Update authorized_keys on a server:
ssh jdfalk@len-serv-001 "mkdir -p ~/.ssh && cat >> ~/.ssh/authorized_keys" <<< "$SSH_KEYS"
```

For fleet-wide propagation, add a cron job or ansible task that polls
`/api/yubikeys/ssh-keys` and updates `~/.ssh/authorized_keys`.

---

## Multiple YubiKeys

You can register multiple YubiKeys (personal laptop key, personal desktop key,
backup key stored in a safe, colleague's emergency key). All approved keys are
used as GPG recipients in every tang backup.

Recommended setup:
- **Primary YubiKey** — your everyday key (on keychain)
- **Backup YubiKey** — stored in a physically secure location (safe, safety deposit box)
- **Emergency key** — colleague or trusted party who can assist in disaster recovery

---

## Decrypting a Backup

With your YubiKey plugged in:

```bash
# The gpg-agent will request your YubiKey PIN and touch:
gpg --decrypt tang-keys-rpi-serv-001-20260609-030000.tar.gz.gpg | tar xzv

# Or use tang-restore.sh for automated restore:
bash tang-restore.sh rpi-serv-001
```

When you touch the YubiKey gold contact, the decryption proceeds. No password is
stored on disk — possession of the physical key is required.

---

## YubiKey PIN Management

| PIN | Default | Purpose |
|---|---|---|
| User PIN | 123456 | Required for signing/decryption (limited attempts) |
| Admin PIN | 12345678 | Required for key management |

**Change the default PINs immediately:**

```bash
gpg --change-pin    # changes user PIN
# At menu, choose: 3 - change Admin PIN
```

Recommended: 6-8 digit numeric or 8+ character alphanumeric PINs.
PIN lockouts: 3 wrong user PINs locks the card (admin PIN can reset).
8 wrong admin PINs permanently bricks the GPG functionality.

---

## Troubleshooting

### YubiKey Not Detected

```bash
# Check USB detection:
lsusb | grep Yubico
gpg --card-status

# Restart pcscd (smartcard daemon):
sudo systemctl restart pcscd
```

### GPG Decryption Fails

```bash
# Check key is in keyring (public key must be imported):
gpg --list-keys | grep <fingerprint>

# Import from agent if missing:
curl http://provisioning-server:25000/api/yubikeys/<FINGERPRINT>/pubkey | gpg --import
```

### Registration Script Fails

```bash
# Check if ssh-agent is running and has YubiKey key loaded:
ssh-add -L

# For FIDO2 resident keys, load them first:
ssh-keygen -K    # exports resident keys to ~/.ssh/
```
