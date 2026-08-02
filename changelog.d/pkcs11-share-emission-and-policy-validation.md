<!-- file: changelog.d/pkcs11-share-emission-and-policy-validation.md -->
<!-- version: 1.0.0 -->
<!-- guid: 3f8b21c9-5e47-4a06-9d13-c7a0e6b482f5 -->
<!-- last-edited: 2026-08-02 -->

### Added

#### PKCS#11 shares emit at any nesting depth, and unlock policies are now validated

`UnlockPin::Pkcs11` shares serialize into the clevis binding as
`"pkcs11":[{"uri":…},…]` — an array, one element per share, uniform with every
other pin kind — at arbitrary nesting depth. `SssPolicy::fleet_three_group`
builds the settled fleet policy: an outer `t=1` OR over three groups, so any one
of (both Tang peers) / (any 2 of 3 tokens) / (any one Tang AND either carried
key) unlocks the host. The lenserv variant additionally ANDs a `tpm2` share into
group 1; the RPis have no TPM and deliberately get none anywhere.

**The chassis-resident nano token is deliberately excluded from group 3.** A
thief who steals the server already holds it; if it counted there, reaching a
single Tang server would open the disk. Two tests fail if someone adds it back.

New `SssPolicy::validate`, wired into `validate_resolved` so it gates installs
rather than merely existing. It walks the whole tree recursively and reports
every violation at once:

- a `pin-value=` in a PKCS#11 URI — the URI is stored in the LUKS header in the
  clear, so a stored PIN silently reduces the factor to something-you-have
- a URI keyed on `slot-id=` with no `serial=` — slot IDs are reassigned between
  insertions, so the binding addresses the wrong token at the next boot
- a URI that is not an RFC 7512 `pkcs11:` URI
- a threshold outside `1 <= t <= shares` **at any level**, or an empty group;
  clevis enforces this per level and a nested violation kills the bind half-way
- the same Tang URL or token URI twice **within one level** — it counts twice
  toward `t` but can only be satisfied once. Deliberately per-level, never
  global: the fleet policy legitimately names the same peer and the same carried
  key in more than one group.

A no-op `1-of-1` nested wrapper warns rather than blocks.

### Fixed

#### The clevis 23 `openssl-provider-legacy` risk was never real

Measured on stock Ubuntu 26.04: 26.10 carries **two** builds of clevis 23. The
`-proposed` `23-1build1` is rebuilt against OpenSSL 4 and pulls `libssl4`; the
**release** `23-1` links against OpenSSL 3 and is satisfied by 26.04's own
`libssl3t64`. The original analysis read the proposed build. Pinning
`Suites: stonking` (release only) installs clevis 23-1 with `libssl4` never
pulled and `openssl-provider-legacy` untouched.

`libssl4` and `openssl-provider-legacy` are out of the apt pin allowlist, and a
new test asserts the generated sources file contains no `proposed` — that one
word is the difference between a clean install and an OpenSSL 4 migration.

### Documentation

`docs/research/2026-08-02-pkcs11-share-binding-hazard.md` — binding the token
group requires all three tokens plugged in simultaneously, which is exactly when
clevis's `pkcs11-tool -O | … | head -1` public-key lookup bites: if the slot
option fails to resolve from the URI, every share silently encrypts to the first
token's key, producing something that looks like a healthy 2-of-3 and is really
"one token, three times". Includes the mandatory post-bind verification matrix
(six PASS rows, six FAIL rows), each of which must be demonstrated
independently.
