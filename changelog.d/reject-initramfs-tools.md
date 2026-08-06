<!-- file: changelog.d/reject-initramfs-tools.md -->
<!-- version: 1.0.0 -->
<!-- guid: 3f8a1d6c-0b47-4e29-9c15-a7d2e64b8f31 -->
<!-- last-edited: 2026-08-03 -->

### Changed

- **`initramfs_type: initramfs-tools` is now a hard configuration error.** The
  fleet is dracut everywhere, and initramfs-tools is not merely unmaintained
  here — it is unsafe. The mechanism that bounds a *failed* disk unlock is a
  systemd drop-in on `initrd.target`
  (`JobTimeoutSec` + `JobTimeoutAction=reboot-force`, shipped as
  `dracut/92uaa-unlock-deadline/`). initramfs-tools has no systemd in the
  initramfs, so there is no `initrd.target` and no job to time out: a failed
  unlock waits at an interactive prompt indefinitely (measured at 902 seconds
  and still waiting). On a host with no remote power — the RPi Tang servers —
  that is a physical trip. The combination is now refused at authoring time
  rather than discovered at boot.

  The rejection fires in `validate_resolved` (rule 7) for the profile pipeline
  **and** in `InstallationConfig::from_yaml_file` for the hand-written-YAML CLI
  path, which never reaches `validate_resolved`. The `InitramfsType::InitramfsTools`
  enum variant is deliberately retained so an old committed config still
  deserializes and lands on the explanation above, instead of an opaque serde
  "unknown variant" error. No committed config or fixture selects it, so the
  len-serv and unimatrixone rebuilds are unaffected.
