<!-- file: todo.d/2026-08-06-rpi-tang-selfunlock-and-touch-sudo.md -->
<!-- version: 2.0.0 -->
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

- [x] **Online group:** `t=2 { pkcs11(chassis nano), sss{ tang t=1 } }` —
      **NOT** a bare `tang t=2`, which is what was originally drafted here and
      what the emitter actually shipped. A bare Tang group scores two shares
      and clears the share-count floor, but both shares are Tang, so two Tang
      keys decrypt the volume; with the outer `t=1` OR that made the WHOLE
      policy Tang-satisfiable. The nano fills the structural role a TPM plays
      on a lenserv: always seated, so available at boot with no human.

      Fixed in `SssPolicy::fleet_three_group`'s `None` arm and enforced by
      `verify::satisfiable_with_only` — see the changelog fragment
      `verifier-rejects-single-factor-kind.md`.

      Dropping the tang sub-threshold to `t=1` also answers the cold-start
      question: with `t=2` you must manually unlock **two** RPis before the
      third chains up (unlock #1; #2 still sees only one peer since it is
      itself locked and #3 is down; unlock #2; #3 then sees two). At `t=1`
      it is **one** manual unlock and the other two follow.
- [ ] **Cold-start group:** `t=2 { pkcs11(chassis nano), pkcs11(carried key) }`.
      **Two** token shares, not one. A permanently-seated nano is a factor a
      thief gets for free with the chassis; under nano+PIN alone a stolen RPi
      reduces to guessing one PIN. Requiring a carried key means the chassis
      yields one share and is useless without a key from the operator's pocket.
      Same shape as the len servers' G2 group; multi-share PIN handling already
      exists (the `clevis-decrypt-pkcs11` fork, #9).

### Token roster and rotation (operator, 2026-08-06)

Additional carried keys are planned, threshold stays at `t=2`, and one carried
key goes to an **offsite vault** as backup. Consequences that are easy to get
wrong:

- [ ] **Enrol the vault key BEFORE it leaves.** sss shares are fixed when the
      JWE is created; there is no way to add a share later without regenerating
      the keyslot. Bind the full set (nano + carried + vault) in one operation.
- [ ] **Binding with a token absent does not error.** It silently collapses the
      shares onto whichever tokens are present — `rc=0`, empty stderr. Binding
      while the vault key is already offsite therefore yields a weaker policy
      than intended and nothing says so. Run
      `uaa verify-policy --device <dev>` after every bind.
- [ ] **Keep the roster small.** With the nano permanently seated, the chassis
      always contributes one of the two required shares, so every additional
      token is another single item that completes the set for whoever holds the
      box. Three or four total, deliberately chosen.
- [ ] **Revoking a lost key is also a re-bind**, with the same all-tokens-present
      requirement — i.e. a trip to the vault. Decide now whether the vault key is
      backup-only or routine; it changes how often that trip happens.
- [ ] Known gap: `verify-policy` has **no notion of factor independence within a
      kind**. It now rejects tang-only and tpm2-only policies, but it cannot tell
      that one of two `pkcs11` shares is a token living permanently inside the
      machine. That judgement stays with the operator.

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
