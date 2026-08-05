<!-- file: changelog.d/fix-nested-sss-tang-and-tpm2.md -->
<!-- version: 1.0.0 -->
<!-- guid: 3f1c7a92-6b04-4d58-9e2a-0c7d5b8f1a34 -->
<!-- last-edited: 2026-08-02 -->

### Fixed

- **The NativeKeystore D2-B clevis policy was `tang OR tpm2`, not `tang AND
  tpm2`.** The SSS config was emitted flat as
  `{"t":2,"pins":{"tang":[a,b,c],"tpm2":{…}}}`, but in clevis SSS an array pin
  contributes **one share per element** — so that policy was really 2-of-**4**
  and the three Tang servers alone met the threshold. The tpm2 share was
  decorative: anything that could reach two Tang servers could unseal without
  the TPM. The policy is now **nested**, wrapping the Tang group in a one-share
  inner `sss` so the outer `t=2` runs over exactly two shares (`tpm2`, Tang
  group) and both are genuinely required:
  `{"t":2,"pins":{"tpm2":{…},"sss":[{"t":N,"pins":{"tang":[…]}}]}}`.

  Deliberate availability trade-off: under the old flat policy a host whose TPM
  failed or whose PCR7 changed (firmware update, Secure Boot key rotation) still
  unlocked from Tang alone; under the nested policy it will not unlock at all.
  That is the intended AND semantics. This is **install-time code generation
  only** — no already-bound host is re-bound by this change.

### Changed

- **Clevis SSS policy JSON is now built by a pure, unit-tested emitter**
  (`SystemConfigurator::build_clevis_sss_config`) using `serde_json` instead of
  ad-hoc `format!` string concatenation, with the Tang advertisement pre-fetch
  left where it was (it is load-bearing for non-interactive `clevis luks bind`).
  The legacy Tang-only shape `{"t":N,"pins":{"tang":[…]}}` is **byte-identical**
  to the previous output — the production binding on len-serv-001/002 is
  regression-tested against the original `format!` expression kept verbatim as a
  test oracle, plus hardcoded golden literals.
