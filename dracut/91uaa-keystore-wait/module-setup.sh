#!/bin/bash
# file: dracut/91uaa-keystore-wait/module-setup.sh
# version: 1.0.0
# guid: 3a3d7611-6c6d-4ddf-b3fa-79834c3febad
# last-edited: 2026-07-22
#
# Dracut module closing the D7.1 keystore-zvol race (design
# docs/specs/u1-zfs-native-encryption-design.md §5). Ubuntu's zfs-dracut port
# dropped the loop that waits for /dev/zvol/* — udev creates the node
# asynchronously after `zpool import`, so the stock `zfs load-key` (whose
# keylocation lives on the rpool/keystore zvol) can fire before the node exists
# and drop the boot to an emergency shell. This module reinstates the wait as a
# pre-mount hook that runs before clevis-luks-askpass unlocks the keystore.
#
# Installed only on NativeKeystore hosts (the installer copies this dir into the
# target's /usr/lib/dracut/modules.d/ and adds it to dracut.conf.d).

# dracut hook: decide whether to include this module.
check() {
    # Native-keystore hosts have the zfs userspace; skip elsewhere.
    require_binaries zfs zpool || return 1
    return 0
}

# dracut hook: module dependencies.
depends() {
    echo zfs
    return 0
}

# dracut hook: install the hook script + the binaries it needs into the initramfs.
install() {
    inst_multiple udevadm sleep
    inst_hook pre-mount 89 "$moddir/keystore-wait.sh"
}
