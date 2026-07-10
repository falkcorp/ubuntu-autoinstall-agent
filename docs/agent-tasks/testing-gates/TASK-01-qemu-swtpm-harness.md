<!-- file: docs/agent-tasks/testing-gates/TASK-01-qemu-swtpm-harness.md -->
<!-- version: 1.0.0 -->
<!-- guid: f8af40f6-cb42-4fb6-b578-a4a350eab5a9 -->
<!-- last-edited: 2026-07-09 -->

# TASK-01 — scripts/vm-validate.sh: QEMU+swtpm VM gate (virtio /dev/vda + TPM2) resolving both VERIFY-ON-VM markers — the gate before ANY hardware attempt (todo:qemu-gate)

**Priority:** P1 · **Effort:** L · **Recommended subagent:** Sonnet-class · shell/infra subagent · **Why:** greenfield script; careful orchestration but no production-code risk · **Depends on:** installer-robustness/TASK-01 (HARD — the partition-suffix helper must be merged to origin/main first, or the VM install fails in Phase 2 on /dev/vdap1)

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/testing-gates-qemu-swtpm-harness" -b agent/testing-gates-qemu-swtpm-harness origin/main
cd "$REPO/.worktrees/testing-gates-qemu-swtpm-harness"
git rebase origin/main
```

(Protocol is also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is
authoritative for this task.)

## Goal

Create three net-new files — `scripts/vm-validate.sh` (QEMU+swtpm end-to-end validation
harness, bash, `set -euo pipefail`), `docs/vm-validation.md` (operator doc), and
`examples/configs/install/vm-test.yaml` (committed VM test config with THROWAWAY secrets
clearly labeled not-real) — implementing components C1–C3 of
`docs/specs/qemu-validation-design.md`. The harness boots the remastered SSH-ready ISO
(output of `scripts/make-ssh-ready-iso.sh`) in QEMU with OVMF UEFI, a virtio qcow2 target
disk (guest sees `/dev/vda`) and a swtpm-socket TPM2 (`tpm-tis`), interrogates the live
environment to resolve BOTH `VERIFY-ON-VM` markers in `scripts/build-installer-image.sh`,
runs the full `uaa install --config` against `/dev/vda` over SSH, reboots from disk, and
asserts LUKS unlock + `rpool`/`bpool` import + multi-user. REUSE, do not reinvent:
mirror the header/arg-parsing/`set -euo pipefail` conventions of
`scripts/make-ssh-ready-iso.sh`; base `vm-test.yaml` on the field set of
`examples/configs/install/unimatrixone.yaml` (it deserializes into `InstallationConfig`,
`src/network/ssh_installer/config.rs`). Do NOT reuse the Rust `VmManager` in
`src/utils/vm.rs` (LOCKED rejection in the design spec — Linux-oriented image-creation
code, no swtpm, and coupling the gate to the binary under test would be circular). Do NOT
edit `scripts/build-installer-image.sh` — the harness *reports* the marker answers;
acting on them is follow-up outside this task.

## Background (verify before editing)

- The Path B installer (`src/network/ssh_installer/`) is proven 7/7 phases on
  unimatrixone hardware (2026-07-09), but there is no repeatable pre-hardware gate.
  `scripts/build-installer-image.sh` carries two `VERIFY-ON-VM` markers its own header
  says to confirm "during the QEMU+swtpm validation": (a) ~line 72 — which unit actually
  autostarts the stock installer on the 26.04 live-server image (the script masks three
  *candidates*: `subiquity-server.service`, `serial-subiquity@.service`,
  `snap.subiquity.subiquity-server.service`); (b) ~line 81 — whether
  `debootstrap sgdisk zpool cryptsetup dracut clevis` exist in the live rootfs (the build
  script only WARNs when missing).
- No QEMU/swtpm/kvm shell script exists in `scripts/` — the only `swtpm` mentions
  repo-wide are comments. This script is greenfield.
- The virtio disk is the point: `/dev/vda` ends in a letter, so its partitions are
  `vda1..vda4` (no `p` infix). The pre-fix installer built `{}pN` paths at 11 call sites,
  so Phases 2–3 fail on virtio (`mkfs` on nonexistent `/dev/vdap1`, `zpool create` on
  `vdap3`) — GUID detection self-heals only the ESP. A green stage-4/5 run on `/dev/vda`
  is therefore the end-to-end proof of `installer-robustness/TASK-01` that nothing else
  can fake. This task exercises the partition-suffix helper end-to-end.
- The SSH-ready ISO (`scripts/make-ssh-ready-iso.sh` + `installer-image/nocloud/user-data`)
  boots the live session with openssh-server on, user `ubuntu-server` (throwaway live
  password `default`, operator key) and NOPASSWD sudo. It does NOT contain the `uaa`
  binary — the harness must copy the musl `uaa` binary and the VM config into the live VM
  over SSH before installing.
- **HARD RULES (restated):**
  - NEVER wipe/reimage/touch 172.16.2.30 ("the server") or len-serv-003. DO NOT run
    installs against any physical host — the ONLY install target this script may ever
    address is the qcow2 disk it creates itself. Runs on the server use a scratch
    directory only and leave nginx, autoinstall-agent, the debootstrap cache, netboot
    root, and CockroachDB node4 untouched.
  - Linux host required (the server 172.16.2.30 or any amd64 Linux box). macOS lacks KVM
    — the script must refuse non-Linux hosts at preflight with a clear message.
  - SECRETS: `vm-test.yaml` values are throwaway, VM-only, and every secret value must
    literally contain a `NOT-A-REAL-SECRET` marker. No real `luks_key`/`root_password`/
    `tpm2_pin` in git; no `REPLACE_AT_PLACE_TIME` placeholder may reach the install step.
  - Stay in your worktree; never push/PR/merge — the coordinator owns all git.

- **Re-verify these anchors before editing** — line numbers drift, they are a starting
  point only; zero hits where hits are expected = STOP and report:

  ```bash
  grep -n "VERIFY-ON-VM" scripts/build-installer-image.sh
  # expect: 3 hits: line 25 (NOTE), line 72 (mask stock installer autostart), line 81 (debootstrap+gdisk in live rootfs)
  grep -n "for tool in debootstrap" scripts/build-installer-image.sh
  # expect: 1 hit ~line 86
  grep -rln "swtpm" scripts/ src/
  # expect: 2 hits, both comment-only: scripts/build-installer-image.sh (line 28) and src/network/ssh_installer/system_setup.rs (line 717)
  grep -n "qemu-system" src/utils/vm.rs
  # expect: ~11 hits incl. lines 26, 45-46, 464-465 — read-only context; do NOT reuse this module
  grep -n "pub disk_device" src/network/ssh_installer/config.rs
  # expect: 1 hit at line 49
  grep -n "ubuntu-server" scripts/make-ssh-ready-iso.sh installer-image/nocloud/user-data
  # expect: >=1 hit in each file (live-session SSH user the harness logs in as)
  grep -n "Unified install subcommand" src/cli/args.rs
  # expect: 1 hit ~line 91 (the `install` subcommand: local when no --remote)
  ```

- **Dependency merged-check (run BEFORE writing any code):**

  ```bash
  grep -rn 'sdap' src --include='*.rs'
  # scout baseline BEFORE installer-robustness/TASK-01 merged: 4 hits
  #   (zfs_ops.rs:381 sdap3, system_setup.rs:1001 /dev/sdap1, system_setup.rs:1026 and :1032 luks /dev/sdap4)
  # REQUIRED for THIS task: 0 hits. If any sdap hit remains, the partition-suffix
  # helper has NOT merged — STOP and report "dependency installer-robustness/TASK-01
  # not merged"; do not proceed.
  ```

## Step-by-step

1. Run the ⛔ START HERE block, the anchor re-verify block, and the dependency
   merged-check above. Any unexpected result → STOP and report.
2. Create `scripts/vm-validate.sh` (mode 0755) with a 4-line `# file:` header
   (`# file: scripts/vm-validate.sh`, `# version: 1.0.0`, a NEW guid, `# last-edited:
   2026-07-09`), `#!/usr/bin/env bash`, `set -euo pipefail`, and `while [ $# -gt 0 ]`
   arg parsing mirroring `scripts/make-ssh-ready-iso.sh`. Arguments:
   - `--iso <path>` (required) — the remastered SSH-ready ISO;
   - `--agent <path>` (required) — the musl `uaa` binary
     (`target/x86_64-unknown-linux-musl/release/ubuntu-autoinstall-agent`);
   - `--config <path>` (default `examples/configs/install/vm-test.yaml`);
   - `--workdir <dir>` (default `./vm-validate-work`); `--disk-size` (default `40G`);
   - `--ssh-port` (default `10022`); `--boot-timeout` (default `600` s);
     `--install-timeout` (default `3600` s).
