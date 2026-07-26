<!-- file: changelog.d/fix-keystore-netdev-and-serial-getty.md -->
<!-- version: 1.0.0 -->
<!-- guid: 7f1c2a90-4e6b-4d21-8a3c-2b9d5e7c1f04 -->
<!-- last-edited: 2026-07-26 -->

### Fixed

- **NativeKeystore keystore-LUKS now unlocks after the network is online, not
  before.** The keystore crypttab entry lacked the `_netdev` option, so
  `systemd-cryptsetup-generator` filed the unlock unit in the local
  `cryptsetup.target` phase — ordered *before* networking. On a D2-B (clevis SSS
  `t=2`) binding, which needs at least one reachable Tang server, this meant
  `clevis-luks-askpass` fired while the NIC was still down: every Tang share
  failed, `t=2` was unmet, `zfs load-key` never ran, and `sysroot.mount` dropped
  the boot to the dracut emergency shell. Adding `_netdev` reroutes the unit to
  `remote-cryptsetup.target` (ordered after `network-online`), so clevis waits
  for the network — including a slow DHCP lease — instead of racing it.
- **A login getty now appears on both serial ports (COM1/ttyS0 and
  COM2/ttyS1).** The installer set the kernel `console=` args via the GRUB
  drop-in but never enabled the `serial-getty@` units, so on real root the
  auto-spawn heuristic left one COM port with no login prompt. Both
  `serial-getty@ttyS0` and `serial-getty@ttyS1` are now explicitly enabled so
  the operator gets a prompt regardless of which COM port IPMI SOL is wired to.
