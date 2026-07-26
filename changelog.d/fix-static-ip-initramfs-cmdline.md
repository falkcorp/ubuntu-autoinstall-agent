<!-- file: changelog.d/fix-static-ip-initramfs-cmdline.md -->
<!-- version: 1.0.0 -->
<!-- guid: 3f9a1c48-7b26-4d05-9e13-8c2f6b0a4d17 -->
<!-- last-edited: 2026-07-27 -->

### Changed

- **The initramfs network is now configured statically instead of `ip=dhcp`.**
  On Ubuntu 26.04 the systemd-networkd dracut module installs a default
  `zzzz-dracut-default.network` that forces `DHCP=yes` on every interface and
  sets up no wait-for-network logic, so `ip=dhcp` produced a late/duplicate DHCP
  lease (registering the host under a second DNS record) and `network-online`
  never settled. For hosts with a static `network_address`, the installer now
  emits `ip=<ip>::<gateway>:<prefix>::<iface>:none` on the kernel command line —
  parsed by `systemd-network-generator`, which overrides the DHCP default and
  brings the interface up deterministically before Tang is queried. Hosts whose
  `network_address` is literally `dhcp` still get `ip=dhcp`.
