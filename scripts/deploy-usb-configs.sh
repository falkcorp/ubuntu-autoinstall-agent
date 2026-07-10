#!/usr/bin/env bash
# file: scripts/deploy-usb-configs.sh
# version: 1.1.0
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
#   scripts/deploy-usb-configs.sh [--src <dir>] [--dest <cloud-init base>] \
#       [--inject-from <secrets.yaml>] [host ...]
#
#   --src          directory of <host>.yaml files (default:
#                  examples/configs/install next to this script — NOTE these
#                  still hold placeholders; point --src at your
#                  secret-injected staging copies, or use --inject-from below)
#   --dest         cloud-init web root (default: /var/www/html/cloud-init)
#   --inject-from  optional per-host secrets.yaml (place-time injection, server-
#                  local only — there is NO HTTP secret-write API, by design).
#                  Fills each host's REPLACE_AT_PLACE_TIME slots from a
#                  mktemp staging copy of --src before the existing placement
#                  path runs, so every gate below (unknown host, missing
#                  source, leftover placeholder) still fires against the
#                  staged copy. Without this flag behavior is unchanged.
#
#                  Format — top-level unindented `host:` section headers,
#                  indented `key: value` lines beneath; values are copied
#                  VERBATIM after `key: ` (quotes included):
#
#                    # ~/uaa-secrets.yaml on the server — mode 0600,
#                    # NEVER inside a git tree
#                    len-serv-003:
#                      luks_key: the-real-passphrase
#                      root_password: "the-real-password"
#                      tpm2_pin: "12345678"
#                    unimatrixone:
#                      luks_key: ...
#
#                  Keep the secrets file in ~/ on the server, mode 0600,
#                  outside any git tree — this script refuses it otherwise.
#   host…          subset of hosts to place (default: all known hosts)
#
# Exit status: 0 if every requested host was placed; 1 if any host was refused
# (placeholder present, source missing, unknown host, or a refused secrets
# file when --inject-from is used).
#
# Deploy/use note (HUMAN step, do not automate): copy the updated script to
# the server and run it there, e.g.:
#   scp scripts/deploy-usb-configs.sh 172.16.2.30:~/
#   ssh 172.16.2.30
#   # on the server: keep ~/uaa-secrets.yaml at mode 0600, then:
#   ~/deploy-usb-configs.sh --inject-from ~/uaa-secrets.yaml [host ...]
# No service restart is involved for this script. (For the separate py-mirror
# service the standing note applies instead: `scp scripts/autoinstall-agent.py
# 172.16.2.30:/var/www/html/cloud-init/scripts/autoinstall-agent.py && ssh
# 172.16.2.30 'sudo systemctl restart autoinstall-agent'` — this script does
# not touch that file.)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SRC_DIR="${SCRIPT_DIR}/../examples/configs/install"
DEST_BASE="/var/www/html/cloud-init"
PLACEHOLDER="REPLACE_AT_PLACE_TIME"
SECRETS_FILE=""

# Staging copies (mktemp, 0600) are cleaned up on any exit path.
TMPFILES=""
trap 'rm -f $TMPFILES' EXIT

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
        --inject-from) SECRETS_FILE="$2"; shift 2 ;;
        -h|--help) grep '^#' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        -*) echo "ERROR: unknown flag: $1" >&2; exit 1 ;;
        *)  HOSTS+=("$1"); shift ;;
    esac
done
[ ${#HOSTS[@]} -gt 0 ] || HOSTS=($KNOWN_HOSTS)

if [ -n "$SECRETS_FILE" ]; then
    if [ ! -f "$SECRETS_FILE" ]; then
        echo "REFUSED: --inject-from file not found: $SECRETS_FILE" >&2
        exit 1
    fi
    # Secrets must be un-committable: refuse a secrets file living inside any
    # git work tree. If git itself is unavailable (server-side use), pass.
    secrets_dir="$(cd "$(dirname "$SECRETS_FILE")" && pwd)"
    if [ "$(git -C "$secrets_dir" rev-parse --is-inside-work-tree 2>/dev/null)" = "true" ]; then
        echo "REFUSED: --inject-from file is inside a git work tree: $SECRETS_FILE" >&2
        exit 1
    fi
    # Group/other must have NO access (0600 or stricter). Portable across
    # BSD (macOS) and GNU `ls -l` field layout: chars 5-10 of the long mode.
    perms="$(ls -ld "$SECRETS_FILE" | cut -c5-10)"
    if [ "$perms" != "------" ]; then
        echo "REFUSED: --inject-from file is group/world accessible (need mode 0600 or stricter): $SECRETS_FILE" >&2
        exit 1
    fi
fi

# Fill REPLACE_AT_PLACE_TIME slots in config $2 from section $3 of secrets file $1,
# writing to $4. Values never touch argv/logs; comment lines mentioning the token
# are dropped (they document the placeholder scheme; the committed examples carry
# one, which would otherwise trip the backstop gate on a fully-injected copy).
inject_secrets() {
    awk -v host="$3" '
        NR == FNR {
            if ($0 ~ /^[A-Za-z0-9_-]+:[[:space:]]*$/) {
                section = $0; sub(/:[[:space:]]*$/, "", section); next
            }
            if (section == host && $0 ~ /^[[:space:]]+[A-Za-z0-9_]+:[[:space:]]*[^[:space:]]/) {
                key = $1; sub(/:$/, "", key)
                val = $0; sub(/^[[:space:]]*[A-Za-z0-9_]+:[[:space:]]*/, "", val)
                secret[key] = val
            }
            next
        }
        /REPLACE_AT_PLACE_TIME/ {
            if ($0 ~ /^[[:space:]]*#/) next
            line_key = $1; sub(/:$/, "", line_key)
            if (line_key in secret) {
                indent = $0; sub(/[^[:space:]].*$/, "", indent)
                print indent line_key ": " secret[line_key]
                next
            }
            print; next
        }
        { print }
    ' "$1" "$2" > "$4"
}

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
    if [ -n "$SECRETS_FILE" ]; then
        staged="$(mktemp)"           # mktemp creates 0600 — no umask games needed
        TMPFILES="$TMPFILES $staged"
        inject_secrets "$SECRETS_FILE" "$src" "$host" "$staged"
        src="$staged"
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
