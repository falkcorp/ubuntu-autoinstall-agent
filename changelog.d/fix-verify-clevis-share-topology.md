<!-- file: changelog.d/fix-verify-clevis-share-topology.md -->
<!-- version: 2.0.0 -->
<!-- guid: 8f3a1c47-6d92-4b0e-9a75-3e1c8b0d5f24 -->
<!-- last-edited: 2026-08-05 -->

### Fixed

- **Post-install verification now detects a Tang-satisfiable clevis policy —
  the exact vulnerability the check exists to prevent.** `clevis_binding` was
  validated by substring (`sss`, `"t":2`, every Tang URL appears somewhere), and
  in clevis's `sss` pin an array value contributes one share **per element**. So
  `{"t":2,"pins":{"tang":[a,b,c],"tpm2":{…}}}` is **2-of-4**, not
  `AND(tpm2, tang)` — the three Tang servers alone meet the threshold, and the
  TPM adds availability, not security. That string satisfied all three substring
  checks, so the verifier passed it and always had. The check now parses the
  policy and asserts the real **share arithmetic** rather than matching text.
  The legacy flat Tang-only policy on len-serv-001/002 still verifies clean and
  is reported as such. Failures name the computed share arithmetic and the fix.
  Empty, unparseable, or `sss`-less input fails closed, and one vulnerable
  keyslot fails the check even when another slot is correct.

  Two rules from this change's first draft were **superseded before release**
  and are deliberately not described above, because each of them rejected the
  policy the fleet actually settled on:

  - *"the outer share count must equal `t`"* — the settled policy's outer node
    is a `t=1` OR over three groups, so `shares == t` never holds. Replaced by
    the cheapest-satisfying-set count.
  - *"the nested Tang group's threshold must be `>= 2`"* — the settled policy's
    group 3 is `AND(any-one-Tang, either-carried-key)`, two inner `t=1` groups
    that are sound because the enclosing group requires both. A local rule
    against `t=1` cannot distinguish that from a genuine weakness.

  See `fix-verify-clevis-nested-tree.md` for the rule that replaced both, and
  `fix-tree-only-unlock-host-never-bound.md` for the change of data source from
  `clevis luks list` (which misrenders nested policies) to the JWE stored in the
  LUKS2 token.
