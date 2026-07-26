#!/bin/sh
# file: dracut/91uaa-keystore-wait/keystore-wait.sh
# version: 3.0.0
# guid: 3cf623e7-f792-4410-9cca-6d54b9f3732d
# last-edited: 2026-07-26
#
# pre-mount hook (priority 89). Closes TWO boot-ordering races that both drop the
# boot to the dracut emergency shell, and both run BEFORE zfs-dracut's own
# keystore unlock at pre-mount 90 (`90zfs/zfs-load-key.sh` ->
# `systemd-cryptsetup attach keystore-rpool` -> clevis/Tang). dracut runs
# pre-mount hooks sequentially by ascending priority, so 89 fully completes
# before 90 starts — that is what makes this the reliable gate.
#
#   D7.1  keystore zvol node race: udev creates /dev/zvol/* asynchronously after
#         `zpool import`, so the unlock can fire before the node exists. Wait for
#         it.
#   D7.2  network-online race: the keystore is opened directly by zfs-dracut
#         (NOT via the _netdev crypttab unit — Ubuntu's
#         4001-dracut-Open-and-mount-luks-keystore.patch), and clevis-luks-askpass
#         has NO network-online ordering on Ubuntu (clevis auto-wiring needs a
#         non-empty hostonly_cmdline, which Ubuntu leaves empty). So the SSS
#         t=2 unlock can query Tang before DHCP has leased -> every Tang share
#         fails -> t=2 unmet -> `zfs load-key` never runs -> sysroot.mount fails.
#         Gate on the network being up here, before the unlock at 90.
#
# `_netdev` on /etc/crypttab does NOT cover D7.2 because the keystore is not
# unlocked through the crypttab-generated systemd-cryptsetup unit at all.

type warn >/dev/null 2>&1 || warn() { echo "keystore-wait: $*" >&2; }
type info >/dev/null 2>&1 || info() { echo "keystore-wait: $*" >&2; }

KEYSTORE_ZVOL="/dev/zvol/rpool/keystore"

# ---- D7.1: wait for the keystore zvol device node --------------------------
if [ -e "$KEYSTORE_ZVOL" ]; then
    info "keystore zvol $KEYSTORE_ZVOL already present"
else
    info "waiting for keystore zvol $KEYSTORE_ZVOL"
    i=0
    while [ ! -e "$KEYSTORE_ZVOL" ]; do
        # Nudge udev to process any pending zvol events, then re-check.
        udevadm settle --timeout=2 >/dev/null 2>&1 || true
        [ -e "$KEYSTORE_ZVOL" ] && break
        i=$((i + 1))
        if [ "$i" -ge 60 ]; then
            warn "$KEYSTORE_ZVOL did not appear after ~60s; continuing (boot may drop to emergency shell)"
            break
        fi
        sleep 1
    done
    [ -e "$KEYSTORE_ZVOL" ] && info "keystore zvol present after ${i}s"
fi

# ---- D7.2: wait for the network before the Tang unlock ---------------------
# Only when this host was configured for network unlock. `rd.neednet=1` is set by
# the installer exactly when Tang servers are configured, so it is the precise
# signal that a network-backed unlock is about to happen at pre-mount 90.
if grep -qw rd.neednet /proc/cmdline 2>/dev/null; then
    # Actively pull in network-online.target (no-block: networkd keeps working
    # while we poll below; avoids any hook-vs-target ordering deadlock).
    systemctl start --no-block network-online.target >/dev/null 2>&1 || true

    info "waiting for network before keystore Tang unlock"
    i=0
    while :; do
        # Online per systemd, OR a global-scope IPv4 is configured (Tang lives on
        # the same L2 as this host, so a global IPv4 is sufficient to reach it).
        if systemctl is-active --quiet network-online.target 2>/dev/null; then
            info "network-online.target active after ${i}s"
            break
        fi
        if ip -4 -o addr show scope global 2>/dev/null | grep -q 'inet '; then
            info "global IPv4 present after ${i}s"
            break
        fi
        i=$((i + 1))
        if [ "$i" -ge 120 ]; then
            warn "network not up after ~120s; continuing (Tang unlock may fail -> emergency shell)"
            break
        fi
        sleep 1
    done
fi

# ---- D7.3: perform the keystore unlock DIRECTLY (the proven manual path) ----
# zfs-dracut's stock keystore open at pre-mount 90 (zfs-load-key.sh ->
# `systemd-cryptsetup attach` + clevis-luks-askpass answering an ask-password
# request) does NOT fire reliably in this initramfs — the boot hangs silently
# waiting for a password nobody answers. Since this hook runs at pre-mount 89
# (dracut runs pre-mount hooks sequentially, so 89 fully completes before 90),
# do the exact sequence that works by hand: clevis-unlock the keystore LUKS,
# mount it at the keylocation path, and `zfs load-key`. Stock zfs-load-key.sh
# then hits its own guard (`[ keystatus = unavailable ] || return 0`), sees the
# key already loaded, and no-ops the fragile askpass path entirely.
#
# Mapper name MUST match zfs-load-key.sh's ("keystore-<pool>") and the mount
# path MUST be the keylocation dir (/run/keystore/<pool>) so `zfs load-key -a`
# finds system.key.
POOL="rpool"
KS="/dev/zvol/$POOL/keystore"
MAPPER="keystore-$POOL"
KEYDIR="/run/keystore/$POOL"
if [ -e "$KS" ]; then
    if [ ! -e "/dev/mapper/$MAPPER" ]; then
        info "clevis-unlocking keystore $KS -> $MAPPER"
        clevis luks unlock -d "$KS" -n "$MAPPER" 2>&1 | while IFS= read -r l; do info "clevis: $l"; done
    fi
    if [ -e "/dev/mapper/$MAPPER" ]; then
        mkdir -p "$KEYDIR"
        if ! mountpoint -q "$KEYDIR" 2>/dev/null; then
            mount "/dev/mapper/$MAPPER" "$KEYDIR" 2>&1 | while IFS= read -r l; do info "mount: $l"; done
        fi
        info "loading ZFS keys from keystore"
        if zfs load-key -a >/dev/null 2>&1; then
            info "ZFS keys loaded from keystore"
        else
            warn "zfs load-key -a failed; stock pre-mount 90 will attempt its own path"
        fi
    else
        warn "keystore mapper $MAPPER did not open; falling through to stock pre-mount 90"
    fi
fi

exit 0
