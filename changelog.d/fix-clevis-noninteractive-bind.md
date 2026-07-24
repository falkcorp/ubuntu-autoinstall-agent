<!-- file: changelog.d/fix-clevis-noninteractive-bind.md -->
<!-- version: 1.0.0 -->
<!-- guid: 5b2e9c1a-7d84-4f0e-a3b6-9c1d2e4f6a80 -->
<!-- last-edited: 2026-07-24 -->

### Fixed

- **NativeKeystore D2-B install now actually binds clevis and fails loudly if it
  can't.** The Tang clevis bind ran with bare `{"url":...}` pins, so
  `clevis luks bind` demanded interactive `/dev/tty` trust confirmation and
  failed over SSH — and the failure was swallowed as a non-fatal warning, so the
  installer reported success on a keystore with **no unattended-unlock binding**.
  Now each Tang advertisement is pre-fetched (`curl .../adv`) and referenced via
  the clevis `adv` key so the bind is non-interactive, and a bind failure is
  **fatal**. The keystore-initramfs `dracut` regeneration is likewise no longer
  swallowed. Added `clevis-tpm2` + `tpm2-tools` to the live-env and target
  package sets so the SSS **tpm2 pin** can actually seal (previously missing).
