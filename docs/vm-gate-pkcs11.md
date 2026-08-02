<!-- file: docs/vm-gate-pkcs11.md -->
<!-- version: 1.0.0 -->
<!-- guid: dc19836c-37bd-4fd3-a4d5-a65086a976b9 -->
<!-- last-edited: 2026-08-02 -->

# Clevis PKCS#11 gate (SoftHSM2)

The fleet unlocks LUKS2 roots via clevis. We are adding a **PKCS#11 pin**
(YubiKey PIV) as a break-glass unlock factor. There is no YubiKey available
to test against right now, so this gate uses **SoftHSM2** as a software
PKCS#11 token to prove the *mechanism* end to end.

This gate is a **companion to** [`docs/vm-validation.md`](vm-validation.md),
not a replacement. `scripts/vm-validate.sh` boots a whole VM and proves an
install; the scripts here prove unlock **policy logic** and **initramfs
contents**, fast and repeatably, without a boot.

## The design under test

Three LUKS keyslots. Keyslots are OR — any one of them unlocks the volume.

| Slot | Policy | Purpose |
|---|---|---|
| A | `{"t":2,"pins":{"tpm2":{"pcr_ids":"7","pcr_bank":"sha256"},"sss":[{"t":2,"pins":{"tang":[a,b,c]}}]}}` | Unattended boot |
| B | `{"t":1,"pins":{"pkcs11":[uriA,uriB]}}` | Break-glass, **no Tang dependency** |
| 0 | passphrase | Last resort |

Slot A is **nested on purpose**. A flat
`{"t":2,"pins":{"tang":[a,b,c],"tpm2":{}}}` is 2-of-**four** shares, and Tang
alone satisfies it — measured, not assumed. True AND across "TPM2" and "a Tang
quorum" requires the inner `sss`. The gate asserts both halves of that claim.

### Why the gate checks topology, not substrings

`evaluate_clevis_binding` (`crates/uaa-core/src/autoinstall/verify.rs:257`)
validates a binding by substring: `contains("sss")`, `contains("\"t\":2")`, and
every Tang URL present. **The broken flat config satisfies all three, and so
does the correct nested one** — the existing verifier cannot distinguish them.
It is worse than it looks: the check runs against the whole `clevis luks list`
line, which begins `1: sss '…'`, so the literal *pin name* satisfies
`contains("sss")` even for a policy with no nested `sss` at all.

So this gate asserts on the **actual share topology** — it parses the pin JSON
and rejects any binding where `tang` is a direct child of the outer `pins`
object — and pairs that with a behavioural control: with the TPM absent and all
Tang up, the nested policy must **fail** to unlock while the flat one still
opens. `todo.d/2026-08-02-clevis-binding-topology-verifier.md` tracks porting
the predicate back into `verify.rs`.

## The three tiers

| Tier | What it proves | Script | Needs a boot? |
|---|---|---|---|
| 1 | Unlock policy logic, negative controls | `scripts/vm-gate/pkcs11-clevis-gate.sh` | No — loopback LUKS containers |
| 2 | The boot-time askpass path actually runs | `scripts/vm-validate.sh` | Yes |
| 3 | The initramfs contains what the pins need | `scripts/vm-gate/verify-initramfs.sh` | No — read-only, no root |

Tier 1 runs against **loopback LUKS2 containers the harness creates itself**.
That is what makes the negative controls cheap enough to be non-skippable: a
negative control that costs a VM boot gets skipped, and a gate whose red path
is never exercised proves nothing.

## Files

```
scripts/vm-gate/softhsm-setup.sh      provision a throwaway SoftHSM2 token, print its PKCS#11 URI
scripts/vm-gate/pkcs11-clevis-gate.sh the assertion harness (tier 1)
scripts/vm-gate/verify-initramfs.sh   pre-reboot initramfs content check (tier 3)
```

## Prerequisites

**Linux only, root required** (losetup + `cryptsetup luksFormat`). Run it
inside the gate VM, or on any throwaway amd64 Linux box. macOS is refused at
preflight.

```bash
apt install -y softhsm2 opensc cryptsetup-bin clevis clevis-luks \
                clevis-tpm2 clevis-pin-tpm2 tpm2-tools tang jose curl jq \
                swtpm swtpm-tools dracut-core
```

`clevis-tpm2` is called out deliberately: **it is not installed on the live
fleet today**, which is why the production initramfs contains clevis, tang,
sss and libtss2 but zero `clevis-decrypt-tpm2`. The harness fails at preflight
if it is missing, rather than quietly asserting less.

Every tool in the preflight list is **required**. There is no skip state
anywhere in this gate; a missing prerequisite is a FAIL.

## Running it

### 1. Provision a token (usually done for you)

`pkcs11-clevis-gate.sh` calls this itself, but it is useful standalone when
you want a URI to paste into a profile:

```bash
./scripts/vm-gate/softhsm-setup.sh --workdir ./vm-gate-work --label uaa-gate
```

It writes an isolated `SOFTHSM2_CONF` under `--workdir` so the system token
store (`/var/lib/softhsm/tokens`) is never touched, then prints:

