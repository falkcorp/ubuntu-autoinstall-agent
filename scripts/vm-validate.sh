#!/usr/bin/env bash
# file: scripts/vm-validate.sh
# version: 1.0.0
# guid: 83274dbf-b287-4567-b4d8-2f31fa604974
# last-edited: 2026-07-09
#
# QEMU+swtpm VM validation gate. THIS SCRIPT PASSING IS THE GATE — no hardware
# attempt or len-serv-003 wipe before it passes.
#
# Boots the remastered SSH-ready ISO (scripts/make-ssh-ready-iso.sh output) in
# QEMU with OVMF UEFI firmware, a virtio qcow2 target disk (the guest sees
# /dev/vda — end-to-end proof of the partition-suffix helper: vda's partitions
# are vda1..vda4, no `p` infix, unlike /dev/sdap-style paths), and a swtpm
# socket TPM2 device (tpm-tis). It copies the musl `uaa` binary + a VM test
# config into the live session over SSH, runs the full `uaa install --config`
# there, reboots from the installed disk, and asserts LUKS unlock + rpool/bpool
# import + multi-user. Stage 3 additionally interrogates the live environment
# to resolve BOTH VERIFY-ON-VM markers in scripts/build-installer-image.sh
# (the stock-installer autostart unit, and presence of the live-rootfs install
# tools) and prints the answers in a machine-greppable report at the end.
#
# LINUX HOST ONLY: macOS has no KVM. Run this on the server (172.16.2.30) or
# any amd64 Linux box. Never against a physical/hardware install target — the
# ONLY disk this script ever touches is the qcow2 image it creates itself in
# --workdir.
#
# Usage:
#   sudo ./scripts/vm-validate.sh --iso <ssh-ready.iso> \
#       --agent target/x86_64-unknown-linux-musl/release/ubuntu-autoinstall-agent \
#       [--config examples/configs/install/vm-test.yaml] \
#       [--workdir ./vm-validate-work] [--disk-size 40G] [--ssh-port 10022] \
#       [--boot-timeout 600] [--install-timeout 3600]
#
# Requires: qemu-system-x86_64, swtpm, qemu-img, ssh, scp, an ovmf package
# (OVMF_CODE*.fd under /usr/share/OVMF or /usr/share/qemu), and either
# sshpass (for the live-session password login) or an SSH agent holding the
# operator private key matching installer-image/nocloud/user-data. socat is
# optional (best-effort LUKS-passphrase auto-answer on the disk-boot serial
# console — see stage 5 below).
#
# See docs/vm-validation.md for the full operator walkthrough.

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${HERE}/.." && pwd)"

die() { echo "ERROR: $*" >&2; exit 1; }

# --- defaults -----------------------------------------------------------
ISO=""
AGENT=""
CONFIG="${REPO_ROOT}/examples/configs/install/vm-test.yaml"
WORKDIR="./vm-validate-work"
DISK_SIZE="40G"
SSH_PORT="10022"
BOOT_TIMEOUT="600"
INSTALL_TIMEOUT="3600"
SSH_USER="ubuntu-server"
SSH_LIVE_PASSWORD="default"

# --- arg parsing (mirrors scripts/make-ssh-ready-iso.sh) ----------------
while [ $# -gt 0 ]; do
  case "$1" in
    --iso)              ISO="${2:?--iso needs a path}"; shift 2 ;;
    --agent)             AGENT="${2:?--agent needs a path}"; shift 2 ;;
    --config)            CONFIG="${2:?--config needs a path}"; shift 2 ;;
    --workdir)            WORKDIR="${2:?--workdir needs a dir}"; shift 2 ;;
    --disk-size)          DISK_SIZE="${2:?--disk-size needs a size, e.g. 40G}"; shift 2 ;;
    --ssh-port)           SSH_PORT="${2:?--ssh-port needs a port}"; shift 2 ;;
    --boot-timeout)       BOOT_TIMEOUT="${2:?--boot-timeout needs seconds}"; shift 2 ;;
    --install-timeout)    INSTALL_TIMEOUT="${2:?--install-timeout needs seconds}"; shift 2 ;;
    -h|--help)            grep '^#' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    -*)                   die "unknown flag: $1" ;;
    *)                    die "unexpected positional arg: $1 (use --iso/--agent/... flags)" ;;
  esac
