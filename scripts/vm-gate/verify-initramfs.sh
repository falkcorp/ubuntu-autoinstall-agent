#!/usr/bin/env bash
# file: scripts/vm-gate/verify-initramfs.sh
# version: 1.1.0
# guid: 7d267727-2c1d-4a5e-8ce6-3007511f3de4
# last-edited: 2026-08-03
#
# PRE-REBOOT VERIFICATION. Given an initramfs image, assert that everything
# the configured clevis unlock pins need is actually INSIDE it — before
# anybody reboots a host that then hangs at a passphrase prompt with no
# console.
#
# THE BUG THIS EXISTS TO CATCH (measured on the live fleet, 2026-08):
#   the production initramfs contains clevis + clevis-decrypt-tang +
#   clevis-decrypt-sss + libtss2, and ZERO `clevis-decrypt-tpm2`, because the
#   `clevis-tpm2` package was never installed. Every static check that only
#   looked for "clevis" passed. The tpm2 share silently could not be used.
#   The pkcs11 pin has exactly the same failure mode, plus one more: the
#   PKCS#11 provider .so can be absent even when clevis-decrypt-pkcs11 is
#   present, and clevis-decrypt-pkcs11 without a provider library can decrypt
#   nothing.
#
# READ-ONLY. This script opens the image and nothing else: no root needed, no
# device touched, no file written outside /tmp scratch used by the extractor.
#
# Usage:
#   ./scripts/vm-gate/verify-initramfs.sh --image /boot/initrd.img-$(uname -r) \
#       [--pin all|tpm2|pkcs11|tang] [--provider opensc|softhsm|any] [--quiet]
#
# --pin tpm2     what the fleet needs TODAY (use this on len-serv-001/002/U1)
# --pin pkcs11   the new break-glass Slot B path
# --pin all      everything (default) — what the VM gate asserts
#
# --provider selects which PKCS#11 module counts as present:
#   opensc  (default for real hardware) opensc-pkcs11.so — the YubiKey path
#   softhsm libsofthsm2.so — gate-only; a real host must NOT ship this
#   any     either one satisfies the check
#
# Exit 0 only when every required item is present. Exit 1 lists EVERY missing
# item (not just the first), so one run tells the operator the whole gap.

set -euo pipefail

die() { echo "ERROR: $*" >&2; exit 1; }

IMAGE=""
PIN="all"
PROVIDER="opensc"
QUIET="0"

while [ $# -gt 0 ]; do
  case "$1" in
    --image)    IMAGE="${2:?--image needs a path}"; shift 2 ;;
    --pin)      PIN="${2:?--pin needs all|tpm2|pkcs11|tang}"; shift 2 ;;
    --provider) PROVIDER="${2:?--provider needs opensc|softhsm|any}"; shift 2 ;;
    --quiet)    QUIET="1"; shift ;;
    -h|--help)  grep '^#' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    -*)         die "unknown flag: $1" ;;
    *)          die "unexpected positional arg: $1 (use --image/--pin/... flags)" ;;
  esac
done

