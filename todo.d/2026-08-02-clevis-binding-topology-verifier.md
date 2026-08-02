<!-- file: todo.d/2026-08-02-clevis-binding-topology-verifier.md -->
<!-- version: 1.0.0 -->
<!-- guid: d10ed669-e593-4ce6-a1d6-053f06d85346 -->
<!-- last-edited: 2026-08-02 -->

- [ ] **clevis-topology** Make `evaluate_clevis_binding`
      (`crates/uaa-core/src/autoinstall/verify.rs:257`) assert the share
      **topology**, not substrings. It currently passes when
      `contains("sss") && contains("\"t\":2")` and every Tang URL appears
      anywhere in the `clevis luks list` output. The BROKEN flat policy
      `{"t":2,"pins":{"tang":[a,b,c],"tpm2":{}}}` satisfies all three — it is
      2-of-**four** shares, so Tang alone opens it with no TPM — and so does the
      correct nested AND
      `{"t":2,"pins":{"tpm2":{…},"sss":[{"t":2,"pins":{"tang":[…]}}]}}`. The
      verifier cannot tell them apart. Worse, the `contains("sss")` check is
      satisfied by the literal pin NAME in the line prefix (`1: sss '…'`), so it
      is true for any sss-bound volume regardless of nesting. Fix: parse the pin
      JSON and reject any binding where `tang` is a direct child of the outer
      `pins` object. `scripts/vm-gate/pkcs11-clevis-gate.sh` already asserts
      exactly this (`pos-10`/`neg-05`) and demonstrates the gap (`pos-11`);
      port that predicate into `verify.rs` and add the flat config as a failing
      unit-test fixture next to the existing `clevis_binding_*` tests.
