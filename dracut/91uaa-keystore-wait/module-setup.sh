#!/bin/bash
# file: dracut/91uaa-keystore-wait/module-setup.sh
# version: 2.0.0
# guid: 3a3d7611-6c6d-4ddf-b3fa-79834c3febad
# last-edited: 2026-07-26
#
# Dracut module closing the two D7 boot-ordering races (design
# docs/specs/u1-zfs-native-encryption-design.md §5) via a single pre-mount 89
# hook that runs before zfs-dracut's own keystore unlock at pre-mount 90:
#
#   D7.1 keystore-zvol race — Ubuntu's zfs-dracut port dropped the loop that
#        waits for /dev/zvol/*; udev creates the node asynchronously after
#        `zpool import`, so the unlock can fire before the node exists ->
#        emergency shell. The hook reinstates the wait.
#   D7.2 network-online race — the keystore is opened directly by zfs-dracut
#        (systemd-cryptsetup attach), and clevis-luks-askpass has no
#        network-online ordering on Ubuntu, so the Tang unlock can fire before
#        DHCP has leased -> Tang unreachable -> emergency shell. The hook also
#        waits for the network to be up (when rd.neednet is set) before it
#        returns, so the unlock at pre-mount 90 always runs with the net up.
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
    # udevadm+sleep: D7.1 zvol wait. ip: D7.2 network readiness check
    # (systemctl is already present in the systemd-based initramfs).
    inst_multiple udevadm sleep ip
    inst_hook pre-mount 89 "$moddir/keystore-wait.sh"
}
