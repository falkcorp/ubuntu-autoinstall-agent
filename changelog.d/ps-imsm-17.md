### Removed

#### Drop `/dev/md` IMSM/mdraid sniffing from the SSH installer

Per the settled storage architecture (IMSM dropped in favor of native ZFS),
removed every `disk_device.starts_with("/dev/md")` branch from the SSH
installer: `DiskManager::assemble_md_if_needed` (and its two call sites in
`prepare_disk` and `mount_existing_target`), the conditional `mdadm` target
package in `SystemConfigurator`, and the `mdraid` dracut module +
`/etc/mdadm/mdadm.conf` write in `configure_dracut_crypt_modules`.
`SystemConfigurator::build_dracut_crypt_conf` drops its `include_mdraid`
parameter entirely rather than always passing `false`. Tests that asserted
md-specific behavior are deleted or repointed at plain nvme paths; the
unconditional `mdadm` apt package in `packages.rs` (harmless on non-md hosts)
is left in place as out of scope. No committed host relies on `/dev/md` —
len-serv hosts use plain `nvme0n1` and unimatrixone moves to native ZFS — so
the len-serv `PlainLuks` path is functionally unaffected (its generated
dracut module list is unchanged; only an inert `mdraid`-related comment line
is no longer emitted into `90-uaa-crypt.conf`).
