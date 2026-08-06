<!-- file: todo.d/2026-08-06-rpi-tang-selfunlock-and-touch-sudo.md -->
<!-- version: 1.0.0 -->
<!-- guid: 7e41b9d2-8c05-4f36-a1e7-3b962d05c8f4 -->
<!-- last-edited: 2026-08-06 -->

## Harden the RPi Tang servers: self-unlock policy + touch-required sudo (2026-08-06)

Decided with the operator 2026-08-06. Blocked on the same three items as the
rest of the PKCS#11 work — **#16** (`clevis-luks-pkcs11-askpin` has only ever
been syntax-checked, never executed), **#20** (two upstream clevis bugs likely
break real YubiKeys at root unlock), **#13** (QEMU usb-ccid gate before
hardware). Do not start on hardware until those clear.

### Measured state (2026-08-06)

- All three RPis (`172.16.2.45/.46/.47`, `rpi-serv-001/002/003`) are
  LUKS-encrypted on `/dev/nvme0n1p2` with ZFS root on top, `/etc/crypttab`
  carries the `clevis` option, and `clevis-luks` + `clevis-dracut` are
  installed.
- **No TPM on any RPi** — `/dev/tpm*` does not exist. Of the three factors in
  the settled design (tpm2 / tang / pkcs11) an RPi can only ever use two.
- **A YubiKey is already seated in each RPi** (`1050:0407`, OTP+U2F+CCID), but
  `pcscd` is **inactive**, so it is bound to nothing today.
- SSH to the RPis as **`ubuntu`** (not `jdfalk`). `sudo -n` is denied.

### Unconfirmed — read this before designing

The current unlock policy was **not** read: `sudo -n` was denied. No TPM plus
inactive `pcscd` leaves `tang` as the only pin they can be using, which would
mean the Tang servers depend on each other and a whole-house power loss is a
hard deadlock (all locked → no `tangd`, whose key material lives on the
encrypted root → nothing unlocks). Confirm before acting:

```
ssh ubuntu@172.16.2.45 'sudo cryptsetup luksDump --dump-json-metadata /dev/nvme0n1p2'
```

Ground truth is the JWE in the LUKS2 token — **not** `clevis luks list`, which
misrenders nested policies (drops shares, collapses arrays into bare objects).

### Target policy — `sss t=1` over two groups

- [ ] **Online group:** `tang` over `.45/.46/.47` at `t=2`. For a given RPi's
      own boot it is itself locked, so this means the other two must be up.
      Fine for rolling reboots, and stealing one RPi yields nothing — an
      attacker would need two more boxes on the LAN.
- [ ] **Cold-start group:** `t=2 { pkcs11(chassis nano), pkcs11(carried key) }`.
      **Two** token shares, not one. A permanently-seated nano is a factor a
      thief gets for free with the chassis; under nano+PIN alone a stolen RPi
      reduces to guessing one PIN. Requiring a carried key means the chassis
      yields one share and is useless without a key from the operator's pocket.
      Same shape as the len servers' G2 group; multi-share PIN handling already
      exists (the `clevis-decrypt-pkcs11` fork, #9).

**A LUKS passphrase cannot be AND-ed into this.** The operator asked for
"YubiKey + PIN + password". There is no passphrase pin in clevis — a passphrase
is a separate keyslot, and keyslots are OR'd, so adding one creates a
passphrase-only door that opens the volume with no token at all. The PIN
already *is* the password (`askpin` prompts for it at boot); the second token
is what adds real strength. See
`~/.claude/.../feedback_dont_drift_from_settled_design_constraints`.

### Touch-required sudo (`pam_u2f`)

- [ ] Add `pam_u2f` to `/etc/pam.d/sudo` with touch required. It uses the
      **FIDO/U2F applet**, which is separate from PIV, so it coexists with a
      PKCS#11 clevis binding on the same key — no conflict.
- [ ] **On the RPis this means physical-presence-only sudo**, since the key is
      inside the box. Accepted deliberately by the operator: a remote attacker
      who lands a shell cannot escalate unattended. Known limit — touch proves
      presence at *a* sudo, not *which* sudo, so a user-level attacker can
      piggyback on a legitimate touch (PATH-shadowed `sudo`, terminal hooking).
      What it reliably prevents is silent 3am escalation.
- [ ] Roll out on the len servers and the server (172.16.2.30) **first** —
      those have DASH/console recovery. Leave the RPis until the unlock work
      above is proven; do not create a box you can neither unlock nor sudo into.
- [ ] Breaks anything relying on `sudo -n`, including `scripts/server-deploy.sh`
      on the server. Budget for that before enabling it there.
- [ ] Roll out with `nullok` first, keep a documented root console path, and
      prove it in a VM before hardware (standing rule).
