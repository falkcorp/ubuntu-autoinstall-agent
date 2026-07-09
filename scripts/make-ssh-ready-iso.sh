#!/usr/bin/env bash
# file: scripts/make-ssh-ready-iso.sh
# version: 1.0.0
# guid: 5d2b7f18-9c04-4a6e-8b31-2f7a0c9d6e42
# last-edited: 2026-07-09
#
# Re-master a stock Ubuntu Server ISO into an auto-SSH-ready installer USB.
#
# Injects the NoCloud cloud-init seed in installer-image/nocloud/ (user-data +
# meta-data) and adds `ds=nocloud;s=/cdrom/nocloud/` to the GRUB kernel cmdline,
# so the LIVE installer session boots with openssh-server on, user
# `ubuntu-server` (known password + operator key) and NOPASSWD sudo — no manual
# per-boot setup, and the `uaa install` agent can run every command as root over
# SSH without root login. It does NOT autoinstall (no `autoinstall:` key).
#
# Usage:
#   scripts/make-ssh-ready-iso.sh <input.iso> [output.iso]
#
# Then write the output to the USB, e.g.:
#   sudo dd if=<output.iso> of=/dev/sdX bs=4M status=progress conv=fsync
#
# Requires: xorriso (Linux: apt install xorriso; macOS: brew install xorriso).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# Seed location defaults next to the repo layout; override with UAA_SEED_DIR when
# running the script standalone (e.g. copied to /tmp on a target).
SEED_DIR="${UAA_SEED_DIR:-${SCRIPT_DIR}/../installer-image/nocloud}"

IN_ISO="${1:?usage: make-ssh-ready-iso.sh <input.iso> [output.iso]}"
OUT_ISO="${2:-${IN_ISO%.iso}-ssh-ready.iso}"

command -v xorriso >/dev/null 2>&1 || { echo "ERROR: xorriso not found (apt install xorriso / brew install xorriso)"; exit 1; }
[ -f "$SEED_DIR/user-data" ] && [ -f "$SEED_DIR/meta-data" ] || { echo "ERROR: seed missing in $SEED_DIR"; exit 1; }

# Input may be a regular .iso file OR a block device (e.g. the boot USB itself,
# /dev/sdc). xorriso addresses non-MMC devices with a "stdio:" prefix.
if [[ "$IN_ISO" == stdio:* ]]; then
  IN_DEV="$IN_ISO"
elif [ -b "$IN_ISO" ]; then
  IN_DEV="stdio:$IN_ISO"
elif [ -f "$IN_ISO" ]; then
  IN_DEV="$IN_ISO"
else
  echo "ERROR: input not found (need an .iso file or block device): $IN_ISO"; exit 1
fi
case "$OUT_ISO" in
  stdio:*|/dev/*) echo "ERROR: refusing to write output to a device ($OUT_ISO); give a file path"; exit 1 ;;
esac

WD="$(mktemp -d)"
trap 'rm -rf "$WD"' EXIT

echo "== extracting GRUB config from $IN_ISO =="
# Pull the boot configs we need to patch (main + EFI loopback if present).
xorriso -osirrox on -indev "$IN_DEV" \
  -extract /boot/grub/grub.cfg "$WD/grub.cfg" 2>/dev/null || { echo "ERROR: no /boot/grub/grub.cfg in ISO"; exit 1; }
HAVE_LOOPBACK=0
if xorriso -osirrox on -indev "$IN_DEV" -extract /boot/grub/loopback.cfg "$WD/loopback.cfg" 2>/dev/null; then
  HAVE_LOOPBACK=1
fi

# Add the cloud-init NoCloud datasource to every kernel line that boots casper.
# The semicolon must be escaped for GRUB. Idempotent (skip if already present).
patch_cfg() {
  local f="$1"
  if grep -q "ds=nocloud" "$f"; then echo "  ($f already patched)"; return; fi
  # Insert right after the vmlinuz path on each linux/linuxefi line.
  sed -i -E 's#(linux(efi)?[[:space:]]+/casper/vmlinuz)#\1 ds=nocloud\\;s=/cdrom/nocloud/ autoinstall=0#' "$f"
  echo "  patched $f"
}
echo "== patching kernel cmdline =="
patch_cfg "$WD/grub.cfg"
[ "$HAVE_LOOPBACK" = 1 ] && patch_cfg "$WD/loopback.cfg"

echo "== repacking -> $OUT_ISO (preserving El Torito boot) =="
MAPS=( -map "$WD/grub.cfg" /boot/grub/grub.cfg -map "$SEED_DIR" /nocloud )
[ "$HAVE_LOOPBACK" = 1 ] && MAPS+=( -map "$WD/loopback.cfg" /boot/grub/loopback.cfg )

xorriso -indev "$IN_DEV" -outdev "$OUT_ISO" \
  -boot_image any replay \
  -compliance no_emul_toc \
  "${MAPS[@]}"

echo
echo "DONE: $OUT_ISO"
echo "Write it to the USB, e.g.:  sudo dd if='$OUT_ISO' of=/dev/sdX bs=4M status=progress conv=fsync"
