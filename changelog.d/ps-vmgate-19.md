### Added

#### VM-gate readiness probe is now application-kind driven (PS-VMGATE-19)

`scripts/vm-validate.sh` stage 6's application readiness check (previously a
hardcoded CockroachDB `SELECT 1` probe) now dispatches on the profile's
`applications[0].kind`, read straight out of the `--config` YAML: `cockroach`
keeps the existing SQL readiness probe unchanged; `tang-server` runs a
`curl -sf --max-time 5 <url>/adv` check (same shape as
`crates/uaa-core/src/luks_keys.rs`'s Tang probe); an empty/absent
`applications:` list asserts `multi-user.target` reached only, which stages
above already prove. Added `examples/configs/install/vm-test-app-free.yaml`
as the fixture exercising that empty-applications branch. Stages 0-5 and the
LUKS/ZFS assertions are unchanged; a code comment flags that those
assertions are storage-mode-specific and will need role/storage-mode gating
in a future brief.