# =========================================================================
# GUARD — this is read-only, but it must still refuse to be aimed at a real
# device or a fleet host, so an operator cannot typo their way into
# `--image /dev/nvme0n1`.
# =========================================================================
for a in "$IMAGE" "$PIN" "$PROVIDER"; do
  case "$a" in
    *172.16.*)
      die "GUARD: '$a' names a 172.16.x.x host. This script reads a LOCAL initramfs file; copy the image off the host and inspect it here." ;;
    /dev/*)
      die "GUARD: '$a' names a device node. --image must be a regular initramfs FILE." ;;
  esac
done

[ -n "$IMAGE" ] || die "--image is required"
[ -f "$IMAGE" ] || die "--image not found or not a regular file: $IMAGE"
case "$PIN" in all|tpm2|pkcs11|tang) ;; *) die "--pin must be all|tpm2|pkcs11|tang (got '$PIN')" ;; esac
case "$PROVIDER" in opensc|softhsm|any) ;; *) die "--provider must be opensc|softhsm|any (got '$PROVIDER')" ;; esac

log() { [ "$QUIET" = "1" ] || echo "==> $*" >&2; }

# =========================================================================
# Listing. This repo ships a dracut module (dracut/91uaa-keystore-wait), so
# lsinitrd is preferred and is also the only lister that can report the
# dracut MODULE list (as opposed to just file paths). Fall back to
# lsinitramfs (initramfs-tools) and then to a raw cpio walk.
# =========================================================================
LISTING="$(mktemp)"
MODULES="$(mktemp)"
# shellcheck disable=SC2329 # invoked indirectly via `trap cleanup EXIT`
cleanup() { rm -f "$LISTING" "$MODULES"; }
trap cleanup EXIT

LISTER=""
if command -v lsinitrd >/dev/null && lsinitrd "$IMAGE" >"$LISTING" 2>/dev/null && [ -s "$LISTING" ]; then
  LISTER="lsinitrd"
  # `lsinitrd -m` prints the dracut module names, which is how the
  # 50clevis-pin-pkcs11 module presence is checked properly (the module can
  # be listed even though its name never appears as a file path).
  lsinitrd -m "$IMAGE" >"$MODULES" 2>/dev/null || : >"$MODULES"
elif command -v lsinitramfs >/dev/null && lsinitramfs "$IMAGE" >"$LISTING" 2>/dev/null && [ -s "$LISTING" ]; then
  LISTER="lsinitramfs"
  : >"$MODULES"
else
  # Raw fallback: strip the compression, walk the cpio.
  LISTER="cpio-fallback"
  if zstd -dc "$IMAGE" 2>/dev/null | cpio -t 2>/dev/null >"$LISTING" && [ -s "$LISTING" ]; then :
  elif zcat "$IMAGE" 2>/dev/null | cpio -t 2>/dev/null >"$LISTING" && [ -s "$LISTING" ]; then :
  elif xzcat "$IMAGE" 2>/dev/null | cpio -t 2>/dev/null >"$LISTING" && [ -s "$LISTING" ]; then :
  elif lz4cat "$IMAGE" 2>/dev/null | cpio -t 2>/dev/null >"$LISTING" && [ -s "$LISTING" ]; then :
  else
    die "could not list '$IMAGE' with lsinitrd, lsinitramfs, or any cpio fallback. Install dracut-core (lsinitrd) — this is a FAIL, not a reason to reboot anyway."
  fi
  : >"$MODULES"
fi
log "listed ${IMAGE} via ${LISTER} ($(wc -l <"$LISTING") entries)"

# =========================================================================
# Requirement table.
#
# Each entry:  <kind>|<needle>|<why it matters>
#   file    — a basename that must appear somewhere in the listing
#   module  — a dracut module name (checked against `lsinitrd -m`, and
#             against the listing as a fallback for non-dracut images)
#   anyof   — colon-separated alternatives, at least one must be present
# =========================================================================
REQS=()

# Common to every clevis pin. Without these, no pin decrypts anything.
REQS+=("file|clevis|the clevis driver itself")
REQS+=("file|clevis-luks-unlock|the LUKS unlock entrypoint")
REQS+=("file|jose|JOSE/JWE crypto used by every clevis pin")
REQS+=("file|clevis-decrypt-sss|the SSS pin — the whole Slot A policy is an sss")

if [ "$PIN" = "all" ] || [ "$PIN" = "tang" ] || [ "$PIN" = "tpm2" ]; then
  REQS+=("file|clevis-decrypt-tang|the Tang pin")
  REQS+=("file|curl|Tang recovery is an HTTP POST; clevis-decrypt-tang shells out to curl")
fi

if [ "$PIN" = "all" ] || [ "$PIN" = "tpm2" ]; then
  # THE MEASURED FLEET BUG. Do not soften this check.
  REQS+=("file|clevis-decrypt-tpm2|the tpm2 pin — ABSENT ON THE LIVE FLEET because the clevis-tpm2 package is not installed")
  REQS+=("file|clevis-pin-tpm2|dracut/initramfs hook for the tpm2 pin")
  REQS+=("anyof|tpm2_unseal:tpm2_createprimary:tpm2_load|tpm2-tools binaries clevis-decrypt-tpm2 shells out to")
  REQS+=("anyof|libtss2-tcti-device.so:libtss2-tcti-device.so.0|TCTI to reach /dev/tpmrm0 from the initramfs")
fi

if [ "$PIN" = "all" ] || [ "$PIN" = "pkcs11" ]; then
  REQS+=("module|50clevis-pin-pkcs11|the dracut module that installs the pkcs11 pin into the initramfs")
  REQS+=("file|clevis-decrypt-pkcs11|the pkcs11 pin decrypt helper")
  REQS+=("file|pkcs11-tool|clevis-decrypt-pkcs11 shells out to pkcs11-tool --login --decrypt")
  REQS+=("file|clevis-luks-pkcs11-askpass|the boot-time askpass agent that writes /run/systemd/clevis-pkcs11.pin")
  case "$PROVIDER" in
    opensc)  REQS+=("file|opensc-pkcs11.so|the PKCS#11 provider module (YubiKey path). clevis-decrypt-pkcs11 with no provider .so can decrypt NOTHING — same bug class as the missing clevis-decrypt-tpm2") ;;
    softhsm) REQS+=("file|libsofthsm2.so|the SoftHSM provider module (GATE IMAGES ONLY — a real host must not ship this)") ;;
    any)     REQS+=("anyof|opensc-pkcs11.so:libsofthsm2.so|a PKCS#11 provider module of some kind") ;;
  esac
  # A real YubiKey is reached through pcscd/CCID. SoftHSM bypasses that
  # entirely, which is why the VM gate can never prove this half — but the
  # pieces still have to be in the image for a hardware token to work.
  REQS+=("anyof|pcscd:libpcsclite.so:libpcsclite.so.1|the pcscd/CCID smartcard stack a real YubiKey needs (SoftHSM bypasses it, so the VM gate cannot exercise it)")
  # MEASURED 2026-08-03 in the root-LUKS gate VM: Ubuntu's libpcsclite.so.1 is a
  # dlopen shim that loads libpcsclite_real.so.1 at runtime, and dracut's
  # 50clevis-pin-pkcs11 installs only the shim. The initramfs console printed
  #   loading "libpcsclite_real.so.1" failed: ... No such file or directory
  #   No slots.
  # so pcscd could enumerate no readers. SoftHSM does not go through pcsc-lite,
  # so the VM gate cannot prove the YubiKey path either way — but a smartcard
  # stack that cannot load its own backend cannot see a token, and this is the
  # cheapest place to catch it before a host with no remote power reboots.
  REQS+=("file|libpcsclite_real.so.1|the REAL pcsc-lite backend behind Ubuntu's libpcsclite.so.1 dlopen shim; dracut's 50clevis-pin-pkcs11 does NOT install it, so add it via install_items or a real YubiKey is invisible in the initramfs")
fi

# =========================================================================
# Evaluate. Collect EVERY miss, not just the first.
# =========================================================================
have_file() { grep -qE "(^|/)$(printf '%s' "$1" | sed 's/[.[\*^$]/\\&/g')(\$|[[:space:]])" "$LISTING"; }
# `lsinitrd -m` prints dracut module names WITHOUT their numeric directory
# prefix: the module that lives in /usr/lib/dracut/modules.d/50clevis-pin-pkcs11
# is listed as `clevis-pin-pkcs11`. Matching the on-disk directory name with
# -qx therefore never hit, and this check printed "MISSING ... DO NOT REBOOT"
# for an initramfs that then booted and unlocked fine (measured 2026-08-03).
#
# Accept either spelling by stripping a leading run of digits from BOTH the
# needle and each listed module before comparing, and anchor the comparison so
# `clevis` never matches `clevis-pin-sss`.
#
# The old fallback `|| grep -q -- "$1" "$LISTING"` was an unanchored substring
# search over the whole file listing, which is a false POSITIVE generator: the
# string "clevis-pin-pkcs11" appears in the listing as part of hook paths even
# when the module itself was not enabled. The listing fallback is kept only for
# images whose `lsinitrd -m` output is unavailable, and it is anchored to a
# modules.d path component.
have_module() {
  local needle="${1#"${1%%[!0-9]*}"}"   # strip any leading digits
  if [ -s "$MODULES" ]; then
    local m
    while IFS= read -r m; do
      m="${m#"${m%%[!0-9]*}"}"
      [ "$m" = "$needle" ] && return 0
    done < "$MODULES"
    return 1
  fi
  grep -qE "modules\.d/[0-9]*${needle}(/|\$)" "$LISTING"
}

MISSING=0
for req in "${REQS[@]}"; do
  kind="${req%%|*}"; rest="${req#*|}"
  needle="${rest%%|*}"; why="${rest#*|}"
  ok=1
  case "$kind" in
    file)   have_file "$needle" || ok=0 ;;
    module) have_module "$needle" || ok=0 ;;
    anyof)
      ok=0
      IFS=':' read -r -a alts <<<"$needle"
      for alt in "${alts[@]}"; do
        if have_file "$alt"; then ok=1; break; fi
      done
      ;;
  esac
  if [ "$ok" = "1" ]; then
    [ "$QUIET" = "1" ] || printf 'PRESENT %-8s %s\n' "$kind" "$needle"
  else
    printf 'MISSING %-8s %-32s -- %s\n' "$kind" "$needle" "$why"
    MISSING=$((MISSING + 1))
  fi
done

echo "---- initramfs verification ----"
echo "image:    ${IMAGE}"
echo "lister:   ${LISTER}"
echo "pin-set:  ${PIN}   provider: ${PROVIDER}"
if [ "$MISSING" = "0" ]; then
  echo "INITRAMFS: OK (all ${#REQS[@]} required items present)"
  echo "REMINDER: contents present != boot proven. This check cannot tell you"
  echo "          whether the askpass socket unit is ENABLED, nor whether the"
  echo "          policy actually unlocks. Run scripts/vm-gate/pkcs11-clevis-gate.sh"
  echo "          for the policy, and scripts/vm-validate.sh for a real boot."
  exit 0
fi
echo "INITRAMFS: MISSING ${MISSING} of ${#REQS[@]} required items — DO NOT REBOOT"
echo "           this host until they are installed and the initramfs regenerated."
exit 1