done

[ -n "$ISO" ]   || die "--iso is required"
[ -n "$AGENT" ] || die "--agent is required"
[ -f "$ISO" ]   || die "--iso not found: $ISO"
[ -f "$AGENT" ] || die "--agent not found: $AGENT"
[ -f "$CONFIG" ] || die "--config not found: $CONFIG"

mkdir -p "$WORKDIR/logs" "$WORKDIR/tpmstate"
WORKDIR="$(cd "$WORKDIR" && pwd)"

# --- state tracked for cleanup + the final report -----------------------
QEMU_PID=""
SWTPM_PID=""
SERIAL_READER_PID=""
SERIAL_INJECT_PID=""
FIRST_FAILING_STAGE=""
OBSERVED_UNITS=""
MARKER72_VERDICT=""
declare -A TOOL_STATUS

stage_echo() { echo "==> stage $1 $2"; }

# Never `pkill` by name (the host may run other VMs) — only kill pids this
# harness itself started.
# shellcheck disable=SC2329 # invoked indirectly via `trap cleanup EXIT` below
cleanup() {
  local ec=$?
  for pid in "$SERIAL_INJECT_PID" "$SERIAL_READER_PID" "$QEMU_PID" "$SWTPM_PID"; do
    if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
      kill "$pid" 2>/dev/null || true
      wait "$pid" 2>/dev/null || true
    fi
  done
  exit "$ec"
}
trap cleanup EXIT

print_report() {
  local gate="$1"
  {
    echo "==== VERIFY-ON-VM REPORT ===="
    echo "marker build-installer-image.sh:72 (stock-installer autostart unit):"
    echo "  observed-units: ${OBSERVED_UNITS:-UNKNOWN (stage 3 not reached)}"
    echo "  masked-by-build-script: subiquity-server.service serial-subiquity@.service snap.subiquity.subiquity-server.service"
    echo "  verdict: ${MARKER72_VERDICT:-UNKNOWN (stage 3 not reached)}"
    echo "marker build-installer-image.sh:81 (live-rootfs tools):"
    for tool in debootstrap sgdisk zpool cryptsetup dracut clevis; do
      printf '  %-12s %s\n' "${tool}:" "${TOOL_STATUS[$tool]:-UNKNOWN}"
    done
    if [ "$gate" = "PASS" ]; then
      echo "GATE: PASS"
    else
      echo "GATE: FAIL (${FIRST_FAILING_STAGE:-unknown stage})"
    fi
    echo "============================="
  } | tee -a "$WORKDIR/logs/07-report.log"
}

fail_stage() {
  local stage="$1" msg="$2"
  FIRST_FAILING_STAGE="stage ${stage}: ${msg}"
  echo "ERROR: stage ${stage} failed: ${msg}" >&2
  echo "See logs under: ${WORKDIR}/logs/" >&2
  print_report "FAIL"
  exit 1
}

# --- SSH/SCP helpers ------------------------------------------------------
SSH_OPTS=(-p "$SSH_PORT" -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o ConnectTimeout=5)

# ssh_run <timeout-seconds|0> <remote-user> <remote-command>
# Password auth (sshpass) is only used for the live-session `ubuntu-server`
# user; any other user (e.g. `root` on the installed target) is key-only.
ssh_run() {
  local tmo="$1" user="$2" cmd="$3"
  local base=(ssh "${SSH_OPTS[@]}" "${user}@127.0.0.1")
  if [ "$HAVE_SSHPASS" = 1 ] && [ "$user" = "$SSH_USER" ]; then
    base=(sshpass -p "$SSH_LIVE_PASSWORD" "${base[@]}")
  fi
  if [ "$tmo" -gt 0 ]; then
    timeout "$tmo" "${base[@]}" "$cmd"
  else
    "${base[@]}" "$cmd"
  fi
}

# scp_run <local-src> <remote-dst> [remote-user]
scp_run() {
  local src="$1" dst="$2" user="${3:-$SSH_USER}"
  local base=(scp -P "$SSH_PORT" -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null)
  if [ "$HAVE_SSHPASS" = 1 ] && [ "$user" = "$SSH_USER" ]; then
    base=(sshpass -p "$SSH_LIVE_PASSWORD" "${base[@]}")
  fi
  "${base[@]}" "$src" "${user}@127.0.0.1:${dst}"
}

