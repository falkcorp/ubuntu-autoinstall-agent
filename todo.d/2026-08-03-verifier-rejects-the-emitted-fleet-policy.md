<!-- file: todo.d/2026-08-03-verifier-rejects-the-emitted-fleet-policy.md -->
<!-- version: 1.0.0 -->
<!-- guid: 3d9b5f21-8c47-4e0a-9f36-1b7ad2c40e85 -->
<!-- last-edited: 2026-08-03 -->

- [ ] **Post-install verifier rejects the fleet policy the installer emits.**
      `validate_sss_policy` (`crates/uaa-core/src/autoinstall/verify.rs`)
      hardcodes `t == CLEVIS_THRESHOLD` (2), but
      `SssPolicy::fleet_three_group` emits an outer **t=1** OR over three
      groups — so `evaluate_clevis_binding` fails every correctly-built host
      with `threshold is t=1, fleet policy requires t=2`. The staleness was
      invisible while the verifier parsed `clevis luks list` (it was fed a
      misrendering and was wrong either way); reading the JWE exposed it.
      Fixing it means teaching the validator about **grouped OR** policies:
      an outer `t=1` over N groups is only as strong as its weakest group, so
      each branch must be independently sound (no single-Tang group, no group
      satisfiable by chassis-resident factors alone). Marked by
      `known_gap_verifier_rejects_the_emitted_fleet_policy`, which fails the
      day the gap closes.
