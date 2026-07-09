#!/bin/bash
# file: scripts/build-installer-image.sh
# version: 1.0.0
# guid: a2f47c9b-8d13-4e6a-b075-1c9e2f83a6d0
# last-edited: 2026-07-09
#
# Build the custom ZFS-on-LUKS installer image (Option 2) by OVERLAYING the
# Ubuntu 26.04 live-server squashfs with the static agent + boot automation.
#
# Reuses Canonical's signed casper kernel/initrd unchanged (Secure Boot friendly)
# and only rewrites the root squashfs. iPXE then boots:
#
#   kernel  <casper/vmlinuz>
#   initrd  <casper/initrd>
#   cmdline boot=casper netboot=url url=<...> uaa.autoinstall uaa.config=<host-yaml> ip=dhcp
#
# Requires: root, squashfs-tools (unsquashfs/mksquashfs), rsync.
#
# Usage:
#   sudo ./build-installer-image.sh \
#     --src-squashfs /var/www/html/ubuntu/casper/ubuntu-server-minimal.squashfs \
#     --agent        ./target/x86_64-unknown-linux-musl/release/ubuntu-autoinstall-agent \
#     --out          /var/www/html/ubuntu/casper/uaa-installer.squashfs
#
# NOTE: two steps are marked VERIFY-ON-VM — the exact name of the stock installer
# autostart unit on the 26.04 live-server image, and that debootstrap is present in
# the live rootfs (apt-add it into the overlay if not). Confirm both during the
# QEMU+swtpm validation before trusting this on hardware.

set -euo pipefail

SRC_SQUASHFS=""
AGENT_BIN=""
OUT_SQUASHFS=""
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
IMG_DIR="$(cd "${HERE}/../installer-image" && pwd)"

die() { echo "ERROR: $*" >&2; exit 1; }

while [ $# -gt 0 ]; do
    case "$1" in
        --src-squashfs) SRC_SQUASHFS="$2"; shift 2 ;;
        --agent)        AGENT_BIN="$2"; shift 2 ;;
        --out)          OUT_SQUASHFS="$2"; shift 2 ;;
        *) die "unknown arg: $1" ;;
    esac
done

[ -n "${SRC_SQUASHFS}" ] && [ -f "${SRC_SQUASHFS}" ] || die "--src-squashfs missing/not found"
[ -n "${AGENT_BIN}" ]    && [ -f "${AGENT_BIN}" ]    || die "--agent missing/not found"
[ -n "${OUT_SQUASHFS}" ]                             || die "--out required"
[ "$(id -u)" -eq 0 ] || die "must run as root (unsquashfs/mksquashfs + chroot bits)"
command -v unsquashfs >/dev/null || die "install squashfs-tools"

WORK="$(mktemp -d)"
ROOT="${WORK}/squashfs-root"
trap 'rm -rf "${WORK}"' EXIT

echo "==> Unpacking ${SRC_SQUASHFS}"
unsquashfs -d "${ROOT}" "${SRC_SQUASHFS}" >/dev/null

echo "==> Injecting agent + boot automation"
install -m 0755 "${AGENT_BIN}"              "${ROOT}/usr/local/bin/uaa"
install -m 0755 "${IMG_DIR}/uaa-autoinstall.sh"      "${ROOT}/usr/local/bin/uaa-autoinstall.sh"
install -m 0644 "${IMG_DIR}/uaa-autoinstall.service" "${ROOT}/etc/systemd/system/uaa-autoinstall.service"

echo "==> Enabling uaa-autoinstall.service (multi-user.target.wants)"
mkdir -p "${ROOT}/etc/systemd/system/multi-user.target.wants"
ln -sf ../uaa-autoinstall.service \
    "${ROOT}/etc/systemd/system/multi-user.target.wants/uaa-autoinstall.service"

# VERIFY-ON-VM: mask whatever autostarts the stock installer on 26.04 live-server.
# On recent server ISOs this is subiquity-server.service (snap-wrapped variants exist).
# Masking is a no-op if the unit is absent, so mask the likely candidates.
echo "==> Masking stock installer autostart (VERIFY unit name on VM)"
for unit in subiquity-server.service serial-subiquity@.service \
            snap.subiquity.subiquity-server.service; do
    ln -sf /dev/null "${ROOT}/etc/systemd/system/${unit}" || true
done

# VERIFY-ON-VM: the agent needs debootstrap + gdisk in the LIVE rootfs (casper has
# cryptsetup + zfs already). If absent, they must be baked into the overlay. We
# can't apt-install offline here reliably, so flag it loudly rather than silently
# shipping a broken image.
echo "==> Checking live-rootfs install tools"
for tool in debootstrap sgdisk zpool cryptsetup dracut clevis; do
    if [ ! -e "${ROOT}/usr/sbin/${tool}" ] && [ ! -e "${ROOT}/sbin/${tool}" ] \
       && [ ! -e "${ROOT}/usr/bin/${tool}" ]; then
        echo "  WARN: '${tool}' not found in live rootfs — bake it into the overlay" >&2
    fi
done

echo "==> Repacking squashfs -> ${OUT_SQUASHFS}"
rm -f "${OUT_SQUASHFS}"
mksquashfs "${ROOT}" "${OUT_SQUASHFS}" -comp zstd -no-progress >/dev/null
echo "==> Done: ${OUT_SQUASHFS} ($(du -h "${OUT_SQUASHFS}" | cut -f1))"
echo "    Point iPXE at this squashfs and add: uaa.autoinstall uaa.config=<host-yaml-url>"
