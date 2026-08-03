<!-- file: changelog.d/fix-verify-clevis-share-topology.md -->
<!-- version: 1.0.0 -->
<!-- guid: 8f3a1c47-6d92-4b0e-9a75-3e1c8b0d5f24 -->
<!-- last-edited: 2026-08-02 -->

### Fixed

- **Post-install verification now detects a Tang-satisfiable clevis policy —
  the exact vulnerability the check exists to prevent.** `clevis_binding` was
  validated by substring (`sss`, `"t":2`, every Tang URL appears somewhere), and
  in clevis's `sss` pin an array value contributes one share **per element**. So
  `{"t":2,"pins":{"tang":[a,b,c],"tpm2":{…}}}` is **2-of-4**, not
  `AND(tpm2, tang)` — the three Tang servers alone meet the threshold, and the
  TPM adds availability, not security. That string satisfied all three substring
  checks, so the verifier passed it and always had. The check now parses the
  `clevis luks list` JSON and asserts the real **share topology**: `tang` must
  not be a direct child of the outer `pins` when another pin is present, the
  outer share count must equal `t` for an AND, and the nested Tang group's
  threshold must be `>= 2` and satisfiable. The legacy flat Tang-only policy on
  len-serv-001/002 still verifies clean and is reported as such. Failures name
  the computed share arithmetic and the fix. Empty, unparseable, or `sss`-less
  output now fails closed, and one vulnerable keyslot fails the check even when
  another slot is correct.
