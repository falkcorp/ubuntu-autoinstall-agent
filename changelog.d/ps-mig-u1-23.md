<!-- file: changelog.d/ps-mig-u1-23.md -->
<!-- version: 1.0.0 -->
<!-- guid: 6a9c1e4f-2b7d-4a08-9e3c-1f5a8d2b6c91 -->
<!-- last-edited: 2026-07-24 -->

### Changed

#### Migrate unimatrixone (U1) to component authoring, establishing the reference pattern (PS-MIG-U1-23)

`examples/configs/install/unimatrixone.yaml` bumps to `v4.0.0`; its header
now documents `crates/uaa-core/tests/fixtures/components/unimatrixone.yaml`
(created by PS-GATE-15) as the canonical component-authored source — a
`HostGroupProfile` defaults + single-host `HostProfile` overrides pair,
struct-equal by `merge()`+`lower()` to this committed file. The flat file
body is unchanged (header-only edit): the installer and
`scripts/vm-validate.sh` still consume it directly as the wire
`InstallationConfig`. unimatrixone is a standalone single-host group, so this
establishes the "group defaults + single host override, with an
authoring-only nested `unlock_policy.tpm2_clevis_peer` leaf" shape the
PS-MIG-LEN-* briefs reuse for the indexed len-serv group.

Adds the three test-coverage gaps this migration's acceptance bar calls for,
none previously exercised against the real unimatrixone fixture:
`crates/uaa-core/tests/component_equality_gate.rs` now also asserts
`validate_resolved` accepts unimatrixone's merge output;
`crates/uaa-core/tests/placeholder_survival.rs` adds unimatrixone-specific
`REPLACE_AT_PLACE_TIME` survival checks for `luks_key` (the keystore-LUKS
recovery passphrase), `root_password`, and `install_ca_cert`, plus a
negative check that `tpm2_pin` — a field unimatrixone's NativeKeystore
unlock policy never authors — stays `None` rather than picking up a stray
value; and `crates/uaa-control/src/profiles/reify.rs` adds a test proving a
reified unimatrixone group/profile row is stamped `schema_version == 1`
(`PS-SCHEMA-20`'s `SCHEMA_VERSION_MAX`).

The D2-B VM gate (`scripts/vm-validate.sh` against a real amd64 KVM host) is
deferred to an operator run on the server — see the PR body.