# wait_for_ssh <user> <overall-timeout-seconds> <logfile>
wait_for_ssh() {
  local user="$1" overall="$2" logfile="$3" waited=0
  while [ "$waited" -lt "$overall" ]; do
    if ssh_run 5 "$user" true >>"$logfile" 2>&1; then
      return 0
    fi
    sleep 5
    waited=$((waited + 5))
  done
  return 1
}

# =========================================================================
# Stage 0: preflight
# =========================================================================
stage_echo 0 preflight
PRE_LOG="$WORKDIR/logs/00-preflight.log"
: > "$PRE_LOG"

[ "$(uname -s)" = "Linux" ] || die "Linux host required (no KVM on macOS) — run on the server 172.16.2.30 or any amd64 Linux box"

for bin in qemu-system-x86_64 swtpm qemu-img ssh scp; do
  command -v "$bin" >/dev/null 2>&1 || die "missing required tool '$bin' — install it (e.g. qemu-system-x86, swtpm, openssh-client packages)"
done

HAVE_SSHPASS=0
if command -v sshpass >/dev/null 2>&1; then
  HAVE_SSHPASS=1
else
  echo "WARN: sshpass not found — falling back to key-only SSH auth for the live-session login (needs the operator key loaded, e.g. in an ssh-agent)" | tee -a "$PRE_LOG" >&2
fi

HAVE_SOCAT=0
command -v socat >/dev/null 2>&1 && HAVE_SOCAT=1
if [ "$HAVE_SOCAT" = 0 ]; then
  echo "WARN: socat not found — cannot auto-answer a LUKS passphrase prompt on the disk-boot serial console (stage 5); if the reboot hangs there, install socat or ensure TPM2/Clevis auto-unlock is configured" | tee -a "$PRE_LOG" >&2
fi

OVMF_CODE=""
OVMF_VARS_SRC=""
for d in /usr/share/OVMF /usr/share/qemu /usr/share/edk2/ovmf /usr/share/edk2-ovmf; do
  for f in OVMF_CODE_4M.fd OVMF_CODE.fd; do
    [ -z "$OVMF_CODE" ] && [ -f "${d}/${f}" ] && OVMF_CODE="${d}/${f}"
  done
  for f in OVMF_VARS_4M.fd OVMF_VARS.fd; do
    [ -z "$OVMF_VARS_SRC" ] && [ -f "${d}/${f}" ] && OVMF_VARS_SRC="${d}/${f}"
  done
done
[ -n "$OVMF_CODE" ] || die "OVMF firmware (OVMF_CODE*.fd) not found under /usr/share/OVMF or /usr/share/qemu — install the 'ovmf' package"

KVM_OK=0
if [ -r /dev/kvm ] && [ -w /dev/kvm ]; then
  KVM_OK=1
else
  echo "WARN: /dev/kvm not writable — falling back to TCG software emulation (slow); this is a WARN, not a failure" | tee -a "$PRE_LOG" >&2
fi

# Placeholders must never reach an install.
grep -q "REPLACE_AT_PLACE_TIME" "$CONFIG" && die "--config $CONFIG still contains REPLACE_AT_PLACE_TIME placeholders — never install with unsubstituted secrets"

echo "preflight OK: kvm=${KVM_OK} sshpass=${HAVE_SSHPASS} socat=${HAVE_SOCAT} ovmf_code=${OVMF_CODE} ovmf_vars=${OVMF_VARS_SRC:-none}" | tee -a "$PRE_LOG"

# =========================================================================
# Stage 1: workspace
# =========================================================================
stage_echo 1 workspace
WS_LOG="$WORKDIR/logs/01-workspace.log"
: > "$WS_LOG"

DISK_IMG="$WORKDIR/disk.qcow2"
qemu-img create -f qcow2 "$DISK_IMG" "$DISK_SIZE" >>"$WS_LOG" 2>&1

if [ -n "$OVMF_VARS_SRC" ]; then
  cp "$OVMF_VARS_SRC" "$WORKDIR/OVMF_VARS.fd"
  FIRMWARE_ARGS=(
    -drive "if=pflash,format=raw,readonly=on,file=${OVMF_CODE}"
    -drive "if=pflash,format=raw,file=${WORKDIR}/OVMF_VARS.fd"
  )
