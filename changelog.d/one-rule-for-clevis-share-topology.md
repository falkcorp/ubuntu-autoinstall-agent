<!-- file: changelog.d/one-rule-for-clevis-share-topology.md -->
<!-- version: 1.0.0 -->
<!-- guid: 6c31e8a7-5b04-4d92-8f16-a07d3e91b524 -->
<!-- last-edited: 2026-08-05 -->

### Added

- **`uaa verify-policy`** — judges a clevis unlock policy's share topology
  locally, with no SSH. Takes `--device` (runs the LUKS2 metadata dump itself),
  `--file`, or stdin, and exits non-zero when the policy is satisfiable by a
  single share. It runs the *same* evaluator `uaa verify` runs over SSH, so a
  gate or a runbook can ask "is this binding safe?" and get the shipping answer
  rather than a second opinion.

### Fixed

- **The VM gate and the installer no longer disagree about what a safe policy
  is.** `pkcs11-clevis-gate.sh` decided share topology with its own `jq`
  predicate over `clevis luks list` output. Both halves were wrong:

  - **Wrong data.** `clevis luks list` misrenders a nested policy — it drops
    shares and collapses arrays into bare objects. A topology judgement made
    from it describes a tree that does not exist.
  - **Wrong rule.** "`tang` must not be a direct child of the outer `pins`" is
    much weaker than the property the fleet needs. It blesses, among other
    things, an outer `t=1` OR one of whose branches a single share satisfies —
    no top-level `tang` anywhere, and one factor opens the volume.

  Two implementations of one security rule drift apart, and these had: the gate
  was still asserting against a `verify.rs` that no longer exists. The rule now
  lives only in `evaluate_clevis_binding`, and the gate calls it. The gate also
  fails closed and early if the binary is missing, rather than silently falling
  back to a rule only it believes.

  The retired `pos-11` assertion (`substrings-pass-on-BROKEN-flat`) existed to
  demonstrate that `verify.rs`'s three substring predicates passed on the flat
  2-of-4 config. Those predicates are gone, so the demonstration would have to
  reimplement a deleted implementation to keep asserting anything — and its
  subject is precisely what `neg-05` now proves is fixed.