```
SOFTHSM2_CONF=/…/vm-gate-work/softhsm2.conf
SOFTHSM_MODULE=/usr/lib/softhsm/libsofthsm2.so
TOKEN_LABEL=uaa-gate
TOKEN_PIN=123456
PKCS11_URI_1=pkcs11:token=uaa-gate;id=%01;object=uaa-gate-key1?module-path=/usr/lib/softhsm/libsofthsm2.so
```

`--keypairs 2` produces the two-keypair token used as the "first public key
wins" fixture. **A token backing a real Slot B URI must contain exactly one
keypair** — see below.

### 2. Run the gate

```bash
sudo ./scripts/vm-gate/pkcs11-clevis-gate.sh --workdir ./vm-gate-work
```

Everything it creates lives under `--workdir`: LUKS container images, the
SoftHSM token store, three Tang key databases, swtpm state, and per-stage logs
in `<workdir>/logs/`. It is always safe to `rm -rf` that directory.

`--stages <list>` (`neg,slota,slotb,gotcha,regen`) exists for iteration only.
Any value other than the default `all` prints **`GATE: PARTIAL`** and exits
non-zero, so a partial run can never be mistaken for a passing gate.

### 3. Verify an initramfs before rebooting anything

```bash
# What the fleet needs today:
./scripts/vm-gate/verify-initramfs.sh --image /boot/initrd.img-$(uname -r) --pin tpm2

# The full new policy:
./scripts/vm-gate/verify-initramfs.sh --image ./initramfs.img --pin all
```

Read-only, no root, exits non-zero listing **every** missing item. Against a
**synthetic image built to match the measured fleet contents** (clevis + tang +
sss + libtss2, no `clevis-decrypt-tpm2`), `--pin tpm2` correctly reports:

```
MISSING file     clevis-decrypt-tpm2  -- the tpm2 pin — ABSENT ON THE LIVE FLEET …
INITRAMFS: MISSING 4 of 10 required items — DO NOT REBOOT
```

## The assertions, and their polarity

Negative controls run **first**. If any of them fails to go red, the harness
declares itself confounded and exits without running a single positive
assertion — a green result below a broken red path is worthless.

| ID | Assertion | Expected |
|---|---|---|
| `pos-00` | Plain passphrase unlock of a fresh container | **OK** — control; if this fails every red below is meaningless |
| `pos-00b` | **PIN-delivery control**: same token, **correct** PIN unlocks | **OK** — without this, `neg-01` could go red merely because clevis received no PIN at all |
| `neg-01` | Unlock with the **wrong PIN** | **FAIL** |
| `neg-02` | Unlock with the **token absent** (empty token store) | **FAIL** |
| `neg-03` | Slot A with **2 of 3 Tang down** — inner quorum unmet | **FAIL** |
| `pos-10` | Nested Slot A: no `tang` as a direct child of outer `pins` | **OK** |
| `neg-05` | Same topology check against the **flat** config | **FAIL** — proves the check has teeth |
| `pos-11` | `verify.rs`'s substring predicates run against the **broken flat** binding | **OK** — i.e. they *pass*, demonstrating the gap |
| `pos-09` | **Witness**: flat config unlocks with **TPM absent**, all Tang up | **OK** — proves it really is 2-of-4, and that "TPM absent" is not simply breaking everything |
| `neg-04` | **Nested Slot A with TPM absent, all Tang up** | **FAIL** — Tang alone must never open Slot A |
| `pos-01` | Slot A, all Tang up | **OK** |
| `pos-02` | Slot A, **1 of 3 Tang down** | **OK** |
| `pos-03` | Slot B, **ALL Tang down** | **OK** — this is the entire point of Slot B |
| `pos-04` | Two-keypair token, bind with `id=01` (control) | **OK** |
| `gotcha-01` | Two-keypair token, bind with `id=02` | tri-state, see below |
| `pos-05` | Slot B's token has exactly one keypair | **OK** |
| `pos-06` | Regenerated initramfs passes `verify-initramfs.sh --pin all` | **OK** |
| `pos-07` | Slot A still unlocks after initramfs regeneration | **OK** |
| `pos-08` | Slot B still unlocks after initramfs regeneration | **OK** |

Every line is printed with its polarity visible, so a reviewer can see at a
glance which rows were *supposed* to be red:

```
ASSERT neg-01     wrong-pin-unlock-fails             EXPECT=FAIL OBSERVED=FAIL RESULT=PASS
ASSERT pos-03     slotB-all-tang-down                EXPECT=OK   OBSERVED=OK   RESULT=PASS
GATE: PASS
```

`RESULT=PASS` means *observed matched expected*, not *the command succeeded*.

Not every nonzero exit counts as a legitimate red. A timeout (124) or a
missing/non-executable binary (125-127) means the assertion never actually
ran, so it is recorded as `OBSERVED=INDET` and **always** fails the gate —
otherwise an `EXPECT=FAIL` row could pass because the command was misspelled.

### The "first public key wins" gotcha (`gotcha-01`)

`clevis-encrypt-pkcs11` picks the key to encrypt to with

```sh
pkcs11-tool -O | grep -i 'Public' -A10 | grep 'ID:' | head -1
```

