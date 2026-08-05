<!-- file: changelog.d/fix-installer-luks2-metadata-and-pkcs11-uris.md -->
<!-- version: 1.0.0 -->
<!-- guid: 5c1e7a92-4f38-4d6b-b0a1-9e2d7c4f83b6 -->
<!-- last-edited: 2026-08-03 -->

### Fixed

- **Every LUKS2 volume is now formatted with a 1 MiB metadata area, so the
  fleet's nested-`sss` clevis binding actually fits.** The metadata area is
  chosen at `cryptsetup luksFormat` time and can never be grown afterwards, and
  the default is 16 KiB — while our nested policy produces a JWE of roughly
  22 KB. The binding therefore failed with `Failed to import token from file.`
  on a host whose pools were already built, and no amount of post-hoc repair
  could fix it. Measured on cryptsetup 2.8.4: a 22064-byte token import fails
  into a default header and succeeds at `--luks2-metadata-size 1m`, with the
  metadata growth taken out of the keyslots area (14 MiB remaining, ample) and
  the payload offset **unchanged** at 16 MiB. `--luks2-keyslots-size` is
  deliberately *not* raised — measured, it moves the data offset to 6 MiB for no
  benefit. The flags are one shared constant applied to the rpool partition, the
  `rpool/keystore` zvol, and the image-deployer path.

- **A PKCS#11 token URI must now carry both `serial=` and `token=`, and is
  rejected at config-parse time if it does not.** Measured against clevis 23:
  `clevis_get_pkcs11_final_slot_from_uri` returns rc=1 for a `serial=`-only URI,
  so no slot is selected and `pkcs11-tool -O | head -1` wins — every share
  silently binds to whichever token enumerates first, at rc=0, with no warning.
  A cross-decrypt matrix over the fleet's five shares found all five bound to a
  single token: five "independent" factors that were one factor wearing five
  hats. `clevis_get_slot_by_serial_and_token_from_uri` is the only resolver that
  works for our tokens (`clevis_get_slot_by_serial_from_uri` invokes
  `pkcs11-tool` without `--module` and is dead for any non-OpenSC provider), and
  it needs both attributes. A URI that cannot resolve a slot is now a hard
  error, never a warning. The attribute check is structural — the URI is split
  into RFC 7512 path and query attributes and each is split on its first `=` —
  so a `token=` buried inside another attribute's value does not satisfy it.

- **Every emitted `pkcs11` pin now carries `"mechanism":"RSA-PKCS"`.** Omit it
  and the policy binds cleanly and then never unlocks: `clevis-decrypt-pkcs11`
  fails in the initramfs with `error: Decrypt mechanism not supported`, i.e. a
  green install and a host that never boots. The field defaults on
  deserialization and the emitter substitutes the default for any `None` that
  reaches it, so a binding can no longer be written without one. The field stays
  `Option<String>` with `skip_serializing_if`, which remains load-bearing in the
  other direction: `"mechanism": null` is not something clevis can use at all.
  An explicitly empty mechanism is now a validation error.

- **Nothing parses `clevis luks list` any more — it misrepresents nested
  policies.** Measured by binding a known tree to a loopback LUKS2 volume and
  reading back what the tool printed: an authored
  `{"t":2,"pins":{"null":[{}],"sss":[{"t":2,"pins":{"tang":[a,b,c]}}]}}` came
  back as `{"t":2,"pins":{"sss":{"t":2,"pins":{"tang":[a,b,c]}}}}` — the `null`
  share dropped entirely and the `sss` **array** flattened to a bare object. A
  second tree with two nested groups rendered as
  `{"t":1,"pins":{"sss":{"t":1,"pins":{}}}}`, losing every leaf. That corruption
  is not cosmetic: it changes the verdict, and the post-install verifier
  condemned a correctly-bound volume as unsatisfiable because the share
  arithmetic it read was fiction. Verification now reads
  `cryptsetup luksDump --dump-json-metadata` and reconstructs the policy tree
  from the JWE stored in the LUKS2 token — the same bytes clevis itself decrypts
  at boot. The autoinstall template's idempotency guard likewise asks
  cryptsetup's own token listing whether a clevis token is bound, rather than
  reading clevis's rendering of the policy.
