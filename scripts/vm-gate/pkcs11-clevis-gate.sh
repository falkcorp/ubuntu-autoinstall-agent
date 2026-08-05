#!/usr/bin/env bash
# file: scripts/vm-gate/pkcs11-clevis-gate.sh
# version: 1.0.0
# guid: 87debf16-b71f-4c2c-ab16-774b9470a674
# last-edited: 2026-08-02
#
# Clevis PKCS#11 + SSS policy gate, driven by SoftHSM2, local Tang and swtpm.
# Proves the UNLOCK POLICY LOGIC of the three-keyslot design before anyone
# reboots a real host:
#
#   Slot A (unattended)  {"t":2,"pins":{"tpm2":{"pcr_ids":"7","pcr_bank":"sha256"},
#                          "sss":[{"t":2,"pins":{"tang":[a,b,c]}}]}}
#   Slot B (break-glass) {"t":1,"pins":{"pkcs11":[uriA,uriB]}}
#   Slot 0               passphrase
#
# NEGATIVE CONTROLS RUN FIRST AND ARE NOT SKIPPABLE. A gate that has never
# shown a red result proves nothing; this repo has already been bitten by a
# confounded test. Stage 2 asserts that three bindings which MUST fail DO
# fail; if any of them passes, the whole run is declared confounded and the
# gate fails immediately without running the positive assertions.
#
# THERE IS NO SKIP STATE. A missing prerequisite is a FAIL.
#
# SCOPE — read docs/vm-gate-pkcs11.md "What this gate cannot prove":
#   - SoftHSM is not a YubiKey (no hardware-enforced destructive PIN retry
#     counter, no touch policy, no pcscd/CCID stack in the path).
#   - swtpm PCR7 values do not match real Secure Boot hardware.
#   - A userspace `clevis luks unlock` does NOT exercise the boot-time
#     askpass path. That is why scripts/vm-gate/verify-initramfs.sh exists.
#
# NON-DESTRUCTIVE BY CONSTRUCTION:
#   - There is deliberately NO --device flag. Every LUKS container is a
#     sparse file this script creates under --workdir and attaches with
#     losetup. It is impossible to aim this harness at a real disk.
#   - Tang runs on 127.0.0.1 only. Any 172.16.x.x argument is a hard failure.
#   - Only PIDs this script started are ever killed (never `pkill` by name —
#     the host may be running other VMs; same rule as scripts/vm-validate.sh).
#
# Usage (as root — losetup/cryptsetup need it):
#   sudo ./scripts/vm-gate/pkcs11-clevis-gate.sh \
#       [--workdir ./vm-gate-work] [--tang-base-port 9080] \
#       [--swtpm-port 2321] [--stages all] [--keep]
#
# --stages is a DEBUG aid only (comma list of: neg,slota,slotb,gotcha,regen).
# Anything other than `all` prints "GATE: PARTIAL" and can never print
# "GATE: PASS", so a partial run cannot be mistaken for a passing gate.
#
# Requires: cryptsetup, losetup, clevis (>=18) incl. clevis-pin-tpm2 and the
# pkcs11 pin, jose, curl, tang (tangd + tangd-keygen), systemd-socket-activate,
# swtpm, tpm2-tools, softhsm2, opensc (pkcs11-tool).
#
# See docs/vm-gate-pkcs11.md for the operator walkthrough.

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

die() { echo "ERROR: $*" >&2; exit 1; }

# --- defaults -----------------------------------------------------------
WORKDIR="./vm-gate-work"
TANG_BASE_PORT="9080"
SWTPM_PORT="2321"
STAGES="all"
KEEP="0"
PIN_FILE="/run/systemd/clevis-pkcs11.pin"
TOKEN_PIN="123456"
WRONG_PIN="999999"
LUKS_PASS="vm-gate-throwaway-passphrase"

while [ $# -gt 0 ]; do
  case "$1" in
    --workdir)        WORKDIR="${2:?--workdir needs a dir}"; shift 2 ;;
    --tang-base-port) TANG_BASE_PORT="${2:?--tang-base-port needs a port}"; shift 2 ;;
    --swtpm-port)     SWTPM_PORT="${2:?--swtpm-port needs a port}"; shift 2 ;;
    --stages)         STAGES="${2:?--stages needs a comma list or 'all'}"; shift 2 ;;
    --pin-file)       PIN_FILE="${2:?--pin-file needs a path}"; shift 2 ;;
    --keep)           KEEP="1"; shift ;;
    -h|--help)        grep '^#' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    -*)               die "unknown flag: $1" ;;
    *)                die "unexpected positional arg: $1 (use --workdir/... flags)" ;;
  esac
done