else
  FIRMWARE_ARGS=(-bios "$OVMF_CODE")
fi

SWTPM_SOCK="$WORKDIR/swtpm.sock"
SWTPM_PIDFILE="$WORKDIR/swtpm.pid"
swtpm socket \
  --tpmstate "dir=${WORKDIR}/tpmstate" \
  --ctrl "type=unixio,path=${SWTPM_SOCK}" \
  --tpm2 --daemon --pid "file=${SWTPM_PIDFILE}" >>"$WS_LOG" 2>&1

# Read the pidfile as soon as it appears (before waiting on the socket) so
# `cleanup` can always kill our own swtpm daemon, even if the socket-wait
# below times out and fail_stage exits early.
PIDFILE_WAITED=0
while [ ! -f "$SWTPM_PIDFILE" ]; do
  sleep 1
  PIDFILE_WAITED=$((PIDFILE_WAITED + 1))
  [ "$PIDFILE_WAITED" -ge 20 ] && fail_stage 1 "swtpm did not write a pid file at $SWTPM_PIDFILE"
done
SWTPM_PID="$(cat "$SWTPM_PIDFILE" 2>/dev/null || true)"
[ -n "$SWTPM_PID" ] || fail_stage 1 "swtpm pid file at $SWTPM_PIDFILE was empty"

SWTPM_WAITED=0
while [ ! -S "$SWTPM_SOCK" ]; do
  sleep 1
  SWTPM_WAITED=$((SWTPM_WAITED + 1))
  [ "$SWTPM_WAITED" -ge 20 ] && fail_stage 1 "swtpm socket never appeared at $SWTPM_SOCK"
done

echo "workspace OK: disk=$DISK_IMG swtpm_pid=$SWTPM_PID swtpm_sock=$SWTPM_SOCK" | tee -a "$WS_LOG"

# =========================================================================
# Stage 2: boot-iso
# =========================================================================
stage_echo 2 boot-iso
BOOT_ISO_LOG="$WORKDIR/logs/02-boot-iso.log"
: > "$BOOT_ISO_LOG"

QEMU_ISO_ARGS=(
  -m 4096
  -smp 2
  "${FIRMWARE_ARGS[@]}"
  -drive "file=${DISK_IMG},if=virtio,format=qcow2"
  -cdrom "$ISO"
  -boot "order=dc"
  -chardev "socket,id=chrtpm,path=${SWTPM_SOCK}"
  -tpmdev "emulator,id=tpm0,chardev=chrtpm"
  -device "tpm-tis,tpmdev=tpm0"
  -netdev "user,id=n0,hostfwd=tcp::${SSH_PORT}-:22"
  -device "virtio-net-pci,netdev=n0"
  -serial "file:${WORKDIR}/logs/02-boot-iso-serial.log"
  -display none
  -no-reboot
)
[ "$KVM_OK" = 1 ] && QEMU_ISO_ARGS+=(-enable-kvm -cpu host)

qemu-system-x86_64 "${QEMU_ISO_ARGS[@]}" >>"$BOOT_ISO_LOG" 2>&1 &
QEMU_PID=$!
echo "qemu (iso boot) pid=$QEMU_PID" | tee -a "$BOOT_ISO_LOG"

if ! wait_for_ssh "$SSH_USER" "$BOOT_TIMEOUT" "$BOOT_ISO_LOG"; then
  fail_stage 2 "SSH to the live installer session did not come up within ${BOOT_TIMEOUT}s (see $BOOT_ISO_LOG and 02-boot-iso-serial.log)"
fi
kill -0 "$QEMU_PID" 2>/dev/null || fail_stage 2 "qemu (iso boot) exited unexpectedly — see $BOOT_ISO_LOG"
echo "SSH reachable on the live session" | tee -a "$BOOT_ISO_LOG"

# =========================================================================
# Stage 3: interrogate (resolves BOTH VERIFY-ON-VM markers)
# =========================================================================
stage_echo 3 interrogate
INTERROGATE_LOG="$WORKDIR/logs/03-interrogate.log"
: > "$INTERROGATE_LOG"

