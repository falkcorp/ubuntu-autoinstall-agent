#!/bin/bash
# file: scripts/create-autoinstall-iso.sh
# version: 1.1.0
# guid: x9y8z7w6-v5u4-3210-9876-543210xyzwvu
# last-edited: 2026-07-08

# Script to create a modified Ubuntu ISO with autoinstall enabled

set -euo pipefail

UBUNTU_ISO="$1"
OUTPUT_ISO="$2"
CLOUD_INIT_DIR="$3"

echo "🔧 Creating autoinstall-enabled Ubuntu ISO..."

# Create temporary working directory
WORK_DIR=$(mktemp -d)
trap "rm -rf $WORK_DIR" EXIT

# Extract the original ISO
echo "📦 Extracting original ISO..."
xorriso -osirrox on -indev "$UBUNTU_ISO" -extract / "$WORK_DIR/iso"

# Modify GRUB configuration to add autoinstall kernel parameters
echo "✏️  Modifying GRUB configuration..."
GRUB_CFG="$WORK_DIR/iso/boot/grub/grub.cfg"

if [[ -f "$GRUB_CFG" ]]; then
    # Add autoinstall parameters to the default Ubuntu Server option
    # nocloud-net was folded into the unified "nocloud" datasource; ds=nocloud
    # now handles http(s)/ftp seedfrom URLs too (docs.cloud-init.io NoCloud ref).
    sed -i 's/linux.*vmlinuz.*/& autoinstall ds=nocloud\\;s=\/run\/cloud-init\/ console=ttyS0,115200n8/' "$GRUB_CFG"
    sed -i 's/set timeout=30/set timeout=5/' "$GRUB_CFG"
fi

# Copy cloud-init configuration
echo "☁️  Adding cloud-init configuration..."
mkdir -p "$WORK_DIR/iso/nocloud"
cp -r "$CLOUD_INIT_DIR"/* "$WORK_DIR/iso/nocloud/"

# Create new ISO
echo "💿 Creating modified ISO..."
xorriso -as mkisofs \
    -iso-level 3 \
    -full-iso9660-filenames \
    -volid "Ubuntu Server AutoInstall" \
    -eltorito-boot boot/grub/bios.img \
    -no-emul-boot \
    -boot-load-size 4 \
    -boot-info-table \
    --eltorito-catalog boot/grub/boot.cat \
    --grub2-boot-info \
    --grub2-mbr /usr/lib/grub/i386-pc/boot_hybrid.img \
    -eltorito-alt-boot \
    -e EFI/BOOT/grubx64.efi \
    -no-emul-boot \
    -append_partition 2 0xef EFI/BOOT/grubx64.efi \
    -output "$OUTPUT_ISO" \
    -graft-points \
    "$WORK_DIR/iso"

echo "✅ Autoinstall ISO created: $OUTPUT_ISO"
