<!-- file: changelog.d/vm-gate-softhsm-pkcs11.md -->
<!-- version: 1.1.0 -->
<!-- guid: 3f938e81-029d-4282-be59-5b208d3615b2 -->
<!-- last-edited: 2026-08-02 -->

### Added

#### SoftHSM-backed clevis PKCS#11 gate, with negative controls that run first

New `scripts/vm-gate/` harness proving the three-keyslot unlock design (Slot A
unattended `tpm2 AND 2-of-3 tang`, Slot B break-glass `pkcs11`, Slot 0
passphrase) before any host reboots. No YubiKey is available to test with, so
`softhsm-setup.sh` provisions a throwaway SoftHSM2 token in an isolated
`SOFTHSM2_CONF` under the workdir and prints the PKCS#11 URI clevis expects.

`pkcs11-clevis-gate.sh` runs the assertions against **loopback LUKS2
containers it creates itself** — there is deliberately no `--device` flag, so
it cannot be aimed at a real disk — with three local `tangd` on `127.0.0.1`
and a local `swtpm`. Negative controls run **first and are not skippable**:
wrong PIN, token absent, and Slot A with the inner Tang quorum unmet must all
go red, and if any of them does not, the run is declared confounded and exits
before a single positive assertion. Every report line carries its expected
polarity (`EXPECT=FAIL OBSERVED=FAIL RESULT=PASS`), so a reviewer can see
which rows were supposed to be red. There is no SKIP state anywhere: a missing
prerequisite is a FAIL. Slot A is asserted to unlock with all Tang up and with
1 of 3 down, and to fail with 2 of 3 down; Slot B is asserted to unlock with
**all** Tang down, which is its entire purpose. The clevis 23 "first public
key wins" bug — `clevis-encrypt-pkcs11` selects the first public key on the
token and ignores `id=` in the URI, while decrypt honours it — is detected
with a tri-state verdict plus a same-token positive control, and a companion
assertion enforces that a Slot B token holds exactly one keypair.

#### Pre-reboot initramfs verifier catches the missing-`clevis-decrypt-tpm2` bug class

`scripts/vm-gate/verify-initramfs.sh` is a read-only, no-root check that a
given initramfs actually contains what the configured pins need, listing
**every** missing item rather than the first. It exists because the live fleet
initramfs contains clevis, tang, sss and libtss2 but **zero**
`clevis-decrypt-tpm2` (the `clevis-tpm2` package was never installed), and
every check that only looked for "clevis" passed. It checks the
`50clevis-pin-pkcs11` dracut module, `clevis-decrypt-pkcs11`,
`clevis-decrypt-tpm2`, `pkcs11-tool`, `clevis-luks-pkcs11-askpass`, the
PKCS#11 provider `.so`, and the pcsc/CCID stack a real YubiKey needs.
`docs/vm-gate-pkcs11.md` documents the operator walkthrough and states plainly
what the gate cannot prove: SoftHSM is not a YubiKey, swtpm PCR7 is not real
hardware PCR7, and a userspace `clevis luks unlock` does not exercise the
boot-time askpass path — the gate validates logic and initramfs contents, not
hardware-specific values.

#### Gate asserts clevis share TOPOLOGY, closing a measured hole in `evaluate_clevis_binding`

`crates/uaa-core/src/autoinstall/verify.rs:257` validates a clevis binding by
substring — `contains("sss")`, `contains("\"t\":2")`, and every Tang URL
present. The **broken flat** policy `{"t":2,"pins":{"tang":[a,b,c],"tpm2":{}}}`
satisfies all three, and so does the **correct nested** AND, so the verifier
cannot tell them apart; the flat form is 2-of-**four** shares and Tang alone
opens it with no TPM. The `contains("sss")` check is additionally satisfied by
the literal pin name in the `1: sss '…'` line prefix, independent of nesting.

The gate now parses the pin JSON and rejects any binding where `tang` is a
direct child of the outer `pins` object (`pos-10` green on nested, `neg-05` red
on flat, so the check demonstrably has teeth), and records the gap explicitly
(`pos-11`: the substring predicates *pass* on the broken flat binding). It is
paired with a behavioural control run with the TPM absent and all Tang up:
`pos-09` witnesses that the flat config still unlocks — proving the simulation
is not simply breaking everything — while `neg-04` requires the nested Slot A to
**fail**. Porting the topology predicate back into `verify.rs` is tracked in
`todo.d/2026-08-02-clevis-binding-topology-verifier.md`; this change touches no
Rust.
