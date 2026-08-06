<!-- file: changelog.d/fix-verify-policy-command-injection.md -->
<!-- version: 1.0.0 -->
<!-- guid: 0d5f2a68-7c14-4e93-b8a0-6f31c9e2074b -->
<!-- last-edited: 2026-08-05 -->

### Security

- **`uaa verify-policy --device` no longer reaches a shell.** The first version
  of the command ran `sh -c "<probe command> <device>"`, interpolating a
  command-line argument into a shell string. `--device '/dev/x; <anything>'`
  executed `<anything>` — and the VM gate invokes this under `sudo`, so it
  executed as root. The probe is now argv-separated: the shared command string
  is split into a program and its flags, and the device is passed as its own
  argument, so no shell ever parses it.

  Argv separation stops shell injection but not *argument* injection, so a
  device beginning with `-` is refused outright rather than handed to
  `cryptsetup` where it would be read as a flag. (`clap` intercepts
  `--device -x`, but not `--device=-x`, which reaches the function.)

  Covered by a regression test that asserts the payload does not run, gated on
  a control proving the same payload *does* run under `sh -c` — so a pass
  cannot be a false negative from `cryptsetup` merely being absent.
