#!/usr/bin/env bash
# file: scripts/deploy-usb-configs.sh
# version: 1.0.0
# guid: 3f8a2c61-7d94-4b0e-a5c2-9e1f6b3d8a47
# last-edited: 2026-07-09
#
# Place per-host InstallationConfig files where the autoinstall-agent's
# MAC-resolved endpoint (GET /autoinstall/uaa-config) serves them:
#
#   <src-dir>/<host>.yaml  ->  /var/www/html/cloud-init/<hexmac>/uaa.yaml
#
# RUN THIS ON (or from) THE SERVER 172.16.2.30 as a HUMAN — the repo copies of
# the configs carry `REPLACE_AT_PLACE_TIME` secret placeholders (luks_key,
# root_password, tpm2_pin). Inject the real secrets into a staging copy first;
# this script REFUSES to place any file that still contains the placeholder
# (fail loud, place nothing for that host) so an unusable/secretless config can
# never be served to a booting installer.
#
# Usage:
#   scripts/deploy-usb-configs.sh [--src <dir>] [--dest <cloud-init base>] [host ...]
#
#   --src   directory of <host>.yaml files (default: examples/configs/install
#           next to this script — NOTE these still hold placeholders; point
#           --src at your secret-injected staging copies)
#   --dest  cloud-init web root (default: /var/www/html/cloud-init)
#   host…   subset of hosts to place (default: all known hosts)
#
# Exit status: 0 if every requested host was placed; 1 if any host was refused
# (placeholder present, source missing, or unknown host).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SRC_DIR="${SCRIPT_DIR}/../examples/configs/install"
DEST_BASE="/var/www/html/cloud-init"
PLACEHOLDER="REPLACE_AT_PLACE_TIME"

# hostname -> MAC (hexmac derived below). These are the known fleet MACs.
# (case-based lookup, not `declare -A`, so the script also runs under the
# ancient bash 3.2 on macOS when driving the server remotely)
KNOWN_HOSTS="len-serv-001 len-serv-002 len-serv-003 unimatrixone"
mac_for_host() {
    case "$1" in
        len-serv-001) echo "6c:4b:90:bc:39:b3" ;;
        len-serv-002) echo "6c:4b:90:bc:f8:a3" ;;
        len-serv-003) echo "6c:4b:90:bc:f7:f4" ;;
        unimatrixone) echo "ac:1f:6b:40:fc:e2" ;;
        *) return 1 ;;
    esac
}

HOSTS=()
while [ $# -gt 0 ]; do
    case "$1" in
        --src)  SRC_DIR="$2";  shift 2 ;;
        --dest) DEST_BASE="$2"; shift 2 ;;
        -h|--help) grep '^#' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        -*) echo "ERROR: unknown flag: $1" >&2; exit 1 ;;
        *)  HOSTS+=("$1"); shift ;;
    esac
done
[ ${#HOSTS[@]} -gt 0 ] || HOSTS=($KNOWN_HOSTS)

fail=0
for host in "${HOSTS[@]}"; do
    if ! mac="$(mac_for_host "$host")"; then
        echo "REFUSED $host: unknown host (add its MAC to mac_for_host)" >&2
        fail=1
        continue
    fi
    hexmac="${mac//:/}"
    src="${SRC_DIR}/${host}.yaml"
    if [ ! -f "$src" ]; then
        echo "REFUSED $host: source not found: $src" >&2
        fail=1
        continue
    fi
    # HARD GATE: never place a config whose secrets were not injected.
    if grep -q "$PLACEHOLDER" "$src"; then
        echo "REFUSED $host: $src still contains ${PLACEHOLDER} — inject real" \
             "secrets into a staging copy and pass it via --src" >&2
        fail=1
        continue
    fi
    dest_dir="${DEST_BASE}/${hexmac}"
    mkdir -p "$dest_dir"
    install -m 0644 "$src" "${dest_dir}/uaa.yaml"
    echo "PLACED  $host ($mac) -> ${dest_dir}/uaa.yaml"
done

exit "$fail"