MASKED_UNITS="subiquity-server.service serial-subiquity@.service snap.subiquity.subiquity-server.service"

UNITS_RAW="$(ssh_run 30 "$SSH_USER" \
  "systemctl list-units --all --no-legend '*subiquity*' 2>/dev/null; systemctl list-unit-files --no-legend '*subiquity*' 2>/dev/null" \
  2>>"$INTERROGATE_LOG" || true)"
echo "$UNITS_RAW" >>"$INTERROGATE_LOG"

OBSERVED_UNITS="$(echo "$UNITS_RAW" | awk '{print $1}' | sort -u | tr '\n' ' ' | sed -E 's/[[:space:]]+$//')"
[ -n "$OBSERVED_UNITS" ] || OBSERVED_UNITS="NONE"

MARKER72_VERDICT="COVERED"
if [ "$OBSERVED_UNITS" != "NONE" ]; then
  for u in $OBSERVED_UNITS; do
    case " $MASKED_UNITS " in
      *" $u "*) ;;
      *) MARKER72_VERDICT="GAP (unit $u not in mask list)" ;;
    esac
  done
fi
echo "observed-units: $OBSERVED_UNITS -> verdict: $MARKER72_VERDICT" >>"$INTERROGATE_LOG"

for tool in debootstrap sgdisk zpool cryptsetup dracut clevis; do
  if ssh_run 15 "$SSH_USER" "command -v $tool >/dev/null 2>&1" >>"$INTERROGATE_LOG" 2>&1; then
    TOOL_STATUS[$tool]="present"
  else
    TOOL_STATUS[$tool]="MISSING"
  fi
  echo "  $tool: ${TOOL_STATUS[$tool]}" >>"$INTERROGATE_LOG"
done
# Stage-3 findings are report-only: a MISSING tool here does not fail the
# gate — stage 4 will fail on it (if it actually blocks the install), and the
# stage-7 report explains why.

# =========================================================================
# Stage 4: install
# =========================================================================
stage_echo 4 install
INSTALL_LOG="$WORKDIR/logs/04-install.log"
: > "$INSTALL_LOG"

scp_run "$AGENT" "/tmp/uaa" >>"$INSTALL_LOG" 2>&1 || fail_stage 4 "scp of agent binary failed — see $INSTALL_LOG"
ssh_run 15 "$SSH_USER" "chmod +x /tmp/uaa" >>"$INSTALL_LOG" 2>&1 || fail_stage 4 "chmod +x /tmp/uaa over ssh failed"
scp_run "$CONFIG" "/tmp/vm-test.yaml" >>"$INSTALL_LOG" 2>&1 || fail_stage 4 "scp of --config failed — see $INSTALL_LOG"

# Deliberately NOT passing --hold-on-failure/--pause-after-storage: with both
# false, install_command -> local_install_command routes through
# perform_installation_with_options_and_pause, which itself short-circuits
# straight to perform_installation() (src/network/ssh_installer/installer.rs)
# — the variant whose run_phase! macro actually logs "✓ Phase completed:
# <label>" per phase and the final "🎉 Installation completed successfully"
# line the assertion below greps for. Passing either flag would route through
# the other (silent-on-success) macro and break these assertions.
INSTALL_EC=0
ssh_run "$INSTALL_TIMEOUT" "$SSH_USER" "sudo /tmp/uaa install --config /tmp/vm-test.yaml" >>"$INSTALL_LOG" 2>&1 || INSTALL_EC=$?

if [ "$INSTALL_EC" -eq 124 ]; then
  fail_stage 4 "uaa install timed out after ${INSTALL_TIMEOUT}s (never a skip — this is a FAIL) — see $INSTALL_LOG"
elif [ "$INSTALL_EC" -ne 0 ]; then
  fail_stage 4 "uaa install exited nonzero ($INSTALL_EC) — see $INSTALL_LOG"
fi

