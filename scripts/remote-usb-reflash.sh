#!/usr/bin/env bash
# file: scripts/remote-usb-reflash.sh
# version: 1.0.0
# guid: 7f3a9c1e-4b6d-4a2f-8e91-5c0d7a3b9f21
# last-edited: 2026-07-16
#
# Remotely reflash the USB stick a target is CURRENTLY live-booted from, with
# no physical access — run this FROM WITHIN an already-live SSH session on
# the target (e.g. `ssh ubuntu-server@172.16.2.35 'bash -s' < remote-usb-reflash.sh -- ...`
# or scp it over and run locally). Built for hosts like unimatrixone where
# pulling the stick, imaging it on a Mac, and reinserting it isn't an option
# (operator traveling — see project memory project_u1_ramdisk_reflash_plan).
#
# WHAT IT DOES (and why each step is ordered this way):
#   1. HEAD the target ISO to learn its size, then require free RAM >= 2x
#      that size before doing anything — refuses to guess, aborts loudly if
#      short. The download and the dd both read FROM the tmpfs copy, so the
#      live squashfs/network never has to survive mid-write.
#   2. Resolve the ACTUAL boot device dynamically from /cdrom's mount source
#      (never hardcoded /dev/sdX — this box also has an internal RAID array;
#      writing to the wrong device is unrecoverable). Refuses to proceed
#      unless the resolved device reports removable=1 in sysfs, unless
#      --force-device explicitly overrides that check.
#   3. tmpfs-stage the ISO: mount a RAM-backed scratch area sized to the
#      download, curl the ISO into it, and verify — a sha256 if --sha256 was
#      given, otherwise a strict downloaded-bytes == Content-Length check
#      (catches truncation; a corrupt image must never reach dd).
#   4. dd from the tmpfs copy onto the physical device, sync, then blockdev
#      --rereadpt so the kernel's partition table view matches what's now on
#      disk (best-effort, no-op if it fails).
#   5. Reports a final "reflash-done" status (reusing reporting.sh's
#      send_status_update, same pattern as uaa-usb-bootstrap.sh) and exits —
#      it does NOT power off the box itself. The live rootfs is still
#      nominally backed by the device just overwritten, so a local shutdown
#      sequence could hang trying to read now-mismatched metadata from
#      /cdrom or /rofs. Power off from the SERVER side over IPMI once this
#      script's final status lands, e.g.:
#        ssh 172.16.2.30 "ipmitool -I lanplus -H <bmc> -U ADMIN -P ADMIN chassis power off"
#
# Usage:
#   remote-usb-reflash.sh --iso-url <url> [--sha256 <hex>]
#                          [--report-base <url>] [--force-device <path>]
#
# Requires: bash, curl, lsblk, findmnt, dd, sha256sum (if --sha256 given).

set -uo pipefail

log() { echo "[remote-usb-reflash] $*" | tee /dev/kmsg 2>/dev/null || echo "[remote-usb-reflash] $*"; }
die() { log "FATAL: $*"; exit 1; }

ISO_URL=""
EXPECT_SHA256=""
REPORT_BASE="${UAA_REPORT_BASE:-http://172.16.2.30/cloud-init}"
FORCE_DEVICE=""
RAM_HEADROOM_FACTOR=2
STAGE_DIR=/mnt/uaa-ramstage

while [ $# -gt 0 ]; do
    case "$1" in
        --iso-url)      ISO_URL="${2:?--iso-url needs a value}"; shift 2 ;;
        --sha256)       EXPECT_SHA256="${2:?--sha256 needs a value}"; shift 2 ;;
        --report-base)  REPORT_BASE="${2:?--report-base needs a value}"; shift 2 ;;
        --force-device) FORCE_DEVICE="${2:?--force-device needs a value}"; shift 2 ;;
        -h|--help)      grep '^#' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *)              die "unknown argument: $1" ;;
    esac
done
[ -n "$ISO_URL" ] || die "--iso-url is required"

report_status() {
    local state="$1" msg="$2"
    curl -fsSL --max-time 10 "${REPORT_BASE}/reporting.sh" -o /run/reporting.sh 2>/dev/null \
        && bash -c "source /run/reporting.sh && send_status_update '${state}' 0 '${msg}'" 2>/dev/null \
        || log "could not reach reporting.sh / send status (best-effort, continuing)"
}

# ── 1. Resolve target device, dynamically ───────────────────────────────────
log "resolving the actual boot device from /cdrom's mount source"
BOOT_SRC="$(findmnt /cdrom -no SOURCE 2>/dev/null)"
[ -n "$BOOT_SRC" ] || die "/cdrom is not mounted — is this actually a live session? refusing to guess a device"

if [ -n "$FORCE_DEVICE" ]; then
    TARGET_DEV="$FORCE_DEVICE"
    log "WARNING: --force-device override in effect ($TARGET_DEV) — skipping the removable-device safety check"
