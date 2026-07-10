<!-- file: docs/specs/qemu-validation-design.md -->
<!-- version: 1.0.0 -->
<!-- guid: 8dcd50bd-8cfd-47c0-9ebb-9adb72b78f02 -->
<!-- last-edited: 2026-07-09 -->

# QEMU/swtpm VM Validation Gate + LocalClient Tests — Design Spec

**Status:** Approved — ready for implementation planning
**Scope:** workstream `testing-gates` — one greenfield shell harness (`scripts/vm-validate.sh` + `docs/vm-validation.md`) and one test-only Rust change (`src/network/local.rs`). No production Rust behavior changes.

---

## Motivation (Problem)

The Path B installer (`src/network/ssh_installer/`) is proven 7/7 phases on unimatrixone
hardware (2026-07-09), but there is **no repeatable pre-hardware validation gate**. Two
concrete gaps:

1. **Two unresolved VERIFY-ON-VM markers** in `scripts/build-installer-image.sh` block
   trusting the remastered installer image on hardware:
   - line ~72: the exact name of the stock-installer autostart unit to mask on the
     26.04 live-server image (the script currently masks three *candidate* units:
     `subiquity-server.service`, `serial-subiquity@.service`,
     `snap.subiquity.subiquity-server.service`);
   - line ~81: whether `debootstrap`/`sgdisk`/`zpool`/`cryptsetup`/`dracut`/`clevis`
     actually exist in the live rootfs (the build script only WARNs when missing).
   The script header (line ~25) explicitly says to confirm both "during the QEMU+swtpm
   validation" — which does not exist yet. No QEMU/swtpm/kvm shell script exists in
   `scripts/`; the only `swtpm` mentions repo-wide are comments.

2. **Zero tests exercise `LocalClient`** (`src/network/local.rs`): the file has no
   `#[cfg(test)]` module and nothing under `tests/` references it, yet `LocalInstall`
   routes the entire installer through it (`SshInstaller` wires
   `Box::new(LocalClient::new())` at `src/network/ssh_installer/installer.rs` lines 38/81).
   The 237 lib tests fake SSH via `CommandExecutor` mocks (`MockExecutor` in
   `src/autoinstall/verify.rs`, `RecordingMock` in `src/autoinstall/place.rs`) but never
   the local runner.

**Goal:** a Linux-hosted `scripts/vm-validate.sh` that boots the remastered ISO in
QEMU+swtpm, runs the full `uaa` install against a virtio disk, boots the result, asserts
LUKS unlock + ZFS import + multi-user, and prints a report resolving both VERIFY-ON-VM
markers — plus a `LocalClient` unit-test module — so that **no hardware install attempt
(and no len-serv-003 wipe) happens until `vm-validate.sh` passes**.

## Current behavior (grep-verified)

Every claim below pairs with a re-verify grep (run from repo root); do not trust bare
line numbers without re-running these:

```bash
grep -n "VERIFY-ON-VM" scripts/build-installer-image.sh
# expect 3 hits: line 25 (NOTE), line 72 (mask stock installer autostart), line 81 (debootstrap+gdisk in live rootfs)
grep -n "for tool in debootstrap" scripts/build-installer-image.sh
# expect 1 hit ~line 86
grep -rln "swtpm" scripts/ src/
# expect 2 hits, both comment-only: scripts/build-installer-image.sh (line 28) and src/network/ssh_installer/system_setup.rs (line 717)
grep -n "qemu-system" src/utils/vm.rs
# expect ~11 hits incl. lines 26, 45-46, 464-465
grep -n "pub trait CommandExecutor" src/network/executor.rs
# expect 1 hit at line 11
grep -n "impl CommandExecutor for" src/network/executor.rs src/autoinstall/place.rs src/autoinstall/verify.rs
# expect 4 hits: executor.rs:45 (SshClient), executor.rs:89 (LocalClient), place.rs:404 (RecordingMock), verify.rs:547 (MockExecutor)
grep -n "pub struct LocalClient" src/network/local.rs
# expect 1 hit at line 12
grep -c "cfg(test)" src/network/local.rs
# expect outputs 0 (grep exits nonzero on zero count)
grep -n "LocalClient" src/network/ssh_installer/installer.rs
# expect 4 hits: lines 9 (doc), 17 (use), 38 and 81 (Box::new(LocalClient::new()))
grep -rn 'sdap' src --include='*.rs'
# expect 4 hits: zfs_ops.rs:381 (sdap3), system_setup.rs:1001 (/dev/sdap1), system_setup.rs:1026 and :1032 (luks /dev/sdap4)
```

