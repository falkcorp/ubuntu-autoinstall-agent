<!-- file: changelog.d/clevis-pkcs11-multitoken-pin-fork.md -->
<!-- version: 1.1.0 -->
<!-- guid: 6d1a3f92-84be-4c07-a5d1-30e9b7c2418f -->
<!-- last-edited: 2026-08-03 -->

### Added

#### Forked `clevis-decrypt-pkcs11` so two PKCS#11 tokens can unlock one device

`clevis/clevis-decrypt-pkcs11` and `clevis/clevis-luks-pkcs11-askpin` are forks
of clevis `23-1`, carried so that an `sss` group of the form
`{"t":2,"pins":{"pkcs11":[...]}}` can obtain a **separate secret PIN for each
token in a single unlock**, without any PIN reaching the LUKS2 header.

Neither upstream channel could do this. `pin-value=` in the PKCS#11 URI is
non-interactive but lands base64url-encoded in the JWE protected header inside
the LUKS2 metadata, where it is recoverable with no passphrase — measured, all
three PINs extracted. The interactive `/run/systemd/clevis-pkcs11.pin` serves
exactly one share, because `clevis-decrypt-pkcs11` deletes it after the first
successful decrypt and the second share then dies on
`error: invalid option(s) given`.

The fork replaces that one shared one-shot file with per-token state keyed on
`sha256(serial|token)` from each share's own URI, prompts per token with
`systemd-ask-password --timeout=` (fails closed, never hangs), and caps prompts
at 2 per token per boot so a typo storm cannot burn a PIV token's last retry.
Slot resolution is made mandatory, which additionally fixes an upstream silent
collapse where an unresolvable URI let every share bind to whichever token
enumerated first — making a 2-of-3 group unlock with one token inserted.
`clevis-decrypt-sss` was measured running its shares concurrently (three shares
entering within 10 µs), so the prompt and login are held under a `flock` and
every prompt names its token.

Proven on the VM gate with negatives first: one token present fails with zero
PIN attempts consumed; a wrong PIN fails with exactly one login per present
token; two different correct PINs succeed; and the PINs are absent from the
LUKS2 header under the same probe that recovers them from a `pin-value=`
binding. Not yet boot-proven in an initramfs and not yet wired into the
installer — see `docs/research/2026-08-03-clevis-pkcs11-multitoken-pin-fork.md`
for the reconciliation baseline hashes and the packaging proposal.
