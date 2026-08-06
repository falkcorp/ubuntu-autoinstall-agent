<!-- file: changelog.d/verifier-rejects-single-factor-kind.md -->
<!-- version: 1.0.0 -->
<!-- guid: 4d81f36a-92b7-4e05-a1c8-70e5b2947d13 -->
<!-- last-edited: 2026-08-06 -->

### Security

- **A policy satisfiable by one KIND of factor is now rejected, and the emitter
  no longer produces one.** The verifier's rule was "no single *share* opens the
  volume" (`min_satisfying_shares >= 2`). That is strictly weaker than what the
  fleet actually requires, and the two come apart exactly where it matters:

  ```
  {"t":2,"pins":{"tang":[a,b,c]}}     // scores 2 shares — and 2 Tang keys open it
  ```

  New `verify::satisfiable_with_only` asks whether any satisfying set can be
  assembled from pins of a single kind, and `tang`-only or `tpm2`-only policies
  now fail with a message naming the branch to fix. `pkcs11` is deliberately
  exempt: two Tang shares are network services on one LAN and two `tpm2` shares
  are one soldered chip counted twice, so a single compromise yields every
  share — whereas two tokens are two physical objects in two places. That
  exemption is what keeps the settled design's group 2 (2-of-3 tokens, zero
  Tang — the cold-outage bootstrap) legal, and a test pins it so the rule cannot
  later be broadened into rejecting the design it protects.

  **This was not hypothetical.** `SssPolicy::fleet_three_group` emitted a bare
  `tang_group(peer_threshold)` as group 1 for any host without a TPM, and since
  the outer policy is a `t=1` OR, that one branch made the entire policy
  Tang-satisfiable. The hosts that took this arm are exactly the ones least able
  to afford it: the Raspberry Pi Tang servers, measured 2026-08-06 as having no
  TPM at all. Group 1 for a TPM-less host now ANDs the chassis-resident nano
  with the Tang group — the nano fills the structural role a TPM plays on a
  lenserv, being permanently seated and therefore present at boot with no human.

  The same defect sat in `verify.rs`'s own `settled_fleet_policy(None)` test
  fixture, so the emitter and the fixture agreed with each other and both were
  wrong. A new test runs the REAL emitter output through the REAL verifier for
  both TPM variants, which is the only assertion that cannot be satisfied by
  fixing one side alone.

- **The live fleet policy now verifies as vulnerable, deliberately.** The
  captured len-serv-003 binding is a bare `tang` `t=2`-of-3, and the test
  asserting it PASSED has been inverted to assert it fails. Two Tang keys and
  nothing else decrypt that volume today; the check must keep failing until the
  fleet is re-bound. The share count was never what was wrong — it read 2 before
  this change and still does.

  A previously-recorded known limitation (`todo.d/2026-08-05-verifier-has-no-factor-diversity-rule.md`)
  is closed by this. Two tests whose subject is the Tang **URL allowlist** were
  given topology-valid fixtures so they still fail for the reason they are named
  for, rather than tripping the new rule first and never reaching the URL check.
