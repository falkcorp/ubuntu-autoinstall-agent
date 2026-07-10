<!-- file: docs/vm-validation.md -->
<!-- version: 1.0.0 -->
<!-- guid: bd881ea8-3d72-4911-8eb1-ae5560cc7b97 -->
<!-- last-edited: 2026-07-09 -->

# VM validation gate (`scripts/vm-validate.sh`)

**THIS SCRIPT PASSING IS THE GATE — no hardware attempt or len-serv-003 wipe
before it passes.**

The Path B installer (`src/network/ssh_installer/`) is proven 7/7 phases on
unimatrixone hardware, but there was no repeatable pre-hardware validation
gate. `scripts/vm-validate.sh` boots the remastered SSH-ready ISO in QEMU
with OVMF UEFI firmware, a virtio qcow2 target disk (the guest sees
`/dev/vda`), and a swtpm-emulated TPM2 device; runs the full `uaa install`
against it; reboots from the installed disk; and asserts LUKS unlock, ZFS
pool import, and multi-user. It also resolves both `VERIFY-ON-VM` markers in
`scripts/build-installer-image.sh` and prints the answers in a
machine-greppable report.

## Prerequisites

- **A Linux host — the server (172.16.2.30) or any amd64 Linux box.**
  **macOS is explicitly unsupported: it has no KVM**, and
  `scripts/vm-validate.sh` refuses to run there at preflight (stage 0 checks
  `uname -s = Linux` and dies with a clear message otherwise). If you are on
  macOS, copy the ISO and agent binary to the server and run the harness
  there instead.
- Packages: `qemu-system-x86`, `swtpm`, `ovmf` (provides `OVMF_CODE*.fd` /
  `OVMF_VARS*.fd`, normally under `/usr/share/OVMF` or `/usr/share/qemu`),
  `squashfs-tools` (only needed if you still need to build the installer
  image), `sshpass` (for the live-session password login — optional if you
  have the operator SSH key loaded in an agent instead), and `socat`
  (optional — enables best-effort auto-answering of a LUKS passphrase prompt
  on first disk boot; see "First disk boot and the LUKS passphrase" below).
- `/dev/kvm` writable is recommended (fast, hardware-accelerated) but not
  required — if it is not writable the harness WARNs and falls back to TCG
  software emulation (slow, but not a failure).

## Building the inputs

1. Build the musl agent binary:

   ```bash
   ./scripts/build-musl.sh
   # produces target/x86_64-unknown-linux-musl/release/ubuntu-autoinstall-agent
   ```

2. Re-master a stock Ubuntu Server ISO into an SSH-ready ISO:

   ```bash
   ./scripts/make-ssh-ready-iso.sh /path/to/ubuntu-26.04-live-server-amd64.iso
   # produces ubuntu-26.04-live-server-amd64-ssh-ready.iso
   ```

   This is the ISO the VM gate boots — it comes up with `openssh-server`
   enabled, user `ubuntu-server` (throwaway live password `default` +
   operator key) and NOPASSWD sudo, with no manual per-boot setup.

## Invocation

```bash
sudo ./scripts/vm-validate.sh \
    --iso  ubuntu-26.04-live-server-amd64-ssh-ready.iso \
    --agent target/x86_64-unknown-linux-musl/release/ubuntu-autoinstall-agent
```

Root (or a user in the `kvm`/`libvirt` group with QEMU permissions) is
needed to access `/dev/kvm` and bind the QEMU network hostfwd port.

Optional flags (all have defaults):

| Flag | Default | Meaning |
|---|---|---|
| `--config <path>` | `examples/configs/install/vm-test.yaml` | Committed throwaway-secret VM install config |
| `--workdir <dir>` | `./vm-validate-work` | Scratch directory for the qcow2 disk, swtpm state, and all logs |
| `--disk-size <size>` | `40G` | Target virtio disk size |
| `--ssh-port <port>` | `10022` | Host-side hostfwd port for the guest's SSH |
| `--boot-timeout <seconds>` | `600` | Max wait for each boot (ISO and disk) to become SSH-reachable |
| `--install-timeout <seconds>` | `3600` | Max wait for `uaa install` to finish |

Everything the harness creates lives under `--workdir` (default
`./vm-validate-work`): the qcow2 disk image, the swtpm TPM state directory,
and per-stage logs at `<workdir>/logs/NN-<stage>.log`. It is always safe to
`rm -rf` that directory between runs — nothing outside it is touched, and
nothing on the server's live services (nginx, autoinstall-agent, the
debootstrap cache, the netboot root, or the CockroachDB node4 instance) is
read or written.

## Stages

0. **preflight** — Linux-host check, required-tool checks
   (`qemu-system-x86_64`, `swtpm`, `qemu-img`, `ssh`, `scp`; `sshpass`/`socat`
   optional with a WARN fallback), OVMF firmware discovery, `/dev/kvm`
   writability check (WARN + TCG fallback, not a failure), and a refusal to
   proceed if `--config` still contains a `REPLACE_AT_PLACE_TIME` placeholder.