# =========================================================================
# GUARD — refuse to run against anything that is not throwaway.
# =========================================================================
for a in "$WORKDIR" "$TANG_BASE_PORT" "$SWTPM_PORT" "$PIN_FILE"; do
  case "$a" in
    *172.16.*) die "GUARD: '$a' names a 172.16.x.x host. This harness is loopback-only and must never touch the fleet." ;;
    /dev/sd*|/dev/nvme*|/dev/vd*|/dev/hd*|/dev/mapper/*|/dev/disk/*)
      die "GUARD: '$a' names a real block device. This harness creates its own loopback containers and must never be pointed at a disk." ;;
  esac
done
case "$WORKDIR" in
  /|/boot|/boot/*|/etc|/etc/*|/usr|/usr/*|/var/lib/softhsm|/var/lib/softhsm/*|/dev|/dev/*)
    die "GUARD: --workdir '$WORKDIR' is a system path. Use a throwaway scratch directory." ;;
esac
[ "$(uname -s)" = "Linux" ] || die "Linux only (needs losetup/cryptsetup/clevis). macOS is unsupported."
[ "$(id -u)" = "0" ] || die "must run as root (losetup + cryptsetup luksFormat)"

mkdir -p "$WORKDIR"
WORKDIR="$(cd "$WORKDIR" && pwd)"
LOGDIR="${WORKDIR}/logs"
mkdir -p "$LOGDIR" "${WORKDIR}/luks" "${WORKDIR}/tang" "${WORKDIR}/swtpm"

# --- state tracked for cleanup + the final report ------------------------
declare -a TANG_PIDS=() TANG_PORTS=() LOOPDEVS=() OPENED_MAPPINGS=()
SWTPM_PID=""
PIN_FILE_CREATED="0"
FIRST_FAILING_STAGE=""
PASS_COUNT=0
FAIL_COUNT=0
GOTCHA_VERDICT="UNKNOWN (stage not reached)"
declare -a REPORT_LINES=()

stage_echo() { echo "==> stage $1 $2"; }

# Never `pkill` by name — only PIDs this harness started.
# shellcheck disable=SC2329 # invoked indirectly via `trap cleanup EXIT`
cleanup() {
  local ec=$? m d pid
  for m in "${OPENED_MAPPINGS[@]}"; do
    cryptsetup close "$m" 2>/dev/null || true
  done
  for pid in "${TANG_PIDS[@]}" "$SWTPM_PID"; do
    if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
      kill "$pid" 2>/dev/null || true
      wait "$pid" 2>/dev/null || true
    fi
  done
  for d in "${LOOPDEVS[@]}"; do
    losetup -d "$d" 2>/dev/null || true
  done
  if [ "$PIN_FILE_CREATED" = "1" ]; then
    shred -u "$PIN_FILE" 2>/dev/null || rm -f "$PIN_FILE" 2>/dev/null || true
  fi
  if [ "$KEEP" = "0" ]; then
    rm -f "${WORKDIR}"/luks/*.img 2>/dev/null || true
  fi
  exit "$ec"
}
trap cleanup EXIT

print_report() {
  local gate="$1"
  {
    echo "==== CLEVIS PKCS#11 GATE REPORT ===="
    printf '%s\n' "${REPORT_LINES[@]}"
    echo "multi-keypair 'first public key wins' verdict: ${GOTCHA_VERDICT}"
    echo "asserts passed=${PASS_COUNT} failed=${FAIL_COUNT}"
    echo "NOTE: this gate proves POLICY LOGIC and INITRAMFS CONTENTS only."
    echo "      SoftHSM != YubiKey; swtpm PCR7 != real hardware PCR7;"
    echo "      userspace unlock != boot-time askpass path."
    case "$gate" in
      PASS)    echo "GATE: PASS" ;;
      PARTIAL) echo "GATE: PARTIAL (--stages='${STAGES}' — a partial run is NOT a passing gate)" ;;
      *)       echo "GATE: FAIL (${FIRST_FAILING_STAGE:-unknown stage})" ;;
    esac
    echo "==================================="
  } | tee -a "${LOGDIR}/99-report.log"
}

fail_stage() {
  local stage="$1" msg="$2"
  FIRST_FAILING_STAGE="stage ${stage}: ${msg}"
  echo "ERROR: stage ${stage} failed: ${msg}" >&2
  echo "See logs under: ${LOGDIR}/" >&2
  print_report "FAIL"
  exit 1
}

want_stage() {
  [ "$STAGES" = "all" ] && return 0
  case ",${STAGES}," in *",$1,"*) return 0 ;; *) return 1 ;; esac
}

# =========================================================================
# Assertion primitive.
#
#   assert_cmd <id> <name> <EXPECT: OK|FAIL> <cmd...>
#
# The EXPECT column is the anti-confounding device: it is printed in the
# report, so a reviewer can see at a glance which lines were supposed to be
# red. RESULT=PASS means "observed matched expected", NOT "the command
# succeeded".
# =========================================================================
assert_cmd() {
  local id="$1" name="$2" expect="$3"; shift 3
  local observed result rc=0 note="" log="${LOGDIR}/assert-${id}.log"
  echo "--- ${id} ${name} (EXPECT=${expect}): $*" >>"$log"
  # </dev/null + timeout: clevis prompts on /dev/tty when it cannot find a
  # PIN/passphrase. Without these the harness would HANG instead of failing,
  # and a hang reads as neither red nor green.
  timeout 120 "$@" </dev/null >>"$log" 2>&1 || rc=$?

  # NOT every nonzero status is a legitimate red. A timeout (124) or a
  # missing binary (127) means the assertion never actually ran, and
  # crediting those as OBSERVED=FAIL would let an EXPECT=FAIL row pass for
  # entirely the wrong reason — the exact confounding this gate exists to
  # prevent. They are INDETERMINATE, and INDETERMINATE is always a failure.
  case "$rc" in
    0)       observed="OK" ;;
    124)     observed="INDET"; note=" (rc=124 TIMEOUT — command never completed; not a legitimate red)" ;;
    125|126|127) observed="INDET"; note=" (rc=${rc} command not found/not executable; the assertion never ran)" ;;
    *)       observed="FAIL" ;;
  esac

  if [ "$observed" = "INDET" ]; then
    result="FAIL"; FAIL_COUNT=$((FAIL_COUNT + 1))
  elif [ "$observed" = "$expect" ]; then
    result="PASS"; PASS_COUNT=$((PASS_COUNT + 1))
  else
    result="FAIL"; FAIL_COUNT=$((FAIL_COUNT + 1))
  fi
  local line
  line="$(printf 'ASSERT %-10s %-34s EXPECT=%-4s OBSERVED=%-5s RESULT=%s%s' \
    "$id" "$name" "$expect" "$observed" "$result" "$note")"
  REPORT_LINES+=("$line")
  echo "$line"
  [ "$result" = "PASS" ]
}

# =========================================================================
# Stage 0: preflight — every tool is REQUIRED. Missing => FAIL, never skip.
# =========================================================================
stage_echo 0 preflight
for tool in cryptsetup losetup clevis clevis-encrypt-sss jose curl \
            softhsm2-util pkcs11-tool swtpm tpm2_pcrread jq \
            systemd-socket-activate shred; do
  command -v "$tool" >/dev/null || \
    fail_stage 0 "required tool '$tool' not found — install it; this gate has no skip state"
done
for helper in clevis-decrypt-pkcs11 clevis-encrypt-pkcs11 clevis-decrypt-tpm2 clevis-decrypt-tang; do
  command -v "$helper" >/dev/null || \
    fail_stage 0 "clevis helper '$helper' not found. For clevis-decrypt-tpm2 this is the exact bug the fleet already has: the clevis-tpm2 package is not installed."
done

TANGD=""
for cand in /usr/libexec/tangd /usr/lib/x86_64-linux-gnu/tangd /usr/lib/aarch64-linux-gnu/tangd /usr/lib/tangd; do
  if [ -x "$cand" ]; then TANGD="$cand"; break; fi
done
[ -n "$TANGD" ] || fail_stage 0 "tangd binary not found (apt install tang)"
TANGD_KEYGEN=""
for cand in /usr/libexec/tangd-keygen /usr/lib/x86_64-linux-gnu/tangd-keygen /usr/lib/aarch64-linux-gnu/tangd-keygen /usr/lib/tangd-keygen; do
  if [ -x "$cand" ]; then TANGD_KEYGEN="$cand"; break; fi
done
[ -n "$TANGD_KEYGEN" ] || fail_stage 0 "tangd-keygen not found (apt install tang)"

# Refuse to clobber a real host's clevis PIN file. If it already exists this
# machine is (or is pretending to be) a real pkcs11-unlock host.
if [ -e "$PIN_FILE" ]; then
  fail_stage 0 "'$PIN_FILE' already exists — this looks like a real clevis-pkcs11 host, not a throwaway gate target. Refusing to touch it. (If a PREVIOUS gate run was kill -9'd, its cleanup never ran and this is just a stale file: verify it is not a real host, 'shred -u $PIN_FILE', and re-run.)"
fi

# =========================================================================
# Stage 1: fixtures — three local Tang, one swtpm, three SoftHSM tokens.
# =========================================================================
stage_echo 1 fixtures

start_tang() {
  local idx="$1" port=$((TANG_BASE_PORT + $1)) keydir="${WORKDIR}/tang/db${1}"
  mkdir -p "$keydir"
  # Key ONCE per key dir. start_tang is called again after every simulated
  # outage, and re-running tangd-keygen on a populated dir risks rotating the
  # keys out from under bindings made earlier in the run — which would surface
  # as "Slot A stopped unlocking after initramfs regeneration" in stage 6 and
  # send the operator chasing the wrong cause entirely.
  if ! compgen -G "${keydir}/*.jwk" >/dev/null 2>&1; then
    "$TANGD_KEYGEN" "$keydir" >>"${LOGDIR}/01-tang.log" 2>&1 || \
      fail_stage 1 "tangd-keygen failed for db${idx} — see ${LOGDIR}/01-tang.log"
  fi
  # tangd is an inetd-style service: one request per accepted connection.
  systemd-socket-activate -l "127.0.0.1:${port}" --accept --inetd \
    -- "$TANGD" "$keydir" >>"${LOGDIR}/01-tang.log" 2>&1 &
  TANG_PIDS[idx]=$!
  TANG_PORTS[idx]="$port"
  local i
  for i in $(seq 1 30); do
    if curl -sf --max-time 2 "http://127.0.0.1:${port}/adv" -o /dev/null; then return 0; fi
    sleep 0.5
  done
  fail_stage 1 "tang ${idx} on 127.0.0.1:${port} never served /adv — see ${LOGDIR}/01-tang.log"
}
for i in 0 1 2; do start_tang "$i"; done

tang_kill() {
  local idx="$1" pid="${TANG_PIDS[$1]}"
  if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
  fi
  TANG_PIDS[idx]=""
  # Prove it is actually down. "Simulated down" that is still answering is
  # the classic confounded negative control.
  if curl -sf --max-time 2 "http://127.0.0.1:${TANG_PORTS[$idx]}/adv" -o /dev/null; then
    fail_stage 1 "tang ${idx} still answering after kill — negative control would be confounded"
  fi
}

# swtpm over TCP: tpm2-tools reaches it via TPM2TOOLS_TCTI, which
# clevis-encrypt-tpm2 / clevis-decrypt-tpm2 inherit because they shell out to
# tpm2_* as child processes.
#
# --tpmstate persists across a restart, so the seed (and therefore the SRK the
# tpm2 pin sealed to) survives tpm_down/tpm_up. That is what lets the harness
# simulate "the TPM is absent" without invalidating the binding.
export TPM2TOOLS_TCTI="swtpm:host=127.0.0.1,port=${SWTPM_PORT}"
start_swtpm() {
  swtpm socket --tpm2 \
    --tpmstate "dir=${WORKDIR}/swtpm" \
    --server "type=tcp,port=${SWTPM_PORT},bindaddr=127.0.0.1" \
    --ctrl "type=tcp,port=$((SWTPM_PORT + 1)),bindaddr=127.0.0.1" \
    --flags not-need-init,startup-clear \
    >>"${LOGDIR}/01-swtpm.log" 2>&1 &
  SWTPM_PID=$!
  local i
  for i in $(seq 1 30); do
    if tpm2_pcrread sha256:7 >>"${LOGDIR}/01-swtpm.log" 2>&1; then return 0; fi
    sleep 0.5
  done
  fail_stage 1 "swtpm not reachable via TPM2TOOLS_TCTI='${TPM2TOOLS_TCTI}'. Slot A cannot be asserted on this host — run the gate inside the QEMU VM (scripts/vm-validate.sh already attaches a swtpm tpm-tis device) instead. This is a FAIL, not a skip."
}
tpm_down() {
  if [ -n "$SWTPM_PID" ] && kill -0 "$SWTPM_PID" 2>/dev/null; then
    kill "$SWTPM_PID" 2>/dev/null || true
    wait "$SWTPM_PID" 2>/dev/null || true
  fi
  SWTPM_PID=""
  # Prove it is really gone. A "simulated absent" TPM that still answers is
  # the classic confounded negative control.
  if tpm2_pcrread sha256:7 >/dev/null 2>&1; then
    fail_stage 1 "swtpm still answering after kill — the TPM-absent control would be confounded"
  fi
}
tpm_up() { start_swtpm; }
start_swtpm

# SoftHSM tokens. Three separate tokens, deliberately:
#   gate1  single keypair  -> Slot B primary URI + positive control
#   gate2  single keypair  -> Slot B secondary URI (proves the pkcs11 array)
#   gatex  TWO keypairs    -> the "first public key wins" fixture
# and gateneg, sacrificed to the wrong-PIN control so a PIN lockout cannot
# poison any other assertion.
softhsm_setup() {
  "${HERE}/softhsm-setup.sh" --workdir "$WORKDIR" --label "$1" \
    --pin "$TOKEN_PIN" --keypairs "$2" --quiet
}
{ softhsm_setup gate1 1; softhsm_setup gate2 1; softhsm_setup gatex 2; softhsm_setup gateneg 1; } \
  >"${LOGDIR}/01-softhsm.env" 2>>"${LOGDIR}/01-softhsm.log" \
  || fail_stage 1 "softhsm-setup.sh failed — see ${LOGDIR}/01-softhsm.log"

export SOFTHSM2_CONF="${WORKDIR}/softhsm2.conf"
SOFTHSM_MODULE="$(grep -m1 '^SOFTHSM_MODULE=' "${LOGDIR}/01-softhsm.env" | cut -d= -f2-)"
[ -n "$SOFTHSM_MODULE" ] || fail_stage 1 "could not determine SOFTHSM_MODULE from ${LOGDIR}/01-softhsm.env"

uri() { # uri <token-label> <id-hex> <keynum>
  printf 'pkcs11:token=%s;id=%%%s;object=%s-key%s?module-path=%s' "$1" "$2" "$1" "$3" "$SOFTHSM_MODULE"
}
URI_A="$(uri gate1 01 1)"
URI_B="$(uri gate2 01 1)"
URI_X_WRONG="$(uri gatex 02 2)"   # asks for key 02 on a two-keypair token
URI_X_RIGHT="$(uri gatex 01 1)"
URI_NEG="$(uri gateneg 01 1)"

# =========================================================================
# Locate the uaa binary. The gate no longer implements its own opinion about
# what a safe policy is; it asks the SAME evaluator the installer's verifier
# uses. See the helper below for why.
#
# NOTE the binary is `ubuntu-autoinstall-agent`, not `uaa`.
# =========================================================================
if [ -n "${UAA_BIN:-}" ] && [ -x "${UAA_BIN}" ]; then
  :
elif UAA_BIN="$(command -v ubuntu-autoinstall-agent 2>/dev/null)" && [ -n "$UAA_BIN" ]; then
  :
else
  _gate_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
  _repo_root="$(cd "${_gate_dir}/../.." && pwd)"
  for _cand in "${_repo_root}/target/release/ubuntu-autoinstall-agent" \
               "${_repo_root}/target/debug/ubuntu-autoinstall-agent"; do
    [ -x "$_cand" ] && { UAA_BIN="$_cand"; break; }
  done
fi
# Fail CLOSED and EARLY. If this is unset the check-binding helper exits 127,
# which assert_cmd scores INDETERMINATE — correct, but it would burn a whole
# gate run to say "you didn't build the binary".
[ -n "${UAA_BIN:-}" ] && [ -x "${UAA_BIN}" ] || {
  echo "ERROR: could not find the ubuntu-autoinstall-agent binary." >&2
  echo "       Build it (cargo build) or set UAA_BIN=/path/to/ubuntu-autoinstall-agent." >&2
  echo "       The gate needs it to judge share topology with the same code the" >&2
  echo "       installer's verifier uses; it will not fall back to its own rule." >&2
  exit 2
}
echo "==> judging policies with ${UAA_BIN} verify-policy" >&2

# =========================================================================
# Generated helper: inspect the ACTUAL SHARE TOPOLOGY of a binding.
#
# WHY THIS DELEGATES INSTEAD OF DECIDING.
#
# This helper used to hold its own rule — "exit 1 if `tang` is a DIRECT child
# of the outer pins" — evaluated with jq over `clevis luks list` output. Both
# halves of that were wrong by the time the nested-unlock work landed:
#
#   1. WRONG DATA. `clevis luks list` misrenders a nested policy: it DROPS
#      shares and collapses arrays into bare objects (measured on clevis 20 /
#      cryptsetup 2.8.4 — see CLEVIS_PROBE_COMMAND in verify.rs). A topology
#      judgement made from it is a judgement about a tree that does not exist.
#
#   2. WRONG RULE. "no top-level pins.tang" is far weaker than the property the
#      fleet actually needs. It blesses, for example, an outer t=1 OR one of
#      whose branches is satisfiable by a single share — no top-level tang
#      anywhere, and one factor opens the volume.
#
# Two implementations of one security rule drift apart, and these two did. The
# rule now lives ONLY in evaluate_clevis_binding(), and this asks it directly:
# ground truth from the LUKS2 token's JWE, judged by cheapest-satisfying-set.
#
# Modes:
#   topology  exit 0 iff the real verifier passes the binding on $dev
# =========================================================================
CHECK_BINDING="${WORKDIR}/check-binding.sh"
cat >"$CHECK_BINDING" <<CHECKEOF
#!/usr/bin/env bash
# GENERATED by pkcs11-clevis-gate.sh — do not edit in place.
# Thin shim so assert_cmd (which runs \`timeout 120 "\$@"\`, not shell
# functions) can invoke the real verifier as a command.
set -uo pipefail
mode="\$1"; dev="\$2"; shift 2
case "\$mode" in
  topology)
    # --device makes the binary run CLEVIS_PROBE_COMMAND itself, so the gate
    # and the SSH path ask the kernel for metadata the exact same way.
    exec "${UAA_BIN}" verify-policy --device "\$dev"
    ;;
  *)
    echo "unknown mode: \$mode (only 'topology' survives; the 'substring' mode" >&2
    echo "demonstrated a verify.rs substring check that no longer exists)" >&2
    exit 2
    ;;
esac
CHECKEOF
chmod +x "$CHECK_BINDING"

# The PIN file clevis-decrypt-pkcs11 reads. Created by us, removed on exit.
write_pin_file() {
  mkdir -p "$(dirname "$PIN_FILE")"
  install -m 0600 /dev/null "$PIN_FILE"
  printf '%s' "$1" >"$PIN_FILE"
  PIN_FILE_CREATED="1"
}
write_pin_file "$TOKEN_PIN"

# Tang advertisements, pre-fetched. WITHOUT `adv`, `clevis luks bind` prompts
# on /dev/tty to confirm the Tang signing keys and fails non-interactively —
# the exact silent-killer fixed in crates/uaa-core/.../system_setup.rs.
# Split in two: fetch_advs may fail_stage (it runs in the current shell),
# tang_json only prints (safe inside a command substitution).
fetch_advs() {
  local idx
  for idx in 0 1 2; do
    curl -sf --max-time 10 "http://127.0.0.1:${TANG_PORTS[$idx]}/adv" \
      -o "${WORKDIR}/tang/${idx}.adv" \
      || fail_stage 1 "could not fetch Tang ${idx} advertisement"
  done
}
tang_json() {
  local idx sep=""
  for idx in 0 1 2; do
    printf '%s{"url":"http://127.0.0.1:%s","adv":"%s"}' \
      "$sep" "${TANG_PORTS[$idx]}" "${WORKDIR}/tang/${idx}.adv"
    sep=","
  done
}
fetch_advs
TANG_JSON="$(tang_json)"

# True AND across tpm2 and the Tang quorum requires NESTING. A flat
# {"t":2,"pins":{"tang":[a,b,c],"tpm2":{}}} is 2-of-FOUR shares and Tang
# alone satisfies it — measured, not assumed.
SLOT_A_JSON="{\"t\":2,\"pins\":{\"tpm2\":{\"pcr_ids\":\"7\",\"pcr_bank\":\"sha256\"},\"sss\":[{\"t\":2,\"pins\":{\"tang\":[${TANG_JSON}]}}]}}"
SLOT_B_JSON="{\"t\":1,\"pins\":{\"pkcs11\":[{\"uri\":\"${URI_A}\"},{\"uri\":\"${URI_B}\"}]}}"

# The BROKEN flat form, bound deliberately as a WITNESS. It is 2-of-FOUR
# shares (three tang + one tpm2), so any two Tang alone satisfy it. It exists
# only so the harness can prove its own TPM-absent simulation is meaningful
# and that the topology check has teeth.
FLAT_A_JSON="{\"t\":2,\"pins\":{\"tang\":[${TANG_JSON}],\"tpm2\":{\"pcr_ids\":\"7\",\"pcr_bank\":\"sha256\"}}}"

# --- LUKS container factory ---------------------------------------------
KEYFILE="${WORKDIR}/luks.key"
install -m 0600 /dev/null "$KEYFILE"
printf '%s' "$LUKS_PASS" >"$KEYFILE"

# new_container <name> -> sets the global NEW_DEV.
#
# Deliberately NOT `dev=$(new_container x)`: a command substitution runs in a
# subshell, so LOOPDEVS+=() would be lost (cleanup would leak loop devices)
# and a fail_stage inside would only kill the subshell.
NEW_DEV=""
new_container() {
  local name="$1" img dev
  img="${WORKDIR}/luks/${name}.img"
  rm -f "$img"
  truncate -s 64M "$img"
  dev="$(losetup --find --show "$img")" || fail_stage 1 "losetup failed for $img"
  LOOPDEVS+=("$dev")
  # Defence in depth: even though we created it, verify it really is a loop
  # device backed by a file under WORKDIR before cryptsetup touches it.
  [ "$(lsblk -no TYPE "$dev" | head -1)" = "loop" ] || \
    fail_stage 1 "GUARD: '$dev' is not a loop device — refusing to luksFormat it"
  case "$(losetup -nO BACK-FILE "$dev")" in
    "${WORKDIR}"/*) : ;;
    *) fail_stage 1 "GUARD: '$dev' is not backed by a file under ${WORKDIR} — refusing to luksFormat it" ;;
  esac
  cryptsetup luksFormat --type luks2 --batch-mode --pbkdf pbkdf2 \
    --pbkdf-force-iterations 1000 "$dev" "$KEYFILE" \
    >>"${LOGDIR}/01-luks.log" 2>&1 || fail_stage 1 "luksFormat failed on $dev"
  NEW_DEV="$dev"
}

bind_pin() { # bind_pin <dev> <pin> <json>
  clevis luks bind -y -d "$1" -k "$KEYFILE" "$2" "$3"
}

# assert_cmd runs its command under `timeout`, which is an external binary and
# therefore CANNOT invoke a shell function. Every unlock assertion below spells
# out `clevis luks unlock` directly; track_mapping only registers the mapper
# name for cleanup.
track_mapping() { OPENED_MAPPINGS+=("$1"); }

close_mapping() { cryptsetup close "$1" 2>/dev/null || true; }

# =========================================================================
# Stage 2: NEGATIVE CONTROLS — these run FIRST and must all go red.
# =========================================================================
if want_stage neg; then
  stage_echo 2 negative-controls
  NEG_BASELINE_FAILS=0

  # pos-00 is a POSITIVE control that lives in the negative stage on purpose:
  # if plain passphrase unlock does not work, every red below is meaningless
  # (a harness that cannot unlock anything trivially "passes" every negative).
  new_container ctl00; DEV="$NEW_DEV"; track_mapping gate-ctl00
  assert_cmd pos-00 baseline-passphrase-unlock OK \
    cryptsetup open --key-file "$KEYFILE" "$DEV" gate-ctl00 || NEG_BASELINE_FAILS=1
  close_mapping gate-ctl00

  # pos-00b: PIN-DELIVERY control. This is the control that makes neg-01
  # attributable, and it MUST run first, on the same token and the same
  # container.
  #
  # Without it, if clevis-decrypt-pkcs11 does not read $PIN_FILE in userspace
  # at all, neg-01 goes red because clevis got NO pin — not because it got the
  # WRONG one — and the negative control banks a pass for entirely the wrong
  # reason. pos-00 (passphrase) does not control for this: it never touches
  # pkcs11. So: correct PIN must unlock HERE, then we swap in the wrong PIN.
  #
  # Uses the sacrificial `gateneg` token throughout, so a SoftHSM soft PIN
  # lockout triggered by the wrong-PIN attempt cannot poison any later
  # assertion. pos-00b has already run by then.
  new_container neg01; DEV="$NEW_DEV"
  bind_pin "$DEV" pkcs11 "{\"uri\":\"${URI_NEG}\"}" >>"${LOGDIR}/02-neg.log" 2>&1 \
    || fail_stage 2 "neg-01 setup: bind to gateneg failed outright — cannot test the wrong-PIN path (INDETERMINATE, not a red)"

  write_pin_file "$TOKEN_PIN"
  track_mapping gate-neg01a
  assert_cmd pos-00b pkcs11-correct-pin-unlocks OK \
    clevis luks unlock -d "$DEV" -n gate-neg01a || NEG_BASELINE_FAILS=1
  close_mapping gate-neg01a

  # neg-01: same token, same container, WRONG PIN. Bind only reads the PUBLIC
  # key, so the bind is not the thing under test — the UNLOCK is.
  write_pin_file "$WRONG_PIN"
  track_mapping gate-neg01
  assert_cmd neg-01 wrong-pin-unlock-fails FAIL \
    clevis luks unlock -d "$DEV" -n gate-neg01 || NEG_BASELINE_FAILS=1
  close_mapping gate-neg01
  write_pin_file "$TOKEN_PIN"

  # neg-02: token absent. Point SOFTHSM2_CONF at an EMPTY token store, which
  # is what "the YubiKey is not plugged in" looks like to the module.
  new_container neg02; DEV="$NEW_DEV"; track_mapping gate-neg02
  bind_pin "$DEV" pkcs11 "{\"uri\":\"${URI_A}\"}" >>"${LOGDIR}/02-neg.log" 2>&1 \
    || fail_stage 2 "neg-02 setup: bind to gate1 failed outright (INDETERMINATE, not a red)"
  mkdir -p "${WORKDIR}/empty-tokens"
  cat >"${WORKDIR}/softhsm2-empty.conf" <<EOF
directories.tokendir = ${WORKDIR}/empty-tokens
objectstore.backend = file
log.level = INFO
EOF
  assert_cmd neg-02 token-absent-unlock-fails FAIL \
    env "SOFTHSM2_CONF=${WORKDIR}/softhsm2-empty.conf" \
    clevis luks unlock -d "$DEV" -n gate-neg02 || NEG_BASELINE_FAILS=1
  close_mapping gate-neg02

  # ---- TOPOLOGY: flat-vs-nested. The measured gap in verify.rs:257. -----
  #
  # Bind BOTH forms, then take the TPM away with all three Tang up.
  #   pos-09 (witness) the FLAT config still unlocks -> proves it really is
  #          2-of-4 and that "TPM absent" is not simply breaking everything.
  #   neg-04           the NESTED config must NOT unlock -> the real assertion.
  # Without pos-09, neg-04 could go red merely because the harness broke the
  # TPM path for both, which would prove nothing about the topology.
  new_container flatA; DEV_FLAT="$NEW_DEV"
  bind_pin "$DEV_FLAT" sss "$FLAT_A_JSON" >>"${LOGDIR}/02-neg.log" 2>&1 \
    || fail_stage 2 "witness setup: flat-config bind failed (INDETERMINATE, not a red)"
  new_container neg03; DEV_A_QUORUM="$NEW_DEV"
  bind_pin "$DEV_A_QUORUM" sss "$SLOT_A_JSON" >>"${LOGDIR}/02-neg.log" 2>&1 \
    || fail_stage 2 "neg-03/neg-04 setup: Slot A bind failed with all Tang up (INDETERMINATE, not a red)"

  # Static topology checks first — cheap, and they must disagree about the
  # two bindings or the check has no teeth. Both now run the SAME evaluator
  # the installer's verifier uses, so a green here is a statement about the
  # shipping rule and not about a rule that only the gate believes.
  assert_cmd pos-10 verifier-accepts-nested-AND OK \
    "$CHECK_BINDING" topology "$DEV_A_QUORUM" || NEG_BASELINE_FAILS=1
  assert_cmd neg-05 verifier-rejects-flat-2-of-4 FAIL \
    "$CHECK_BINDING" topology "$DEV_FLAT" || NEG_BASELINE_FAILS=1

  # pos-11 (`substrings-pass-on-BROKEN-flat`) is RETIRED, not lost. It asserted
  # that verify.rs's three substring predicates — contains("sss"),
  # contains("\"t\":2"), every Tang URL present — all passed on the flat config,
  # to demonstrate the verifier could not separate the two bindings. Those
  # predicates no longer exist: evaluate_clevis_binding now walks the policy
  # tree recovered from the LUKS2 token's JWE. The demonstration would have to
  # reimplement a deleted implementation to keep asserting anything, and its
  # subject is exactly what neg-05 above now proves is FIXED.

  # Now the behavioural half: TPM absent, all Tang up.
  tpm_down
  track_mapping gate-flat
  assert_cmd pos-09 flat-tang-alone-unlocks-WITNESS OK \
    clevis luks unlock -d "$DEV_FLAT" -n gate-flat || NEG_BASELINE_FAILS=1
  close_mapping gate-flat

  track_mapping gate-neg04
  assert_cmd neg-04 nested-slotA-tang-alone-fails FAIL \
    clevis luks unlock -d "$DEV_A_QUORUM" -n gate-neg04 || NEG_BASELINE_FAILS=1
  close_mapping gate-neg04
  tpm_up

  # neg-03: Slot A with the INNER Tang quorum unmet. Bind with all three Tang
  # up, then kill two. The inner sss is t=2 of 3, so it can no longer be
  # satisfied; the outer t=2 therefore has only the tpm2 share (1 < 2) and
  # unlock must fail. This is simultaneously the "FAILS with 2 of 3 down"
  # assertion from the design.
  track_mapping gate-neg03
  tang_kill 1
  tang_kill 2
  assert_cmd neg-03 slotA-tang-quorum-unmet-fails FAIL \
    clevis luks unlock -d "$DEV_A_QUORUM" -n gate-neg03 || NEG_BASELINE_FAILS=1
  close_mapping gate-neg03

  if [ "$NEG_BASELINE_FAILS" != "0" ]; then
    fail_stage 2 "a negative control did not produce its expected result — the harness is CONFOUNDED and no positive assertion below can be trusted. Fix this before reading any green."
  fi

  # Restore the two Tang instances for the positive stages.
  start_tang 1
  start_tang 2
  fetch_advs
  TANG_JSON="$(tang_json)"
  SLOT_A_JSON="{\"t\":2,\"pins\":{\"tpm2\":{\"pcr_ids\":\"7\",\"pcr_bank\":\"sha256\"},\"sss\":[{\"t\":2,\"pins\":{\"tang\":[${TANG_JSON}]}}]}}"
fi

# =========================================================================
# Stage 3: Slot A — unattended unlock, tpm2 AND (2-of-3 Tang).
# =========================================================================
if want_stage slota; then
  stage_echo 3 slot-A
  new_container slotA; DEV_A="$NEW_DEV"
  bind_pin "$DEV_A" sss "$SLOT_A_JSON" >>"${LOGDIR}/03-slota.log" 2>&1 \
    || fail_stage 3 "Slot A bind failed — see ${LOGDIR}/03-slota.log"

  track_mapping gate-a1
  assert_cmd pos-01 slotA-all-tang-up OK clevis luks unlock -d "$DEV_A" -n gate-a1 \
    || fail_stage 3 "Slot A did not unlock with all Tang up"
  close_mapping gate-a1

  tang_kill 2
  track_mapping gate-a2
  assert_cmd pos-02 slotA-one-tang-down OK clevis luks unlock -d "$DEV_A" -n gate-a2 \
    || fail_stage 3 "Slot A did not unlock with 1 of 3 Tang down — the 2-of-3 inner threshold is NOT being honoured"
  close_mapping gate-a2
  start_tang 2
fi

# =========================================================================
# Stage 4: Slot B — break-glass. The whole point is that it does NOT depend
# on Tang, so it is asserted with ALL THREE Tang instances down.
# =========================================================================
if want_stage slotb; then
  stage_echo 4 slot-B
  new_container slotB; DEV_B="$NEW_DEV"
  bind_pin "$DEV_B" sss "$SLOT_B_JSON" >>"${LOGDIR}/04-slotb.log" 2>&1 \
    || fail_stage 4 "Slot B bind failed — see ${LOGDIR}/04-slotb.log"

  tang_kill 0; tang_kill 1; tang_kill 2
  track_mapping gate-b1
  assert_cmd pos-03 slotB-all-tang-down OK clevis luks unlock -d "$DEV_B" -n gate-b1 \
    || fail_stage 4 "Slot B did not unlock with all Tang down — it has an unintended Tang dependency, which defeats its entire purpose"
  close_mapping gate-b1
  for i in 0 1 2; do start_tang "$i"; done
  fetch_advs
  TANG_JSON="$(tang_json)"
  SLOT_A_JSON="{\"t\":2,\"pins\":{\"tpm2\":{\"pcr_ids\":\"7\",\"pcr_bank\":\"sha256\"},\"sss\":[{\"t\":2,\"pins\":{\"tang\":[${TANG_JSON}]}}]}}"
fi

# =========================================================================
# Stage 5: the "first public key wins" gotcha.
#
# clevis-encrypt-pkcs11 selects the key with
#   pkcs11-tool -O | grep -i 'Public' -A10 | grep 'ID:' | head -1
# i.e. the FIRST public key on the token, ignoring id=/object= in the URI —
# while clevis-decrypt-pkcs11 DOES honour the URI. On a token with two
# keypairs, binding with id=02 therefore encrypts to key 01's public half and
# tries to decrypt with key 02's private half: bind succeeds, unlock fails.
#
# Tri-state, because "clevis fixed it upstream" is not a gate failure:
#   DETECTED       bind OK, unlock FAILS  -> expected on clevis 23
#   ABSENT         bind OK, unlock OK     -> upstream fixed; report only
#   INDETERMINATE  bind itself errored    -> the only FAIL
# =========================================================================
if want_stage gotcha; then
  stage_echo 5 multi-keypair-gotcha

  # Positive control on the SAME two-keypair token, using id=01 (the key the
  # buggy selector picks anyway). Without this, a harness that simply cannot
  # unlock anything would read as "gotcha detected".
  new_container gotchaCtl; DEV_XC="$NEW_DEV"
  bind_pin "$DEV_XC" pkcs11 "{\"uri\":\"${URI_X_RIGHT}\"}" >>"${LOGDIR}/05-gotcha.log" 2>&1 \
    || fail_stage 5 "control bind with id=01 on the two-keypair token failed"
  track_mapping gate-xc
  assert_cmd pos-04 gotcha-control-id01-unlocks OK clevis luks unlock -d "$DEV_XC" -n gate-xc \
    || fail_stage 5 "control unlock with id=01 failed — the gotcha result would be meaningless"
  close_mapping gate-xc

  new_container gotcha; DEV_X="$NEW_DEV"; track_mapping gate-x
  if bind_pin "$DEV_X" pkcs11 "{\"uri\":\"${URI_X_WRONG}\"}" >>"${LOGDIR}/05-gotcha.log" 2>&1; then
    if timeout 120 clevis luks unlock -d "$DEV_X" -n gate-x </dev/null >>"${LOGDIR}/05-gotcha.log" 2>&1; then
      close_mapping gate-x
      GOTCHA_VERDICT="ABSENT (bind honoured id=02; upstream clevis appears fixed — informational, not a failure)"
      REPORT_LINES+=("$(printf 'ASSERT %-10s %-34s EXPECT=%-4s OBSERVED=%-4s RESULT=%s' \
        gotcha-01 first-pubkey-wins TRI 'ABSENT' 'PASS')")
      PASS_COUNT=$((PASS_COUNT + 1))
    else
      GOTCHA_VERDICT="DETECTED (bind encrypted to key 01 while the URI said id=02; unlock fails). OPERATIONAL CONSEQUENCE: the token backing a real Slot B URI MUST contain exactly ONE keypair."
      REPORT_LINES+=("$(printf 'ASSERT %-10s %-34s EXPECT=%-4s OBSERVED=%-4s RESULT=%s' \
        gotcha-01 first-pubkey-wins TRI 'DETECTED' 'PASS')")
      PASS_COUNT=$((PASS_COUNT + 1))
    fi
  else
    GOTCHA_VERDICT="INDETERMINATE (bind itself errored — the gotcha could not be evaluated)"
    FAIL_COUNT=$((FAIL_COUNT + 1))
    REPORT_LINES+=("$(printf 'ASSERT %-10s %-34s EXPECT=%-4s OBSERVED=%-4s RESULT=%s' \
      gotcha-01 first-pubkey-wins TRI 'INDET' 'FAIL')")
    fail_stage 5 "multi-keypair bind errored outright — cannot distinguish 'gotcha present' from 'harness broken'"
  fi

  # Precondition the real deployment must satisfy, asserted here so nobody
  # ships a Slot B URI pointing at a multi-keypair token.
  assert_cmd pos-05 slotB-token-has-one-keypair OK \
    bash -c "test \"\$(pkcs11-tool --module '$SOFTHSM_MODULE' --token-label gate1 --list-objects --type pubkey 2>/dev/null | grep -c '^Public Key Object')\" = 1" \
    || fail_stage 5 "the Slot B token has more than one keypair — 'first public key wins' makes that binding unsafe"
fi

# =========================================================================
# Stage 6: initramfs regeneration.
#
# A real kernel upgrade cannot be performed non-destructively here, and
# `dracut -f` over /boot is destructive, so this stage regenerates the
# initramfs for the RUNNING kernel into a SCRATCH file under --workdir,
# verifies its contents with verify-initramfs.sh, and re-asserts that the
# existing bindings still unlock. That catches the real regression this
# assertion is for: a regen that drops the clevis pin modules.
#
# A true kernel-UPGRADE path must still be proven by a full boot in
# scripts/vm-validate.sh — see docs/vm-gate-pkcs11.md.
# =========================================================================
if want_stage regen; then
  stage_echo 6 initramfs-regen
  KVER="$(uname -r)"
  NEW_IMG="${WORKDIR}/initramfs-regen-${KVER}.img"
  rm -f "$NEW_IMG"
  if command -v dracut >/dev/null; then
    dracut --force "$NEW_IMG" "$KVER" >>"${LOGDIR}/06-regen.log" 2>&1 \
      || fail_stage 6 "dracut regeneration failed — see ${LOGDIR}/06-regen.log"
  elif command -v mkinitramfs >/dev/null; then
    mkinitramfs -o "$NEW_IMG" "$KVER" >>"${LOGDIR}/06-regen.log" 2>&1 \
      || fail_stage 6 "mkinitramfs regeneration failed — see ${LOGDIR}/06-regen.log"
  else
    fail_stage 6 "neither dracut nor mkinitramfs found — cannot regenerate an initramfs. FAIL, not skip."
  fi

  assert_cmd pos-06 regen-initramfs-contents-ok OK \
    "${HERE}/verify-initramfs.sh" --image "$NEW_IMG" --pin all \
    || fail_stage 6 "the regenerated initramfs is missing required clevis pieces — see ${LOGDIR}/assert-pos-06.log. THIS IS THE BUG CLASS THE FLEET ALREADY HAS (clevis present, clevis-decrypt-tpm2 absent)."

  if want_stage slota; then
    track_mapping gate-a3
    assert_cmd pos-07 slotA-unlocks-after-regen OK clevis luks unlock -d "$DEV_A" -n gate-a3 \
      || fail_stage 6 "Slot A stopped unlocking after initramfs regeneration"
    close_mapping gate-a3
  fi
  if want_stage slotb; then
    track_mapping gate-b3
    assert_cmd pos-08 slotB-unlocks-after-regen OK clevis luks unlock -d "$DEV_B" -n gate-b3 \
      || fail_stage 6 "Slot B stopped unlocking after initramfs regeneration"
    close_mapping gate-b3
  fi
fi

# =========================================================================
# Stage 7: report.
# =========================================================================
stage_echo 7 report
if [ "$FAIL_COUNT" != "0" ]; then
  FIRST_FAILING_STAGE="${FAIL_COUNT} assertion(s) did not match their EXPECT column"
  print_report "FAIL"
  exit 1
fi
if [ "$STAGES" = "all" ]; then
  print_report "PASS"
else
  print_report "PARTIAL"
  exit 1
fi
exit 0