3. Implement the numbered stages below. Each stage echoes `==> stage N <name>` and logs
   to `$WORKDIR/logs/NN-<stage>.log`; on failure print the failing stage + log path and
   exit nonzero (fail-closed: a timeout is a FAIL, never a skip).
   - **Stage 0 preflight:** `[ "$(uname -s)" = Linux ]` or die with
     `"ERROR: Linux host required (no KVM on macOS) — run on the server 172.16.2.30 or any amd64 Linux box"`.
     `command -v` checks for `qemu-system-x86_64`, `swtpm`, `qemu-img`, `ssh`, `scp`,
     `sshpass` (or key-only fallback); locate OVMF firmware (`OVMF_CODE*.fd` under
     `/usr/share/OVMF` or `/usr/share/qemu`) or die naming the `ovmf` package. If
     `/dev/kvm` is not writable, WARN and fall back to TCG (slow), do not die. Refuse a
     `--config` whose file contains `REPLACE_AT_PLACE_TIME`
     (`grep -q REPLACE_AT_PLACE_TIME "$CONFIG" && die ...`) — placeholders must never
     reach an install.
   - **Stage 1 workspace:** `mkdir -p "$WORKDIR"/{logs,tpmstate}`;
     `qemu-img create -f qcow2 "$WORKDIR/disk.qcow2" "$DISK_SIZE"`; start
     `swtpm socket --tpmstate dir="$WORKDIR/tpmstate" --ctrl type=unixio,path="$WORKDIR/swtpm.sock" --tpm2 --daemon`
     (record its pid). Install a `trap ... EXIT` that kills the harness's own swtpm/QEMU
     pids — never `pkill` by name (the server may run other VMs).
   - **Stage 2 boot-iso:** launch QEMU backgrounded with: OVMF pflash/bios firmware;
     `-drive file="$WORKDIR/disk.qcow2",if=virtio` (guest sees `/dev/vda`);
     `-cdrom "$ISO"`; `-chardev socket,id=chrtpm,path="$WORKDIR/swtpm.sock"
     -tpmdev emulator,id=tpm0,chardev=chrtpm -device tpm-tis,tpmdev=tpm0`;
     `-netdev user,id=n0,hostfwd=tcp::${SSH_PORT}-:22 -device virtio-net-pci,netdev=n0`;
     `-m 4096 -smp 2`; `-serial file:$WORKDIR/logs/02-boot-iso-serial.log`;
     `-display none`. Add `-enable-kvm -cpu host` only when `/dev/kvm` is writable.
     Poll `ssh -p $SSH_PORT ubuntu-server@127.0.0.1 true` (password `default` via
     sshpass, or the operator key; `-o StrictHostKeyChecking=no -o
     UserKnownHostsFile=/dev/null`) until success or `--boot-timeout` → FAIL.
   - **Stage 3 interrogate (resolve BOTH VERIFY-ON-VM markers; record answers to
     `$WORKDIR/logs/03-interrogate.log` for the stage-7 report):**
     (a) `systemctl list-units --all --no-legend '*subiquity*'; systemctl list-unit-files
     --no-legend '*subiquity*'` inside the VM → record the EXACT unit name(s) that
     autostart the stock installer (or `NONE`); compare against the three names the build
     script masks and set verdict `COVERED` or `GAP (unit <name> not in mask list)`.
     (b) `for tool in debootstrap sgdisk zpool cryptsetup dracut clevis; do command -v
     "$tool" ...; done` inside the VM → record `present|MISSING` per tool. Stage-3
     findings are report-only: a MISSING tool alone does not fail the gate here (stage 4
     will fail on it, and the report says why).
   - **Stage 4 install:** `scp` the `--agent` binary to `/tmp/uaa` in the VM (`chmod
     +x`), `scp` the config to `/tmp/vm-test.yaml`; run
     `sudo /tmp/uaa install --config /tmp/vm-test.yaml` over SSH (local mode — no
     `--remote`; the live session IS the target host and `ubuntu-server` has NOPASSWD
     sudo), streaming output to `$WORKDIR/logs/04-install.log`, bounded by
     `--install-timeout`. Assert success: exit code 0 AND the log shows all 7 phases
     completed (`grep -c "Phase" appropriately — assert the final phase-7 completion
     line is present`). Nonzero exit or missing phase-7 completion → FAIL.
   - **Stage 5 boot-disk:** `system_powerdown` / SSH `sudo poweroff` and wait for the
     QEMU pid to exit; relaunch QEMU with the SAME disk, SAME swtpm socket/state, SAME
     hostfwd, but NO `-cdrom`; serial to `logs/05-boot-disk-serial.log`. Poll SSH (root
     with the config's `ssh_authorized_keys`, or the console log) until reachable or
     `--boot-timeout` → FAIL. Note: first boot may pause for the LUKS passphrase if no
     auto-unlock slot applies — send the throwaway `luks_key` on the serial console if
     prompted (document this branch in the script comments).
   - **Stage 6 assert (in order, inside the booted installed system):**
     `cryptsetup status luks` output contains `is active` (LUKS unlocked with the test
     key); `zpool list -H -o name` lists BOTH `rpool` AND `bpool`;
     `systemctl is-system-running --wait` returns `running`/`degraded` or `systemctl
     is-active multi-user.target` = `active` (multi-user reached). Each assertion logs
     PASS/FAIL; first FAIL fails the gate.
   - **Stage 7 report:** print EXACTLY this machine-greppable block (values filled from
     stage 3/6), then exit 0 only if stages 2–6 all passed:

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
4. Create `examples/configs/install/vm-test.yaml` with a 4-line `# file:` header (new
   guid) and a LOUD comment block: "THROWAWAY VM-ONLY TEST SECRETS — NOT REAL. Never use
   on hardware.". Base it on `examples/configs/install/unimatrixone.yaml`'s field set
   (verify fields against `InstallationConfig` in `src/network/ssh_installer/config.rs`
   before writing). Values: `hostname: vm-test`, `disk_device: /dev/vda`,
   `timezone: America/New_York`,
   `luks_key: vm-test-throwaway-luks-key-NOT-A-REAL-SECRET`,
   `root_password: vm-test-throwaway-root-pw-NOT-A-REAL-SECRET`,
   `network_interface: enp0s3` (QEMU user-net NIC — note in a comment that the actual
   name may need confirming on first VM run), `network_address: 10.0.2.15/24`,
   `network_gateway: 10.0.2.2`, `network_search: vm.local`, nameservers `[10.0.2.3]`,
   `debootstrap_release: resolute`,
   `debootstrap_mirror: http://archive.ubuntu.com/ubuntu/`, `initramfs_type: dracut`,
   `tang_servers: []` (Tang unlock is a v1 non-goal per the design spec — do NOT point at
   the real Tang servers), `ssh_authorized_keys:` the operator key already committed in
   `installer-image/nocloud/user-data`, `enroll_tpm2: true`,
   `tpm2_pin: "123456"` (throwaway, commented NOT-A-REAL-SECRET), `tpm2_pcr_ids: "7"`,
   `expect_fido2: false` (no YubiKey in a VM). MUST NOT contain
   `REPLACE_AT_PLACE_TIME` anywhere.
5. Create `docs/vm-validation.md` (4-line `<!-- file: -->` header, new guid) covering:
   prerequisites (Linux host — the server 172.16.2.30 or any amd64 Linux box; macOS
   explicitly unsupported, no KVM; packages `qemu-system-x86`, `swtpm`, `ovmf`,
   `squashfs-tools`, `sshpass`); how to build the inputs (`scripts/build-musl.sh` for the
   agent, `scripts/make-ssh-ready-iso.sh` for the ISO); the invocation
   (`sudo ./scripts/vm-validate.sh --iso <iso> --agent <uaa-musl>`); the stage list; how
   to read the VERIFY-ON-VM report (COVERED vs GAP, present vs MISSING and what to do
   about each — fix `build-installer-image.sh`'s mask list / bake tools into the
   overlay as FOLLOW-UP work, not part of this task); the note that runs on the server
   work entirely in a scratch dir and must not touch its live services; and, verbatim,
   the gate statement: `THIS SCRIPT PASSING IS THE GATE — no hardware attempt or
   len-serv-003 wipe before it passes.`
6. Purely additive: create the three files above and NOTHING else. Do not edit
   `scripts/build-installer-image.sh`, `src/utils/vm.rs`, any Rust source, or any
   existing script. Edge semantics to preserve exactly: no-KVM → WARN + TCG fallback
   (not a die); non-Linux → die; missing dependency binary → die naming the package;
   stage-3 MISSING tool → recorded, gate continues; any stage 2–6 assertion timeout →
   FAIL (never skip); placeholder-bearing config → die at preflight.
7. Run the How-to-test gates below.

## How to test

Authoring-time gates (the full VM run is the runtime test — Linux-only, operator-run
after merge; you cannot run it on macOS and MUST NOT attempt it against any physical
host):

```bash
bash -n scripts/vm-validate.sh
# Expected: exit 0 (parses clean)
command -v shellcheck >/dev/null && shellcheck scripts/vm-validate.sh || echo "shellcheck not installed - skipped"
# Expected: no errors (warnings triaged; SC2086-class quoting issues fixed)
cargo test --lib --offline
# Expected: 237+ passed; 0 failed (no Rust touched — count unchanged from current origin/main)
cargo build --offline
# Expected: exit 0
grep -c "VERIFY-ON-VM REPORT" scripts/vm-validate.sh
# Expected: 1 or more (report block emitted)
grep -n "uname -s" scripts/vm-validate.sh
# Expected: 1+ hit (Linux-host preflight present)
grep -n "if=virtio" scripts/vm-validate.sh
# Expected: 1+ hit (target disk is virtio -> /dev/vda)
grep -n "tpm-tis" scripts/vm-validate.sh
# Expected: 1+ hit (swtpm TPM2 wired in)
grep -rn "REPLACE_AT_PLACE_TIME" examples/configs/install/vm-test.yaml || true
# Expected: no output (throwaway values, not placeholders)
```

Runtime gate (documented for the operator; NOT run by this task):

```bash
sudo ./scripts/vm-validate.sh --iso <remastered-iso> --agent target/x86_64-unknown-linux-musl/release/ubuntu-autoinstall-agent
# Expected: "GATE: PASS", exit 0, and a VERIFY-ON-VM REPORT naming the observed
#           autostart unit(s) and present/MISSING for all six tools
```

## Acceptance criteria

- [ ] THIS SCRIPT PASSING IS THE GATE — no hardware attempt or len-serv-003 wipe before it passes. The literal sentence appears in both new docs/script:
      `grep -rn "THIS SCRIPT PASSING IS THE GATE" scripts/vm-validate.sh docs/vm-validation.md` → 1+ hit in each file.
- [ ] `bash -n scripts/vm-validate.sh` exits 0; `shellcheck scripts/vm-validate.sh` clean if shellcheck is installed.
- [ ] Tests green: `cargo test --lib --offline` → 237+ passed, 0 failed (unchanged); `cargo build --offline` exits 0.
- [ ] Script refuses non-Linux hosts: `grep -n "uname -s" scripts/vm-validate.sh` → 1+ hit, and the die message names the server 172.16.2.30 / amd64 Linux.
- [ ] Virtio + TPM2 wiring present: `grep -n "if=virtio" scripts/vm-validate.sh` → 1+; `grep -n "tpm-tis" scripts/vm-validate.sh` → 1+; `grep -n "swtpm socket" scripts/vm-validate.sh` → 1+.
- [ ] Both markers resolved in the report: `grep -n "build-installer-image.sh:72" scripts/vm-validate.sh` → 1+; `grep -n "build-installer-image.sh:81" scripts/vm-validate.sh` → 1+; `grep -cn "debootstrap" scripts/vm-validate.sh` → 1+ (six-tool loop present).
- [ ] Anti-over-suppression (the harness is one big fail-closed gate — prove the pass path exists): `grep -n "GATE: PASS" scripts/vm-validate.sh` → 1+ AND `grep -n "GATE: FAIL" scripts/vm-validate.sh` → 1+ — the script can emit BOTH verdicts, and stage-3 MISSING findings are report-only (verify: the stage-3 code records MISSING without exiting).
- [ ] `examples/configs/install/vm-test.yaml` exists, sets `disk_device: /dev/vda` (`grep -n "disk_device: /dev/vda" examples/configs/install/vm-test.yaml` → 1 hit), every secret value contains `NOT-A-REAL-SECRET` (`grep -c "NOT-A-REAL-SECRET" examples/configs/install/vm-test.yaml` → 3+), and `grep -c "REPLACE_AT_PLACE_TIME" examples/configs/install/vm-test.yaml || true` outputs 0.
- [ ] `docs/vm-validation.md` documents prerequisites, invocation, and report reading; macOS explicitly unsupported (`grep -in "macos" docs/vm-validation.md` → 1+ hit).
- [ ] No existing file modified: `git status --porcelain` shows only the three new files (plus their dirs).
- [ ] File headers present on all three new files with `last-edited: 2026-07-09` (`grep -n "last-edited: 2026-07-09" scripts/vm-validate.sh docs/vm-validation.md examples/configs/install/vm-test.yaml` → 1 hit each).

## Commit message

```
feat(testing): add QEMU+swtpm VM validation gate (scripts/vm-validate.sh)

Greenfield harness: boots the remastered SSH-ready ISO with OVMF + swtpm TPM2
against a virtio qcow2 disk (/dev/vda — end-to-end proof of the partition-suffix
helper), runs the full uaa install with a committed throwaway-secret VM config,
reboots from disk, asserts LUKS unlock + rpool/bpool import + multi-user, and
prints a VERIFY-ON-VM report resolving both markers in build-installer-image.sh.
Adds docs/vm-validation.md (Linux-host-only; the server or any amd64 box) and
examples/configs/install/vm-test.yaml. THIS SCRIPT PASSING IS THE GATE — no
hardware attempt or len-serv-003 wipe before it passes.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

Idempotency (additive — check for the NEW files' presence):
`test -f scripts/vm-validate.sh && grep -n "VERIFY-ON-VM REPORT" scripts/vm-validate.sh && test -f docs/vm-validation.md && test -f examples/configs/install/vm-test.yaml`
— if all hit, the harness already exists; run the acceptance checks instead of
re-applying. Rollback: `git revert` of the single commit removes three net-new files
with zero blast radius — no production code references them, siblings unaffected. The
process rule survives the revert: until an operator run prints `GATE: PASS`, hardware
installs and len-serv-003 remain off-limits.
