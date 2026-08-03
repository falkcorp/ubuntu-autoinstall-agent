#!/usr/bin/env bash
# file: scripts/vm-gate/softhsm-setup.sh
# version: 1.0.0
# guid: eb74820d-afdb-4848-b720-03a95d3d7765
# last-edited: 2026-08-02
#
# Provision a THROWAWAY SoftHSM2 PKCS#11 token for the VM gate and print the
# PKCS#11 URI in the form clevis's pkcs11 pin expects.
#
# WHY: the fleet is adding a PKCS#11 (YubiKey PIV) clevis pin as a break-glass
# unlock factor, and there is no YubiKey available to test with. SoftHSM2 is a
# software PKCS#11 token, so the *mechanism* (bind reads the public key,
# unlock does a --login --decrypt) can be proven end to end without hardware.
# SoftHSM is NOT a YubiKey — see docs/vm-gate-pkcs11.md "What this gate cannot
# prove" before believing a green result means a real token will work.
#
# NON-DESTRUCTIVE BY CONSTRUCTION:
#   - Every token lives under --workdir via a generated SOFTHSM2_CONF. The
#     system token store (/var/lib/softhsm/tokens) is never touched, and this
#     script hard-fails if the caller's ambient SOFTHSM2_CONF points there.
#   - It creates no block devices, mounts nothing, and writes nothing outside
#     --workdir.
#   - It refuses to run if any argument names a 172.16.x.x host or a real
#     block device.
#
# Usage:
#   ./scripts/vm-gate/softhsm-setup.sh --workdir ./vm-gate-work \
#       [--label uaa-gate] [--pin 123456] [--so-pin 12345678] \
#       [--keypairs 1] [--key-type rsa:2048] [--module <libsofthsm2.so>] \
#       [--reset] [--quiet]
#
# Emits, on stdout, machine-greppable lines (the harness parses these):
#   SOFTHSM2_CONF=<path>
#   SOFTHSM_MODULE=<path to libsofthsm2.so>
#   TOKEN_LABEL=<label>
#   TOKEN_PIN=<pin>
#   PKCS11_URI_1=pkcs11:token=...;id=%01;object=...?module-path=...
#   PKCS11_URI_2=...            (only when --keypairs 2)
#
# With --keypairs 2 the token gets TWO keypairs (ids 01 and 02). That token is
# the fixture for the "first public key wins" clevis bug — see stage `gotcha`
# in scripts/vm-gate/pkcs11-clevis-gate.sh. A token backing a REAL Slot B URI
# must contain exactly ONE keypair.
#
# Requires: softhsm2-util, pkcs11-tool (opensc), and a libsofthsm2.so.
# Missing prerequisites are a hard failure, never a skip.

set -euo pipefail

die() { echo "ERROR: $*" >&2; exit 1; }

# --- defaults -----------------------------------------------------------
WORKDIR="./vm-gate-work"
LABEL="uaa-gate"
PIN="123456"
SO_PIN="12345678"
KEYPAIRS="1"
KEY_TYPE="rsa:2048"
MODULE=""
RESET="0"
QUIET="0"

while [ $# -gt 0 ]; do
  case "$1" in
    --workdir)   WORKDIR="${2:?--workdir needs a dir}"; shift 2 ;;
    --label)     LABEL="${2:?--label needs a token label}"; shift 2 ;;
    --pin)       PIN="${2:?--pin needs a value}"; shift 2 ;;
    --so-pin)    SO_PIN="${2:?--so-pin needs a value}"; shift 2 ;;
    --keypairs)  KEYPAIRS="${2:?--keypairs needs 1 or 2}"; shift 2 ;;
    --key-type)  KEY_TYPE="${2:?--key-type needs e.g. rsa:2048}"; shift 2 ;;
    --module)    MODULE="${2:?--module needs a path to libsofthsm2.so}"; shift 2 ;;
    --reset)     RESET="1"; shift ;;
    --quiet)     QUIET="1"; shift ;;
    -h|--help)   grep '^#' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    -*)          die "unknown flag: $1" ;;
    *)           die "unexpected positional arg: $1 (use --workdir/--label/... flags)" ;;
  esac
done

