#!/bin/sh
# file: dracut/91uaa-keystore-wait/keystore-wait.sh
# version: 1.0.0
# guid: 3cf623e7-f792-4410-9cca-6d54b9f3732d
# last-edited: 2026-07-22
#
# pre-mount hook (priority 89): block until the keystore zvol device node
# exists, so the stock clevis unlock + `zfs load-key` (whose keylocation is a
# file inside that zvol's LUKS) do not race udev. See design §5 / D7.1.
#
# rpool is imported by the zfs module in an earlier hook; the zvol node then
# appears asynchronously via udev. Without this wait the load-key can fire first
# → intermittent emergency shell (the worst failure class).

type warn >/dev/null 2>&1 || warn() { echo "keystore-wait: $*" >&2; }
type info >/dev/null 2>&1 || info() { echo "keystore-wait: $*" >&2; }

KEYSTORE_ZVOL="/dev/zvol/rpool/keystore"

# Already there? Nothing to do.
[ -e "$KEYSTORE_ZVOL" ] && exit 0

info "waiting for keystore zvol $KEYSTORE_ZVOL"
i=0
while [ ! -e "$KEYSTORE_ZVOL" ]; do
    # Nudge udev to process any pending zvol events, then re-check.
    udevadm settle --timeout=2 >/dev/null 2>&1 || true
    [ -e "$KEYSTORE_ZVOL" ] && break
    i=$((i + 1))
    if [ "$i" -ge 60 ]; then
        warn "$KEYSTORE_ZVOL did not appear after ~60s; continuing (boot may drop to emergency shell)"
        exit 0
    fi
    sleep 1
done
info "keystore zvol present after ${i}s"
exit 0
