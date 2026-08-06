<!-- file: changelog.d/fix-verify-clevis-nested-tree.md -->
<!-- version: 1.0.0 -->
<!-- guid: 4b7e2f10-9c85-4a3d-b6e1-72d0c4a9f358 -->
<!-- last-edited: 2026-08-03 -->

### Fixed

- **Post-install verification no longer rejects the settled unlock policy.**
  The `clevis_binding` check hardcoded an outer threshold of `t=2`, but the
  policy the fleet settled on has an outer `t=1` — an OR over three groups, each
  of which is itself a real AND (both Tang peers; any 2 of 3 tokens; any one Tang
  AND either carried key). The verifier therefore refused the exact design it
  exists to protect, reporting `threshold is t=1, fleet policy requires t=2`. It
  now checks the property that actually matters — **no single share can open the
  volume, on any path through the tree** — by recursing the policy and computing
  the cheapest satisfying set: a leaf pin costs one share, a pin array costs one
  share per element, a nested `sss` group costs its own cheapest set, and a node
  costs the sum of its `t` cheapest children. An outer `t=1` passes only when
  *every* branch independently requires two or more shares. The flattening bug
  is still rejected on its own terms: a multi-element pin array sitting directly
  beside another pin is `t`-of-many rather than the AND its author intended, so
  `{"t":2,"pins":{"tang":[a,b,c],"tpm2":{…}}}` remains a failure wherever it
  appears in the tree.

- **Tang URLs are verified as a subset of the fleet inventory, not as a
  per-host requirement.** The check demanded that all three fleet Tang servers
  appear in the binding, which the settled two-peer-per-host policy cannot
  satisfy. A host may now bind any subset, but every Tang server it binds must be
  one the fleet runs — an unknown key server, or a policy with no Tang server at
  all, fails.
