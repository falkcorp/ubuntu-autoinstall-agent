<!-- file: changelog.d/fix-rpool-missing-cachefile.md -->
<!-- version: 1.0.0 -->
<!-- guid: 8e2d4a17-6c93-4f05-b1a8-9d3f7c206e41 -->
<!-- last-edited: 2026-07-27 -->

### Fixed

- **NativeKeystore installs are now bootable — rpool is written to
  `/etc/zfs/zpool.cache`.** `create_rpool` omitted `cachefile=/etc/zfs/zpool.cache`
  (bpool had it). Because rpool is created with `-R /mnt/targetos` (altroot, which
  defaults `cachefile=none`), rpool was never recorded in the pool cache. That
  cache is copied into the initramfs, and at boot `zfs-import-cache` imports only
  the pools it lists — so **rpool never imported**, the keystore zvol (which lives
  on rpool) never appeared, no ZFS key was ever loaded, and `sysroot.mount` failed
  with "failed to mount sysroot". Every keystore/clevis/hostid/network fix was
  moot until this: the pool holding the encrypted root simply wasn't there. Adding
  the cachefile makes rpool import in the initramfs like bpool does.
