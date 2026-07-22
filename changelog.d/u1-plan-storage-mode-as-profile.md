### Changed

#### U1 plan: model storage_mode as a profile attribute (machine templates)

The U1 native-encryption plan now specifies `storage_mode` (and the new
`disks`/`tpm2_sss_peer`/Tang-`thp` fields) as **profile-level** attributes in
the deploy-system registry rather than per-host YAML keys — a Lenovo
HostGroupProfile default (`PlainLuks`) the `len-serv-*` hosts inherit, overridden
on the standalone `unimatrixone` profile (`NativeKeystore`). Records that both
profiles already exist from the 2026-07-21 backfill, so `len-serv-003` needs
none of the migration and deploys via the existing plain-LUKS path today.