The `sdap` grep matters here: on a QEMU **virtio** disk the target is `/dev/vda`, whose
partitions are `vda1..vda4` (name ends in a letter — no `p` infix). Today's installer
builds `{}pN` paths at 11 production call sites, so Phase 2 (mkfs on `vdap1`/`vdap2`,
cryptsetup on `vdap4`) fails first on virtio. That is why this harness has a **hard
dependency** on `installer-robustness/TASK-01` (the partition-suffix helper) — the VM
gate is unrunnable before that lands.

## Goals

- One command on a Linux host produces a PASS/FAIL verdict for the full ISO → install →
  reboot → unlocked-and-running pipeline, with logs.
- The run prints a **VERIFY-ON-VM report** answering both markers: (a) the exact unit
  name(s) that autostart the stock installer on the 26.04 live-server image, (b) the
  presence/absence of each of `debootstrap sgdisk zpool cryptsetup dracut clevis` in the
  live rootfs.
- TPM2 present in the VM via swtpm socket, so the TPM2-enrollment path is exercised
  rather than silently skipped.
- `LocalClient` gains direct unit tests through real (harmless) command execution.

## Non-goals (v1)

- Tang-server network unlock inside the VM — deferred (needs a second VM or host Tang;
  TPM2 + passphrase paths are enough to gate hardware).
- macOS host support — explicitly unsupported (no KVM; `src/utils/vm.rs` accel is
  `kvm:tcg` and Linux-oriented). Documented constraint, not a work item.
- CI integration — the harness is operator-run on a Linux box first; wiring into CI is a
  follow-up once runtimes are known.
- Any change to the installer's production code paths (that is workstream
  `installer-robustness`).

## Decisions (LOCKED — do not reopen)

1. **Greenfield `scripts/vm-validate.sh` (bash).** Rejected alternative: reusing the
   Rust `VmManager` in `src/utils/vm.rs`. It is Linux-oriented QEMU launch code with
   hardcoded `/tmp` paths and `accel=kvm:tcg`, built for image creation — it has no
   swtpm support, no serial-console assertion loop, and coupling the validation gate to
   the agent binary under test would be circular. It is **not reused**.
2. **Linux host required** (e.g. the server 172.16.2.30 or any amd64 Linux box). macOS
   lacks KVM; the harness preflights and refuses non-Linux hosts with a clear message.
   Runs on the server are read-only with respect to server services (nginx,
   autoinstall-agent, debootstrap cache, netboot root, CockroachDB node4 stay untouched;
   the harness works entirely in a scratch directory).
3. **Virtio disk → target is `/dev/vda`.** Hard dependency on
   `installer-robustness/TASK-01` (suffix-aware partition-path helper). The harness is
   itself the end-to-end proof of that fix: Phase 2/3 succeeding on `vda` is the
   regression test GUID detection cannot fake.
4. **swtpm socket TPM2** (`swtpm socket --tpmstate ... --ctrl type=unixio,path=...` +
   QEMU `-chardev socket,id=chrtpm -tpmdev emulator -device tpm-tis`).
5. **The harness IS the gate:** no hardware install attempt, and **no len-serv-003
   wipe**, until `scripts/vm-validate.sh` exits 0. This restates the standing hard rule.
6. **`LocalClient` tests use harmless real commands** (`echo`, `true`, `false`,
   `cp` on tempfiles). Rejected alternative: injecting a fake process runner into
   `LocalClient` — there is no seam *inside* it (the mock seam is the `CommandExecutor`
   trait one level up, `src/network/executor.rs:11`), and adding one would change
   production code for test convenience. `LocalClient` executes real `bash -c`, so the
   tests execute real, side-effect-free commands.

## Components

### C1. `scripts/vm-validate.sh` (greenfield, bash, `set -euo pipefail`)

Stage layout (each stage logs to `$WORKDIR/logs/NN-<stage>.log`; failure prints the
failing stage + log path and exits nonzero):