# =========================================================================
# GUARD — hard-fail before doing anything if this looks like real infra.
# A gate script that can be pointed at production is not a gate.
# =========================================================================
guard_no_real_infra() {
  local a
  for a in "$@"; do
    case "$a" in
      *172.16.*)
        die "GUARD: argument '$a' names a 172.16.x.x host. This harness is \
throwaway-only and must never touch the fleet." ;;
      /dev/sd*|/dev/nvme*|/dev/vd*|/dev/hd*|/dev/mapper/*|/dev/disk/*)
        die "GUARD: argument '$a' names a real block device. This script \
creates no devices and must never be pointed at one." ;;
    esac
  done
  case "${WORKDIR}" in
    /|/boot|/boot/*|/etc|/etc/*|/var/lib/softhsm|/var/lib/softhsm/*|/dev|/dev/*)
      die "GUARD: --workdir '${WORKDIR}' is a system path. Use a throwaway \
scratch directory." ;;
  esac
  # An ambient SOFTHSM2_CONF pointing at the system store would make
  # --init-token mutate shared state on the operator's server.
  if [ -n "${SOFTHSM2_CONF:-}" ]; then
    case "${SOFTHSM2_CONF}" in
      /etc/softhsm*|/usr/*|/var/lib/softhsm*)
        die "GUARD: ambient SOFTHSM2_CONF='${SOFTHSM2_CONF}' points at the \
system token store. Unset it; this script generates its own isolated config." ;;
    esac
  fi
}
guard_no_real_infra "$WORKDIR" "$LABEL" "$PIN" "$SO_PIN" "$KEY_TYPE" "$MODULE"

case "$KEYPAIRS" in 1|2) ;; *) die "--keypairs must be 1 or 2 (got '$KEYPAIRS')" ;; esac

log() { [ "$QUIET" = "1" ] || echo "==> $*" >&2; }

# --- prerequisites — FAIL, never skip ------------------------------------
command -v softhsm2-util >/dev/null || die "softhsm2-util not found (apt install softhsm2)"
command -v pkcs11-tool  >/dev/null || die "pkcs11-tool not found (apt install opensc)"

if [ -z "$MODULE" ]; then
  for cand in \
    /usr/lib/softhsm/libsofthsm2.so \
    /usr/lib/x86_64-linux-gnu/softhsm/libsofthsm2.so \
    /usr/lib/aarch64-linux-gnu/softhsm/libsofthsm2.so \
    /usr/lib64/pkcs11/libsofthsm2.so \
    /usr/local/lib/softhsm/libsofthsm2.so; do
    if [ -f "$cand" ]; then MODULE="$cand"; break; fi
  done
fi
[ -n "$MODULE" ] || die "libsofthsm2.so not found — pass --module <path>"
[ -f "$MODULE" ] || die "--module not found: $MODULE"

# --- isolated token store ------------------------------------------------
mkdir -p "$WORKDIR"
WORKDIR="$(cd "$WORKDIR" && pwd)"
TOKENDIR="${WORKDIR}/softhsm-tokens"
CONF="${WORKDIR}/softhsm2.conf"

if [ "$RESET" = "1" ] && [ -d "$TOKENDIR" ]; then
  # Safe: TOKENDIR is unconditionally a fixed child of the guarded WORKDIR.
  log "reset: removing ${TOKENDIR}"
  rm -rf "${TOKENDIR}"
fi
mkdir -p "$TOKENDIR"

cat >"$CONF" <<EOF
# Generated by scripts/vm-gate/softhsm-setup.sh — throwaway gate config.
directories.tokendir = ${TOKENDIR}
objectstore.backend = file
log.level = INFO
slots.removable = false
EOF
export SOFTHSM2_CONF="$CONF"

# --- token ---------------------------------------------------------------
# --free picks the first uninitialised slot. Re-initialising an existing label
# is not attempted: a stale token from an earlier run with different keys is a
# confounder, so tell the operator to --reset rather than silently reusing it.
if softhsm2-util --show-slots 2>/dev/null | grep -q "Label: *${LABEL}\b"; then
  die "token label '${LABEL}' already exists under ${TOKENDIR}. Re-run with \
--reset (or a different --label) — silently reusing a stale token would \
confound the gate."
fi

log "initialising SoftHSM2 token label='${LABEL}' in ${TOKENDIR}"
softhsm2-util --init-token --free --label "$LABEL" \
  --so-pin "$SO_PIN" --pin "$PIN" >&2

P11=(pkcs11-tool --module "$MODULE" --token-label "$LABEL")

# --- keypair(s) ----------------------------------------------------------
# clevis-encrypt-pkcs11 reads the PUBLIC key with
#   pkcs11-tool --read-object --type pubkey --id <id>
# and clevis-decrypt-pkcs11 does
#   pkcs11-tool --login --decrypt --input-file <enc> -p <PIN>
# so the keypair must be an RSA (or EC, mechanism permitting) keypair whose
# private half is usable for decryption on this token.
mk_keypair() {
  local id="$1" obj="$2"
  log "generating keypair id=${id} label=${obj} type=${KEY_TYPE}"
  "${P11[@]}" --login --pin "$PIN" --keypairgen \
    --key-type "$KEY_TYPE" --id "$id" --label "$obj" >&2
}

mk_keypair "01" "${LABEL}-key1"
if [ "$KEYPAIRS" = "2" ]; then
  mk_keypair "02" "${LABEL}-key2"
fi

# --- verify what we actually created (never trust the generator) ---------
PUBKEY_COUNT="$("${P11[@]}" --list-objects --type pubkey 2>/dev/null | grep -c '^Public Key Object' || true)"
[ "$PUBKEY_COUNT" = "$KEYPAIRS" ] || \
  die "expected ${KEYPAIRS} public key object(s) on token '${LABEL}', found ${PUBKEY_COUNT}"

# --- emit the URIs -------------------------------------------------------
# clevis parses `module-path`, and optionally `slot-index` / `mechanism`, out
# of the URI query string. The id is percent-encoded per RFC 7512 (hex byte
# 0x01 -> %01), which is what pkcs11-tool --id 01 produced.
uri_for() {
  printf 'pkcs11:token=%s;id=%%%s;object=%s-key%s?module-path=%s\n' \
    "$LABEL" "$1" "$LABEL" "$2" "$MODULE"
}

echo "SOFTHSM2_CONF=${CONF}"
echo "SOFTHSM_MODULE=${MODULE}"
echo "TOKENDIR=${TOKENDIR}"
echo "TOKEN_LABEL=${LABEL}"
echo "TOKEN_PIN=${PIN}"
echo "TOKEN_KEYPAIRS=${KEYPAIRS}"
echo "PKCS11_URI_1=$(uri_for 01 1)"
if [ "$KEYPAIRS" = "2" ]; then
  echo "PKCS11_URI_2=$(uri_for 02 2)"
fi

log "done. Export SOFTHSM2_CONF=${CONF} in any shell that must see this token."
exit 0