# 7 phases total: Phase 0..Phase 6 ("Phase 6: Final setup" is the last).
PHASE_COMPLETED_COUNT="$(grep -c "Phase completed:" "$INSTALL_LOG" || true)"
if ! grep -q "Phase 6: Final setup" "$INSTALL_LOG" \
   || ! grep -qi "Installation completed successfully" "$INSTALL_LOG" \
   || [ "${PHASE_COMPLETED_COUNT:-0}" -lt 7 ]; then
  fail_stage 4 "install log does not show all 7 phases completed (found ${PHASE_COMPLETED_COUNT:-0} 'Phase completed:' lines, or missing the Phase 6 / final-success line) — see $INSTALL_LOG"
fi
echo "install OK: exit=0, ${PHASE_COMPLETED_COUNT} phases completed" | tee -a "$INSTALL_LOG"

# =========================================================================
# Stage 5: boot-disk (reboot from the installed disk; same swtpm state)
# =========================================================================
stage_echo 5 boot-disk
BOOT_DISK_LOG="$WORKDIR/logs/05-boot-disk.log"
: > "$BOOT_DISK_LOG"

if ssh_run 20 "$SSH_USER" "sudo poweroff" >>"$BOOT_DISK_LOG" 2>&1; then
  echo "poweroff issued over ssh" >>"$BOOT_DISK_LOG"
else
  echo "poweroff ssh call did not return cleanly (expected — the connection drops); continuing to wait for qemu to exit" >>"$BOOT_DISK_LOG"
fi

WAITED=0
while kill -0 "$QEMU_PID" 2>/dev/null; do
  sleep 2
  WAITED=$((WAITED + 2))
  if [ "$WAITED" -ge "$BOOT_TIMEOUT" ]; then
    echo "qemu (iso boot) did not exit after poweroff within ${BOOT_TIMEOUT}s — forcing termination" >>"$BOOT_DISK_LOG"
    kill "$QEMU_PID" 2>/dev/null || true
    sleep 2
    kill -0 "$QEMU_PID" 2>/dev/null && kill -9 "$QEMU_PID" 2>/dev/null || true
    break
  fi
done
wait "$QEMU_PID" 2>/dev/null || true
QEMU_PID=""

# The first boot of the installed disk may pause at a LUKS passphrase prompt
# (TPM2+PIN auto-unlock is enrolled by a first-boot oneshot unit — see
# src/network/ssh_installer/config.rs `enroll_tpm2` docs — so it is not yet
# active on THIS first boot). Best-effort branch: if `socat` is available we
# wire the serial console to a unix socket instead of a plain log file and
# watch for a passphrase prompt, sending the throwaway `luks_key` from
# --config if one appears. Without socat this is skipped and documented as a
# known limitation (see docs/vm-validation.md); a genuine hang here is caught
# by the boot-timeout FAIL below regardless.
SERIAL_DISK_LOG="$WORKDIR/logs/05-boot-disk-serial.log"
: > "$SERIAL_DISK_LOG"
if [ "$HAVE_SOCAT" = 1 ]; then
  SERIAL_SOCK="$WORKDIR/serial-disk.sock"
  SERIAL_ARGS=(-chardev "socket,id=serial0,path=${SERIAL_SOCK},server=on,wait=off" -serial "chardev:serial0")
else
  SERIAL_ARGS=(-serial "file:${SERIAL_DISK_LOG}")
fi

QEMU_DISK_ARGS=(
  -m 4096
  -smp 2
  "${FIRMWARE_ARGS[@]}"
  -drive "file=${DISK_IMG},if=virtio,format=qcow2"
  -boot "order=c"
  -chardev "socket,id=chrtpm,path=${SWTPM_SOCK}"
  -tpmdev "emulator,id=tpm0,chardev=chrtpm"
  -device "tpm-tis,tpmdev=tpm0"
  -netdev "user,id=n0,hostfwd=tcp::${SSH_PORT}-:22"
  -device "virtio-net-pci,netdev=n0"
  "${SERIAL_ARGS[@]}"
  -display none
  -no-reboot
)
[ "$KVM_OK" = 1 ] && QEMU_DISK_ARGS+=(-enable-kvm -cpu host)

qemu-system-x86_64 "${QEMU_DISK_ARGS[@]}" >>"$BOOT_DISK_LOG" 2>&1 &
QEMU_PID=$!
echo "qemu (disk boot) pid=$QEMU_PID" | tee -a "$BOOT_DISK_LOG"