```bash
#!/bin/bash
# Stages (normative):
#  0 preflight   - Linux host (uname -s = Linux); qemu-system-x86_64, swtpm,
#                  OVMF firmware (OVMF_CODE), mkisofs/xorriso as needed; KVM available
#                  (/dev/kvm writable) else WARN and fall back to TCG (slow);
#                  required args present.
#  1 workspace   - create $WORKDIR (default ./vm-validate-work, overridable);
#                  qemu-img create -f qcow2 disk.qcow2 ${DISK_SIZE:-40G};
#                  swtpm socket started with its own tpmstate dir + unix ctrl socket.
#  2 boot-iso    - QEMU boots the remastered installer ISO/kernel+squashfs with:
#                  -drive file=disk.qcow2,if=virtio  (guest sees /dev/vda)
#                  -chardev socket,id=chrtpm,path=$TPMSOCK -tpmdev emulator,id=tpm0,chardev=chrtpm -device tpm-tis,tpmdev=tpm0
#                  OVMF UEFI firmware, serial console captured to log,
#                  hostfwd SSH port for interrogation.
#  3 interrogate - resolve BOTH VERIFY-ON-VM markers inside the live environment:
#                  (a) systemctl list-units + list-unit-files | grep -i subiquity
#                      -> record the EXACT unit name(s) that autostart the stock installer;
#                  (b) for tool in debootstrap sgdisk zpool cryptsetup dracut clevis:
#                      command -v $tool -> record present/MISSING.
#  4 install     - run the full uaa install against /dev/vda inside the VM
#                  (uaa local-install with a VM-specific YAML config whose
#                  disk_device: /dev/vda; secrets are throwaway VM-only values,
#                  never REPLACE_AT_PLACE_TIME placeholders and never real ones).
#  5 boot-disk   - shut down; re-launch QEMU from disk.qcow2 (same swtpm state,
#                  no ISO); watch serial console.
#  6 assert      - inside the booted system assert, in order:
#                  cryptsetup status luks        -> "is active"  (LUKS unlocked)
#                  zpool list -H -o name         -> rpool AND bpool imported
#                  systemctl is-system-running --wait or
#                  systemctl is-active multi-user.target -> reached multi-user
#  7 report      - print the VERIFY-ON-VM REPORT block (see C2) + PASS/FAIL; exit 0
#                  only if stages 2-6 all passed.
```

Fail-closed semantics: any assertion timeout (defaults in bold: boot timeout
**600s**, install timeout **3600s**) is a FAIL, not a skip. Stage 3 findings are
report-only (a MISSING tool is recorded and FAILs the gate only if stage 4 then fails —
the report tells you why).

### C2. VERIFY-ON-VM report (stage 7 output, machine-greppable)

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

### C3. `docs/vm-validation.md`

Operator doc: prerequisites (Linux host — the server 172.16.2.30 or any amd64 Linux box;
macOS explicitly unsupported, no KVM), package list (`qemu-system-x86`, `swtpm`, `ovmf`),
invocation, how to read the report, and the gate statement (no hardware attempt / no
len-serv-003 wipe until PASS). Notes that runs on the server must not touch its live
services.

### C4. `LocalClient` unit tests (`src/network/local.rs`, `#[cfg(test)] mod tests`)

Test-only, additive; the mock seam for *callers* remains the `CommandExecutor` trait
(`src/network/executor.rs:11`) — these tests instead exercise the real implementation
with harmless commands (`LocalClient` runs real `bash -c`):

```rust
#[cfg(test)]
mod tests {
    // All commands are side-effect-free: echo/true/false/cp-on-tempfiles.
    #[tokio::test] async fn test_connect_is_noop_ok() {}                 // connect("x","y") -> Ok
    #[tokio::test] async fn test_execute_true_succeeds() {}              // execute("true") -> Ok
    #[tokio::test] async fn test_execute_false_returns_process_error() {}// execute("false") -> Err(ProcessError{exit_code:Some(1)})
    #[tokio::test] async fn test_execute_with_output_captures_stdout() {}// "echo hello" -> Ok("hello\n")
    #[tokio::test] async fn test_execute_with_output_failure_prefers_stderr() {}
    #[tokio::test] async fn test_execute_with_error_collection_nonzero_is_ok_tuple() {} // (exit,stdout,stderr), no Err on nonzero
    #[tokio::test] async fn test_check_silent_true_false() {}            // Ok(true)/Ok(false)
    #[tokio::test] async fn test_upload_download_copy_tempfiles() {}     // cp round-trip via tempdir
    #[test]        fn test_default_matches_new() {}                      // Default impl
}
```

