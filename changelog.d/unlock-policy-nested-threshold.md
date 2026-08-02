<!-- file: changelog.d/unlock-policy-nested-threshold.md -->
<!-- version: 1.0.0 -->
<!-- guid: 59dda7cf-7fcc-4924-bc16-5a52ca7b434c -->
<!-- last-edited: 2026-08-02 -->

### Added

#### Nested/grouped clevis SSS unlock policies are now fleet-declarable

The unlock policy a host binds at install time could previously only be a FLAT
`t`-of-N over Tang servers. That shape cannot express "TPM2 **and** Tang",
because clevis counts one share per array element: `{"t":2,"pins":{"tang":
[a,b,c],"tpm2":{…}}}` is 2-of-**4**, so the three Tang servers alone satisfy it.
A true AND requires nesting, which collapses the whole Tang group to one share.

`SssPolicy`/`UnlockPin` (new,
`crates/uaa-core/src/network/ssh_installer/unlock_sss.rs`) model that as a
recursive `kind`-tagged tree in which **each element of `pins` is exactly one
share**, so the threshold arithmetic is just `pins.len()`. Profiles author it as
`unlock_policy.sss`; it resolves whole-value (host tier wins outright) and lowers
to the new `InstallationConfig::unlock_sss`. Three policies are expressible and
tested: legacy flat N-of-M Tang, `AND(tpm2, N-of-M tang)`, and an OR of several
PKCS#11 URIs usable as a single share inside an AND.

Hosts that author no tree — every committed config today, len-serv-001/002
included — lower to `unlock_sss: None`, serialize without the key, and keep
their existing flat behavior byte-for-byte. Nothing consumes the tree yet: the
clevis JSON emitter still builds the flat policy from `tang_servers`.
