<!-- file: docs/research/2026-08-02-pkcs11-share-binding-hazard.md -->
<!-- version: 1.0.0 -->
<!-- guid: 9c1f4a72-6d05-4b38-a4e7-1f2c8b3d5e60 -->
<!-- last-edited: 2026-08-02 -->

# The `head -1` hazard: binding three tokens and getting one token three times

**Status:** open hazard in clevis itself. Not fixable from this repo. Mitigated
only by the post-bind verification matrix at the end of this document, which is
**mandatory** and **not optional**, and which no amount of green unit tests can
replace.

## What the policy needs

The settled fleet policy (see `SssPolicy::fleet_three_group`) contains a group
that is "any 2 of 3 PKCS#11 tokens":

```json
{"t":2,"pins":{"pkcs11":[
  {"uri":"pkcs11:serial=NANO0001"},
  {"uri":"pkcs11:serial=CARRIED0A"},
  {"uri":"pkcs11:serial=CARRIED0B"}
]}}
```

Each element is one Shamir share. clevis's `pkcs11` pin **encrypts each share to
that token's own public key**. So binding this policy requires **all three
tokens plugged into the machine at the same time**. There is no incremental
path: you cannot bind the nano today and the carried keys tomorrow, because
`clevis luks bind` writes one keyslot for the whole policy in one shot.

That "all three plugged in at once" requirement is not an inconvenience. It is
the precondition for the failure below.

## What actually happens at bind time

`clevis-encrypt-pkcs11` does roughly this, per share:

1. Generate a random A256GCM JWK.
2. Find the token's **existing** public key:
   ```sh
   pkcs11-tool ${slot_opt} -O | grep -i 'Public' -A10 | grep 'ID:' | head -1
   ```
3. `pkcs11-tool ${slot_opt} --read-object --type pubkey --id <id>`
4. Encrypt the JWK to that public key.

Note what step 2 does **not** do: it does not verify that the object it found
belongs to the token named by the URI. It takes the first public key in the
enumeration.

`${slot_opt}` is derived from the URI. **If it fails to resolve** — a
serial-less URI, a `slot-id=` that no longer exists, a token that enumerated
differently than expected — then `pkcs11-tool -O` runs with **no slot filter**
and enumerates **every slot on the machine**. `head -1` then picks the first
public key across all of them.

With all three tokens plugged in, as the policy requires, that means:

> **All three shares silently encrypt to the first token's public key.**

The bind **succeeds**. The keyslot is written. `clevis luks list` shows a
healthy-looking 2-of-3 `pkcs11` group. Nothing errors, nothing warns, and
nothing in the LUKS header records which key each share actually went to.

What you have built is not "any 2 of 3 tokens". It is **"one token, three
times"**: token A alone satisfies `t=2` (it can decrypt all three shares), and
the carried keys are worthless. Every property the group was supposed to
provide — that losing one token is survivable, that the offsite spare is a real
spare — is gone, and the failure is invisible until the day you need it.

This is the same defect class the validator's duplicate-share rule catches
statically (`policy.pins` naming the same URI twice). The difference is that
this one is produced by the **tooling at bind time**, from a policy that is
perfectly correct on paper. Static validation cannot see it. Only unlocking can.

### Why nothing else catches it

- The emitted JSON is correct — the golden tests in `system_setup.rs` assert it,
  and they pass.
- `SssPolicy::validate` rejects `slot-id=`-without-`serial=` URIs, which removes
  the most common cause. It cannot rule out the others.
- A single successful unlock proves nothing. If you bind three tokens and then
  unlock with `{nano, carriedA}`, that succeeds *both* when the bind was correct
  *and* when everything encrypted to the nano's key.

## Mitigations before binding

1. **Key every URI on `serial=`, never `slot-id=`.** Slot IDs are assigned at
   enumeration time and shift between insertions. `SssPolicy::validate` rejects
   a `slot-id=`-only URI for exactly this reason.
2. **Never put `pin-value=` in a URI.** The URI is stored in the LUKS header in
   the clear; a stored PIN reduces the factor to something-you-have. Also
   rejected by `SssPolicy::validate`.
3. **Confirm each serial before binding**, with all three tokens inserted:
   ```sh
   pkcs11-tool --list-slots
   pkcs11-tool --slot-index 0 -O
   ```
   Every serial in the policy must appear, and each must show its **own** public
   key object.
4. **Nothing is written to the tokens.** The bind reads the *existing* public
   key; it does not generate a keypair, install a cert, or consume a slot. One
   keypair per token serves any number of servers. So a failed bind costs
   nothing on the token side and can be retried freely.

## Post-bind verification matrix — MANDATORY

After binding, and before the host is trusted or put into service, **every PASS
row below must be demonstrated INDEPENDENTLY**.

Independently means: one reboot (or one `clevis luks unlock` against the
keyslot) per row, with **only** the listed factors present and every other
factor physically removed or made unreachable. Unlocking with `{nano, carriedA}`
tells you nothing about whether `{carriedA, carriedB}` bound to two distinct
keys — that is the entire point of the hazard.

### Must unlock (PASS)

| # | Factors present | Which group satisfies it |
|---|---|---|
| 1 | 2 Tang peers | group 1 |
| 2 | nano + carriedA | group 2 |
| 3 | nano + carriedB | group 2 |
| 4 | carriedA + carriedB | group 2 |
| 5 | 1 Tang + carriedA | group 3 |
| 6 | 1 Tang + carriedB | group 3 |

Row 4 is the one that catches the `head -1` bug, because it is the only PASS row
that excludes the nano — and the nano, being first in the array, is the token
everything wrongly encrypts to. **If row 4 fails while rows 2 and 3 pass, you
have the bug.** Do not "fix" it by adding the nano to group 3.

### Must NOT unlock (FAIL)

| # | Factors present | Why it must fail |
|---|---|---|
| 7 | 1 Tang + nano | group 3 excludes the nano — see below |
| 8 | 1 Tang alone | group 1 needs both peers |
| 9 | nano alone | group 2 needs 2 of 3 |
| 10 | carriedA alone | group 2 needs 2 of 3 |
| 11 | carriedB alone | group 2 needs 2 of 3 |
| 12 | nothing | — |

Row 7 is a **security property, not an arithmetic accident**. The nano lives
permanently in the chassis, so anyone who steals the server is already holding
it. If the nano counted toward group 3, that thief would need only to reach ONE
Tang server — trivial while the box is still on the LAN, or via any single
compromised or rehosted Tang — and the disk would open. Restricting group 3 to
the CARRIED keys means physical possession of the chassis is never, by itself,
one of the two factors.

If row 7 **unlocks**, either someone added the nano to group 3 (the tests
`test_nano_is_excluded_from_group_three` in `system_setup.rs` and
`test_fleet_constructor_excludes_the_nano_from_group_three` in `unlock_sss.rs`
exist to stop that) or the `head -1` bug has made the nano able to decrypt
carried-key shares. Either way the binding must be destroyed and redone.

### Recording the result

All twelve rows, pass/fail, with the date and the token serials, into the host's
status report. A partially-verified binding is an unverified binding.

## References

- `crates/uaa-core/src/network/ssh_installer/unlock_sss.rs` — the policy type,
  `fleet_three_group`, and `validate`.
- `crates/uaa-core/src/network/ssh_installer/system_setup.rs` —
  `build_clevis_policy_from_tree` and the golden tests.
- `docs/research/2026-08-02-clevis23-pkcs11-pinning-risk.md` — getting a clevis
  with the pkcs11 pin onto 26.04 in the first place.
