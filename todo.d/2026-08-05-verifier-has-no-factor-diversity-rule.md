<!-- file: todo.d/2026-08-05-verifier-has-no-factor-diversity-rule.md -->
<!-- version: 1.0.0 -->
<!-- guid: 1e7b4d0a-92c3-4f58-a71e-5d6084b2c9f3 -->
<!-- last-edited: 2026-08-05 -->

- [ ] **factor-diversity** The verifier enforces *no single share opens the
      volume* (`min_satisfying_shares >= 2`). The fleet requirement is stronger
      and is about the KIND of factor: **Tang alone must not open a lenserv
      disk.** The two come apart at the outer `t=1` OR. Measured 2026-08-05 with
      `known_limitation_tang_only_group_passes_the_share_count` in
      `crates/uaa-core/src/autoinstall/verify.rs`: a policy whose group 1 is
      `{"t":2,"pins":{"tang":[a,b,c]}}` scores `min_satisfying_shares = Ok(2)`
      and the check returns `passed = true` — yet the cheapest satisfying set is
      **two Tang servers**, and the outer `t=1` means that group alone unlocks
      the host. That is the original flat-`sss` vulnerability re-entering in a
      shape share arithmetic cannot see, because it counts factors without any
      notion of their independence.
      Deciding the rule is a security-policy call, not a bug fix, so nothing was
      changed. Candidate shape: no branch of an outer OR may be satisfiable by a
      single pin KIND alone — which would reject a Tang-only group 1 while still
      accepting the legacy flat Tang-only policy on len-serv-001/002 (that host
      class has no second factor at all and is Tang-only by design, so the rule
      needs a per-host-class escape or it fails the fleet it is protecting).
      When it lands, the test above starts FAILING by design: update it and
      delete this fragment.
- [ ] **tpm2-ordering** Consequence of the above for len-serv-003: dropping the
      `tpm2` pin is NOT a safe way to dodge the PCR7 problem. Under the settled
      policy `tpm2` is a REQUIRED factor in group 1 (`t=2` over exactly
      `{tpm2, tang-group}`), so a PCR7 change kills the only hands-off unlock
      path — groups 2 and 3 both need a physical token, i.e. every reboot needs
      a human with a YubiKey on a host with no remote power. But removing
      `tpm2` makes group 1 Tang-only, which is the vulnerability above. Both
      exits are bad, so the ordering constraint is real: **bind `tpm2` only
      after Secure Boot is settled in User Mode and the firmware upgrade is
      done.** See `docs/status/2026-08-03-len-serv-003-e2e-readiness.md` §
      "Requirements we have of the hardware track", item 1.