1. **workspace** — creates the qcow2 disk and starts the swtpm socket daemon
   (its own tpmstate directory, its own control socket).
2. **boot-iso** — launches QEMU with the virtio disk, the SSH-ready ISO as
   `-cdrom`, the swtpm TPM2 device (`tpm-tis`), and a hostfwd NAT port for
   SSH; polls SSH until the live session answers or `--boot-timeout` expires.
3. **interrogate** — resolves **both** `VERIFY-ON-VM` markers inside the live
   environment (see "Reading the report" below). Findings here are
   report-only: a `MISSING` tool does not fail the gate by itself.
4. **install** — copies the agent binary and `--config` into the live
   session over SSH/SCP and runs `sudo uaa install --config
   /tmp/vm-test.yaml`, asserting exit code 0 and that all 7 install phases
   (`Phase 0` through `Phase 6: Final setup`) completed.
5. **boot-disk** — shuts the VM down, then relaunches QEMU from the same
   qcow2 disk and the same swtpm state (no `-cdrom` this time); polls SSH as
   `root` until reachable or `--boot-timeout` expires.
6. **assert** — inside the booted, installed system, in order: LUKS is
   unlocked (`cryptsetup status luks` contains `is active`), both `rpool`
   and `bpool` are imported (`zpool list -H -o name`), and multi-user has
   been reached (`systemctl is-system-running --wait` or
   `multi-user.target` active). The first failing assertion fails the gate.
7. **report** — prints the `VERIFY-ON-VM REPORT` block and the final
   `GATE: PASS` or `GATE: FAIL (<stage>)` line, then exits 0 only if stages
   2 through 6 all passed.

Fail-closed throughout: any boot/SSH/install timeout is a **FAIL**, never a
silently-skipped stage.

## First disk boot and the LUKS passphrase

The installed system's TPM2+PIN keyslot is enrolled by a first-boot oneshot
unit (it must bind to the *installed* system's PCR values, not the live
installer's — see `enroll_tpm2` in
`src/network/ssh_installer/config.rs`), so it is **not yet active** on the
very first boot of the installed disk. That first boot may pause at a LUKS
passphrase prompt on the serial console. If `socat` is installed, stage 5
wires the serial console through a unix socket and best-effort sends the
throwaway `luks_key` from `--config` when it detects a passphrase prompt. If
`socat` is not installed this auto-answer is skipped (a WARN is printed at
preflight) — a genuine hang is still caught by `--boot-timeout` and reported
as a stage-5 FAIL, so the gate never silently passes on a stuck boot.

## Reading the `VERIFY-ON-VM` report

```text
==== VERIFY-ON-VM REPORT ====
marker build-installer-image.sh:72 (stock-installer autostart unit):
  observed-units: <exact unit name(s) or NONE>
  masked-by-build-script: subiquity-server.service serial-subiquity@.service snap.subiquity.subiquity-server.service
  verdict: COVERED | GAP (unit <name> not in mask list)
marker build-installer-image.sh:81 (live-rootfs tools):
  debootstrap: present|MISSING
  sgdisk:      present|MISSING
  zpool:       present|MISSING
  cryptsetup:  present|MISSING
  dracut:      present|MISSING
  clevis:      present|MISSING
GATE: PASS | FAIL (<first failing stage>)
=============================
```

- **`verdict: COVERED`** — every unit `build-installer-image.sh` observed
  autostarting the stock installer is already in its mask list. No action
  needed.
- **`verdict: GAP (unit <name> not in mask list)`** — the 26.04 live-server
  image autostarts a unit the build script does not mask. **Follow-up work**
  (outside this task): add `<name>` to the mask list in
  `scripts/build-installer-image.sh` (around line 76).
- **`present`** for one of the six tools — nothing to do.
- **`MISSING`** for one of the six tools — **follow-up work** (outside this
  task): bake that tool into the overlay in `scripts/build-installer-image.sh`
  (it currently only WARNs and ships a broken image if the tool is absent).
  A `MISSING` finding here usually correlates with a stage-4 install failure
  in the corresponding phase, which the stage-4 log will show directly.

`scripts/vm-validate.sh` only **reports** these findings — acting on them
(editing `build-installer-image.sh`'s mask list, or baking tools into the
overlay) is deliberately out of scope for this harness; see
`docs/specs/qemu-validation-design.md` for the full design rationale.

## The gate

**THIS SCRIPT PASSING IS THE GATE — no hardware attempt or len-serv-003 wipe
before it passes.** Runs on the server work entirely inside `--workdir` (a
scratch directory) and must never touch its live services or any other
host. The *only* disk `scripts/vm-validate.sh` ever addresses is the qcow2
image it creates itself.