else
    PARENT_NAME="$(lsblk -no pkname "$BOOT_SRC" 2>/dev/null | head -n1)"
    [ -n "$PARENT_NAME" ] || die "could not resolve a parent device for $BOOT_SRC via lsblk — refusing to guess (use --force-device to override)"
    TARGET_DEV="/dev/${PARENT_NAME}"
    [ -b "$TARGET_DEV" ] || die "resolved parent device $TARGET_DEV is not a block device"

    REMOVABLE_FLAG="/sys/block/${PARENT_NAME}/removable"
    REMOVABLE="$(cat "$REMOVABLE_FLAG" 2>/dev/null || echo "")"
    if [ "$REMOVABLE" != "1" ]; then
        die "refusing to write to $TARGET_DEV: sysfs reports removable='${REMOVABLE:-<missing>}' (expected 1)." \
            "This host also has an internal RAID array — writing to the wrong device is unrecoverable." \
            "If you are certain $TARGET_DEV is correct, re-run with --force-device $TARGET_DEV."
    fi
fi
log "target device resolved: $TARGET_DEV (boot source: $BOOT_SRC)"

# ── 2. Size the download, check RAM headroom BEFORE fetching anything ──────
log "checking ISO size via HEAD: $ISO_URL"
ISO_SIZE="$(curl -fsSL --max-time 15 -I "$ISO_URL" 2>/dev/null | tr -d '\r' | awk -F': ' 'tolower($1)=="content-length"{print $2}' | tail -n1)"
case "$ISO_SIZE" in
    ''|*[!0-9]*) die "could not determine ISO size from Content-Length header — refusing to guess a tmpfs size" ;;
esac
log "ISO reports ${ISO_SIZE} bytes"

FREE_KB="$(awk '/^MemAvailable:/{print $2}' /proc/meminfo)"
[ -n "$FREE_KB" ] || die "could not read /proc/meminfo MemAvailable"
FREE_BYTES=$((FREE_KB * 1024))
REQUIRED_BYTES=$((ISO_SIZE * RAM_HEADROOM_FACTOR))
if [ "$FREE_BYTES" -lt "$REQUIRED_BYTES" ]; then
    die "insufficient RAM: ${FREE_BYTES} bytes available, need >= ${REQUIRED_BYTES} (${RAM_HEADROOM_FACTOR}x the ${ISO_SIZE}-byte ISO). Aborting before touching anything."
fi
log "RAM check OK: ${FREE_BYTES} available >= ${REQUIRED_BYTES} required"

# tmpfs sized to the ISO plus one factor of headroom for the mount's own
# bookkeeping overhead — not the full RAM-headroom multiple, which is a
# safety margin for the *system*, not the tmpfs size itself.
TMPFS_MB=$(( (ISO_SIZE / 1024 / 1024) + 512 ))

# ── 3. Stage the ISO in RAM and verify before it ever touches the disk ─────
log "mounting ${TMPFS_MB}M tmpfs at $STAGE_DIR"
mkdir -p "$STAGE_DIR"
mount -t tmpfs -o "size=${TMPFS_MB}M" tmpfs "$STAGE_DIR" || die "tmpfs mount failed"
trap 'umount "$STAGE_DIR" 2>/dev/null || true' EXIT

STAGED_ISO="$STAGE_DIR/target.iso"
log "downloading ISO into tmpfs: $ISO_URL -> $STAGED_ISO"
if ! curl -fsSL --retry 3 --retry-delay 5 "$ISO_URL" -o "$STAGED_ISO"; then
    die "download failed — nothing written to $TARGET_DEV"
fi

DOWNLOADED_SIZE="$(stat -c%s "$STAGED_ISO" 2>/dev/null || echo 0)"
if [ "$DOWNLOADED_SIZE" != "$ISO_SIZE" ]; then
    die "downloaded size ($DOWNLOADED_SIZE) != expected Content-Length ($ISO_SIZE) — truncated/corrupt, refusing to dd"
fi
log "size check OK: downloaded $DOWNLOADED_SIZE bytes matches Content-Length"

if [ -n "$EXPECT_SHA256" ]; then
    ACTUAL_SHA256="$(sha256sum "$STAGED_ISO" | awk '{print $1}')"
    [ "$ACTUAL_SHA256" = "$EXPECT_SHA256" ] || die "sha256 mismatch: expected $EXPECT_SHA256, got $ACTUAL_SHA256 — refusing to dd"
    log "sha256 verified: $ACTUAL_SHA256"
else
    log "WARNING: no --sha256 given, only a downloaded-size check was performed"
fi

report_status running "remote-usb-reflash: staged+verified ISO, about to dd to ${TARGET_DEV}"

# ── 4. Write it. This is the irreversible step. ─────────────────────────────
log "dd'ing $STAGED_ISO -> $TARGET_DEV (reads from RAM; this does not depend on the network)"
if ! dd if="$STAGED_ISO" of="$TARGET_DEV" bs=4M status=progress conv=fsync; then
    report_status failed "remote-usb-reflash: dd to ${TARGET_DEV} failed"
    die "dd failed — $TARGET_DEV may be left in a partially-written state"
fi
sync
blockdev --rereadpt "$TARGET_DEV" 2>/dev/null || log "blockdev --rereadpt failed (best-effort, continuing)"

log "reflash complete. Do NOT reboot/shutdown this session locally — the live rootfs is still"
log "backed by the device just overwritten. Power off from the SERVER side over IPMI instead."
report_status success "remote-usb-reflash: ${TARGET_DEV} reflashed OK from ${ISO_URL} — awaiting IPMI power-off from server side"
