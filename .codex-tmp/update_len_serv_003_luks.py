from pathlib import Path
from datetime import datetime


stamp = datetime.now().strftime("%Y%m%d-%H%M%S")
user_data = Path("/var/www/html/cloud-init/6c4b90bcf7f4/user-data")
chroot = Path("/var/www/html/cloud-init/scripts/len-serv-003-chroot-setup.sh")
ipxe = Path("/var/www/html/ipxe/boot/mac-6c4b90bcf7f4.ipxe")

for path in (user_data, chroot, ipxe):
    backup = path.with_name(f"{path.name}.bak-luks-clevis-{stamp}")
    backup.write_bytes(path.read_bytes())
    print(f"backup {backup}")

text = user_data.read_text()
old_storage = """  storage:
    layout:
      name: direct
"""
new_storage = """  storage:
    layout:
      name: lvm
      match:
        path: /dev/nvme0n1
      sizing-policy: all
      password: "TANG_INITIAL_PASSPHRASE_REPLACE_WITH_CLEVIS"
"""
if old_storage not in text:
    raise SystemExit("expected direct storage block not found")
text = text.replace(old_storage, new_storage, 1)
if "    - clevis-initramfs\n" not in text:
    text = text.replace("    - clevis-dracut\n", "    - clevis-dracut\n    - clevis-initramfs\n", 1)
if "    - lvm2\n" not in text:
    text = text.replace("    - cryptsetup\n", "    - cryptsetup\n    - lvm2\n", 1)
user_data.write_text(text)

script = chroot.read_text()
marker = '\n/usr/local/bin/report-status.sh finished 100 "Installation complete on len-serv-003" || true\n'
insert = r'''
# Bind the installer-created LUKS root to the Tang quorum before first reboot.
# This must run in the target chroot while /dev, /run, /proc, and /sys are bound.
LUKS_DEV=$(blkid -t TYPE=crypto_LUKS -o device | head -n1 || true)
if [[ -z "$LUKS_DEV" ]]; then
  /usr/local/bin/report-status.sh failed 95 "No crypto_LUKS device found for clevis binding" || true
  echo "ERROR: no crypto_LUKS device found for clevis binding" >&2
  exit 1
fi

SSS_POLICY='{"t":2,"pins":{"tang":[{"url":"http://172.16.2.45"},{"url":"http://172.16.2.46"},{"url":"http://172.16.2.47"}]}}'
if clevis luks list -d "$LUKS_DEV" 2>/dev/null | grep -qE 'sss|tang'; then
  echo "Clevis binding already present on $LUKS_DEV"
else
  printf '%s' 'TANG_INITIAL_PASSPHRASE_REPLACE_WITH_CLEVIS' | clevis luks bind -y -k - -d "$LUKS_DEV" sss "$SSS_POLICY"
fi

if command -v update-initramfs >/dev/null 2>&1; then
  update-initramfs -u -k all
elif command -v dracut >/dev/null 2>&1; then
  dracut --force
else
  echo "ERROR: no initramfs rebuild tool found after clevis binding" >&2
  exit 1
fi

clevis luks list -d "$LUKS_DEV"
'''
if marker not in script:
    raise SystemExit("expected final report marker not found in chroot script")
if "Bind the installer-created LUKS root to the Tang quorum" not in script:
    script = script.replace(marker, insert + marker, 1)
chroot.write_text(script)

boot = ipxe.read_text()
if "set menu-default boot-local-disk" not in boot and "set menu-default autoinstall" not in boot:
    raise SystemExit("expected menu-default line not found in host boot override")
boot = boot.replace("set menu-default boot-local-disk", "set menu-default autoinstall", 1)
ipxe.write_text(boot)

print("updated user-data, chroot setup, and host boot override")