if [ "$HAVE_SOCAT" = 1 ]; then
  LUKS_KEY="$(grep -E '^luks_key:' "$CONFIG" | head -n1 | sed -E 's/^luks_key:[[:space:]]*//; s/^"(.*)"$/\1/')"
  sleep 2
  ( socat -u UNIX-CONNECT:"$SERIAL_SOCK" - >"$SERIAL_DISK_LOG" 2>/dev/null ) &
  SERIAL_READER_PID=$!
  (
    tries=$(( (BOOT_TIMEOUT / 2) + 1 ))
    i=0
    while [ "$i" -lt "$tries" ]; do
      if grep -qiE "enter passphrase|please unlock disk" "$SERIAL_DISK_LOG" 2>/dev/null; then
        printf '%s\n' "$LUKS_KEY" | socat -u - UNIX-CONNECT:"$SERIAL_SOCK" 2>/dev/null || true
        break
      fi
      sleep 2
      i=$((i + 1))
    done
  ) &
  SERIAL_INJECT_PID=$!
fi

if ! wait_for_ssh root "$BOOT_TIMEOUT" "$BOOT_DISK_LOG"; then
  fail_stage 5 "SSH to the installed disk (root) did not come up within ${BOOT_TIMEOUT}s (see $BOOT_DISK_LOG and $SERIAL_DISK_LOG — check for a stuck LUKS passphrase prompt)"
fi
kill -0 "$QEMU_PID" 2>/dev/null || fail_stage 5 "qemu (disk boot) exited unexpectedly — see $BOOT_DISK_LOG"
echo "SSH reachable on the installed disk (root)" | tee -a "$BOOT_DISK_LOG"

# =========================================================================
# Stage 6: assert (LUKS unlock + rpool/bpool import + multi-user)
# =========================================================================
stage_echo 6 assert
ASSERT_LOG="$WORKDIR/logs/06-assert.log"
: > "$ASSERT_LOG"

CRYPT_OUT="$(ssh_run 30 root "cryptsetup status luks" 2>&1 || true)"
echo "$CRYPT_OUT" >>"$ASSERT_LOG"
if echo "$CRYPT_OUT" | grep -q "is active"; then
  echo "PASS: LUKS unlocked (cryptsetup status luks: is active)" | tee -a "$ASSERT_LOG"
else
  echo "FAIL: LUKS not active — got: $CRYPT_OUT" | tee -a "$ASSERT_LOG"
  fail_stage 6 "cryptsetup status luks did not report 'is active'"
fi

ZPOOL_OUT="$(ssh_run 30 root "zpool list -H -o name" 2>&1 || true)"
echo "$ZPOOL_OUT" >>"$ASSERT_LOG"
if echo "$ZPOOL_OUT" | grep -qx "rpool" && echo "$ZPOOL_OUT" | grep -qx "bpool"; then
  echo "PASS: ZFS pools imported (rpool + bpool)" | tee -a "$ASSERT_LOG"
else
  echo "FAIL: rpool/bpool not both imported — got: $ZPOOL_OUT" | tee -a "$ASSERT_LOG"
  fail_stage 6 "zpool list did not show both rpool and bpool"
fi

MU_OUT="$(ssh_run 30 root "systemctl is-system-running --wait" 2>&1 || true)"
echo "$MU_OUT" >>"$ASSERT_LOG"
if echo "$MU_OUT" | grep -qE "running|degraded"; then
  echo "PASS: multi-user reached (systemctl is-system-running: $MU_OUT)" | tee -a "$ASSERT_LOG"
else
  MU_ALT="$(ssh_run 15 root "systemctl is-active multi-user.target" 2>&1 || true)"
  echo "$MU_ALT" >>"$ASSERT_LOG"
  if [ "$MU_ALT" = "active" ]; then
    echo "PASS: multi-user reached (multi-user.target active)" | tee -a "$ASSERT_LOG"
  else
    echo "FAIL: multi-user not reached — is-system-running: $MU_OUT, multi-user.target: $MU_ALT" | tee -a "$ASSERT_LOG"
    fail_stage 6 "system did not reach multi-user (is-system-running nor multi-user.target active)"
  fi
fi

# =========================================================================
# Stage 7: report — GATE: PASS only if stages 2-6 all passed above.
# =========================================================================
stage_echo 7 report
print_report "PASS"
exit 0
