<!-- file: changelog.d/serial-console-installed-system.md -->
<!-- version: 1.0.0 -->
<!-- guid: 3c9d1a72-5e84-4b60-9f13-7a2e6c0d8b41 -->
<!-- last-edited: 2026-07-22 -->

### Added

#### Serial console on every installed system (headless-fleet observability)

The installer now writes `/etc/default/grub.d/99-uaa-serial-console.cfg` before
`update-grub`, giving every installed host a serial console
(`console=tty0 console=ttyS0,115200n8`, `GRUB_TERMINAL="console serial"`,
`GRUB_SERIAL_COMMAND=… --speed=115200 --unit=0`). The fleet is headless servers
watched over IPMI SOL, so the boot — including the LUKS/keystore unlock prompt —
must land on `ttyS0` or it is invisible remotely (and to the VM-gate disk-boot
serial capture).

Written as a `grub.d` drop-in, which `grub-mkconfig` sources **after**
`/etc/default/grub`, so `GRUB_CMDLINE_LINUX="$GRUB_CMDLINE_LINUX …"` **appends**
to whatever the dracut+Tang step already set rather than clobbering it; `ttyS0`
is listed last so it is the primary console. Applies to every install (the whole
fleet is headless); harmless on a host with no physical UART.