Key semantic to pin: `execute_with_error_collection` returns `Ok((exit, stdout, stderr))`
even on nonzero exit (it collects, it does not fail), whereas `execute`/
`execute_with_output` return `Err(ProcessError)` on nonzero — the tests encode this
asymmetry so future refactors cannot silently flip it.

## Migration / integration

Nothing migrates. Both deliverables are additive: a new script + new doc, and a new
`#[cfg(test)]` module in an existing file (header version bump on `src/network/local.rs`
required). `scripts/build-installer-image.sh` is NOT edited by this workstream — its
markers are *resolved by evidence* from the harness report; acting on that evidence
(e.g. adjusting the mask list or baking tools into the overlay) is follow-up work
outside this plan.

## Files modified

| File | Change |
|---|---|
| `scripts/vm-validate.sh` | NEW — QEMU+swtpm end-to-end validation harness (TASK-01) |
| `docs/vm-validation.md` | NEW — operator doc: prerequisites, invocation, report reading, gate statement (TASK-01) |
| `src/network/local.rs` | ADD `#[cfg(test)] mod tests` (+ header version bump); no production code change (TASK-02) |

## Testing

| Test | Asserts |
|---|---|
| `bash -n scripts/vm-validate.sh` | script parses (repo gate for shell briefs) |
| `cargo test --lib --offline` | baseline 237 passed grows by the new LocalClient tests; 0 failed |
| VM run stage 3 | both VERIFY-ON-VM markers answered in the report |
| VM run stages 4–6 | install completes on `/dev/vda`; reboot: LUKS active, `rpool`+`bpool` imported, multi-user reached |
| `test_execute_false_returns_process_error` | fail path returns `ProcessError` with `exit_code: Some(1)` (fail-closed) |
| `test_execute_with_error_collection_nonzero_is_ok_tuple` | collection API stays non-failing on nonzero exit |

## Failure modes

- **No KVM on host** (container, nested-virt-less VM): stage 0 WARNs and falls back to
  TCG; timeouts are generous, but an operator on the server gets KVM and normal speed.
  macOS is refused outright at stage 0.
- **Harness run before installer-robustness/TASK-01 merges:** stage 4 fails in Phase 2
  (`mkfs` on nonexistent `/dev/vdap1`). This is expected and is exactly why the
  dependency is hard — the wave plan sequences the harness after the helper.
- **Stock-installer unit name differs on 26.04** (e.g. a new snap-wrapped unit): stage 3
  reports `verdict: GAP` with the exact unit name; the gate output is the actionable fix
  for `build-installer-image.sh`'s mask list.
- **Missing live-rootfs tool** (e.g. no `debootstrap`): stage 4 fails at debootstrap;
  the stage-3 report line already names the missing tool, so diagnosis is immediate.
- **swtpm state left running after abort:** the harness traps EXIT and kills its own
  swtpm/QEMU pids; state lives under `$WORKDIR` and is safe to `rm -rf`.
- **LocalClient tests on a box without `bash`:** not a real risk on target platforms
  (macOS dev + Linux CI both ship bash); tests use only POSIX-trivial commands.

## Rollback

- The harness and doc are net-new files: `git revert` of the TASK-01 commit removes them
  with zero blast radius; no production code references them.
- The LocalClient test module is `#[cfg(test)]`-only: reverting the TASK-02 commit
  returns `src/network/local.rs` to its untested state; shipping binaries are unaffected
  either way.
- Nothing here is flag-gated because nothing here changes runtime behavior — the only
  "behavior" introduced is the *process gate* (Decision 5), which stays in force until
  the operator sees `GATE: PASS`.

## Open questions (resolved — recorded for the plan)

1. ~~Reuse `src/utils/vm.rs` VmManager for the harness?~~ → No — locked Decision 1;
   greenfield bash script.
2. ~~Can the harness run on the macOS dev machine?~~ → No — Linux host required (no KVM
   on macOS); locked Decision 2, documented in `docs/vm-validation.md`.
3. ~~Mock inside LocalClient or use real commands?~~ → Real harmless commands; the seam
   stays at `CommandExecutor` (locked Decision 6).
4. ~~Does the harness edit build-installer-image.sh to resolve the markers?~~ → No — it
   *reports* the answers; acting on them is follow-up outside this workstream.