— the **first** public key on the token, ignoring `id=`/`object=` in the URI.
`clevis-decrypt-pkcs11` **does** honour the URI. On a token with two keypairs,
binding with `id=02` therefore encrypts to key 01's public half and tries to
decrypt with key 02's private half: **bind succeeds, unlock fails**, silently
targeting the wrong key.

The assertion is deliberately tri-state, because "upstream fixed it" is not a
gate failure:

| Verdict | Meaning | Gate |
|---|---|---|
| `DETECTED` | bind OK, unlock fails | expected on clevis 23 — reported, gate passes |
| `ABSENT` | bind OK, unlock OK | upstream appears fixed — reported, gate passes |
| `INDETERMINATE` | the bind itself errored | **the only FAIL** — cannot distinguish "gotcha present" from "harness broken" |

**Operational consequence, regardless of verdict: the token backing a real
Slot B URI must contain exactly ONE keypair.** `pos-05` asserts that as a
precondition so nobody ships a multi-keypair Slot B.

## The kernel-update / initramfs-regeneration assertion

A real kernel upgrade cannot be done non-destructively in tier 1, and
`dracut -f` over `/boot` is destructive. So `pos-06`/`pos-07`/`pos-08`
regenerate the initramfs for the **running** kernel into a scratch file under
`--workdir`, verify its contents, and re-assert that the existing bindings
still unlock. That catches the real regression this assertion exists for: a
regeneration that drops the clevis pin modules.

A true kernel **upgrade** must still be proven by a full boot in
`scripts/vm-validate.sh`. Flagged, not hidden.

## What this gate CANNOT prove

Read this before treating a green run as permission to reboot hardware.

1. **SoftHSM is not a YubiKey.** SoftHSM enforces no *hardware*, destructive
   PIN retry counter, has no touch/presence policy, and no PIV applet
   semantics. `neg-01` proves clevis rejects a wrong PIN; it does **not**
   prove how a real token behaves after three wrong PINs. (For this reason
   the harness sacrifices a dedicated throwaway token to the wrong-PIN
   control, so a soft PIN lockout cannot poison later assertions.)
2. **The pcscd/CCID stack is never exercised.** SoftHSM is a direct `.so`
   load; a real YubiKey is reached through pcscd. Green here does not mean a
   real token works. `verify-initramfs.sh` checks the pcsc pieces are *in the
   image*, which is the most this tier can do.
3. **`neg-04` proves the policy, not the hardware.** The TPM is removed by
   killing the local `swtpm`; a real machine loses its TPM differently (PCR
   mismatch, not an absent device). The share arithmetic is what is under
   test.
4. **swtpm PCR7 values do not match real hardware.** The gate validates that
   the tpm2 share participates in the policy with the right threshold
   arithmetic. It says nothing about whether the PCR7 measurement on a real
   Secure Boot machine will match at boot.
5. **A userspace `clevis luks unlock` does not exercise the boot-time askpass
   path.** `clevis-luks-pkcs11-askpass.socket` writing
   `/run/systemd/clevis-pkcs11.pin` only happens in the initramfs. A green
   tier 1 tells you the *policy* is right and says nothing about whether the
   machine will actually boot. **This is the entire justification for
   `verify-initramfs.sh` existing** — and even that checks contents, not that
   the socket unit is enabled.
6. **The `lsinitrd -m` dracut-module path is not yet exercised.** The
   `50clevis-pin-pkcs11` check resolves through `lsinitrd -m` on a real dracut
   image; the synthetic smoke test exercised only the `cpio-fallback` lister.
   Treat that one requirement as unvalidated until the verifier has been run
   against a real dracut-built initramfs.
7. **Local Tang is not fleet Tang.** The harness runs three `tangd` on
   `127.0.0.1`. It never contacts `172.16.x.x` and will hard-fail if asked
   to. Real Tang quorum health is a separate pre-flight.

In one sentence: **this gate validates LOGIC and INITRAMFS CONTENTS, not
hardware-specific values.**

## Safety guarantees

- **No `--device` flag exists** on `pkcs11-clevis-gate.sh`. Every LUKS
  container is a sparse file the script creates under `--workdir` and
  attaches with `losetup`. It is structurally impossible to aim it at a real
  disk. As defence in depth it still verifies each device is `TYPE=loop` and
  backed by a file under `--workdir` before `luksFormat` touches it.
- Every script hard-fails on any argument matching `172.16.` or naming
  `/dev/sd*`, `/dev/nvme*`, `/dev/vd*`, `/dev/hd*`, `/dev/mapper/*`,
  `/dev/disk/*`, or a system `--workdir`.
- `softhsm-setup.sh` generates its own `SOFTHSM2_CONF` and refuses to run if
  the ambient one points at the system token store.
- `pkcs11-clevis-gate.sh` refuses to start if `/run/systemd/clevis-pkcs11.pin`
  already exists — that would mean a real pkcs11-unlock host.
- Only PIDs the harness started are ever killed. It never `pkill`s by name;
  the host may be running other VMs. Same rule as `scripts/vm-validate.sh`.
