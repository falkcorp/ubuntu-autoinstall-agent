#!/usr/bin/env bash
# file: scripts/arp-discovery-scan.sh
# version: 1.2.1
# guid: 8b1f4a37-2d90-4e6c-9a58-3f0c7b6e15d4
# last-edited: 2026-07-22
#
# Passive segment discovery scanner for the server (172.16.2.30).
#
# The server sits on 172.16.2.0/23, so its kernel neighbor (ARP/NDP) table
# accumulates an entry for every device that communicates on the segment —
# "everything that ARPs." (dnsmasq runs in proxy-DHCP mode and, verified on the
# live box, does NOT log client MACs for non-PXE clients, so its journal is NOT
# a usable capture source — the neighbor table is.)
#
# This polls `ip neigh` on an interval and POSTs each resolved MAC to the
# uaa-control discovery inbox (`POST /api/discovered` on :25000, unauthenticated
# machine plane), where it lands in discovered-macs.json and surfaces on the
# operator SPA's Discovery page for triage. This is the "track EVERYTHING that
# ARPs/DHCPs" capture path, distinct from uaa-control's reactive
# `record_seen_mac` (which only fires on an autoinstall HTTP fetch).
#
# Run via uaa-arp-discovery.service. Needs no privilege (reads the neighbor
# table, curls localhost).

set -euo pipefail

INGEST_URL="${UAA_DISCOVERED_URL:-http://127.0.0.1:25000/api/discovered}"
# Seconds between neighbor-table scans.
INTERVAL="${UAA_DISCOVERED_INTERVAL:-30}"
# Per-MAC re-POST throttle (seconds). record() is idempotent, but each POST
# rewrites discovered-macs.json, so a MAC already reported inside this window is
# skipped; outside it, it re-POSTs to refresh last_seen. Default 10 min.
TTL="${UAA_DISCOVERED_TTL:-600}"

log() { echo "arp-discovery-scan: $*" >&2; }

declare -A last_post

# A minimal JSON string-escaper (hostnames/IPs are tame, but never trust input).
json_str() { printf '%s' "$1" | sed 's/\\/\\\\/g; s/"/\\"/g'; }

# post_mac <mac> <ip> <hostname>  — ip/hostname may be empty.
post_mac() {
    local mac="$1" ip="$2" host="$3" now prev body
    now="$(date +%s)"
    prev="${last_post[$mac]:-0}"
    (( now - prev < TTL )) && return 1
    body="{\"mac\":\"$(json_str "$mac")\""
    [[ -n "$ip"   ]] && body+=",\"ip\":\"$(json_str "$ip")\""
    [[ -n "$host" ]] && body+=",\"hostname\":\"$(json_str "$host")\""
    body+="}"
    if curl -fsS -m 3 -X POST -H 'Content-Type: application/json' \
        --data "$body" "$INGEST_URL" >/dev/null 2>&1; then
        last_post[$mac]="$now"
        return 0
    fi
    log "POST failed for ${mac} (will retry next scan)"
    return 2
}

log "scanning ip neigh → ${INGEST_URL} every ${INTERVAL}s (throttle ${TTL}s/MAC)"

while true; do
    scanned=0 posted=0 named=0
    # Pass 1: collapse each MAC to one IP, PREFERRING IPv4 (IPv6 link-local
    # fe80:: addresses don't resolve to a hostname). FAILED/INCOMPLETE entries
    # have no lladdr and are skipped.
    declare -A mac_ip
    while read -r line; do
        [[ -z "$line" ]] && continue
        # `|| true`: grep exits 1 on no-match, which under `set -e`+pipefail in a
        # command substitution would kill the whole scanner. Same for getent below.
        local_ip="$(awk '{print $1}' <<<"$line" || true)"
        local_mac="$(grep -ioE 'lladdr ([0-9a-f]{2}:){5}[0-9a-f]{2}' <<<"$line" | awk '{print $2}' || true)"
        [[ -z "$local_mac" ]] && continue
        local_mac="${local_mac,,}"
        case "$local_ip" in
            *:*) [[ -z "${mac_ip[$local_mac]:-}" ]] && mac_ip[$local_mac]="$local_ip" ;; # IPv6: only if none yet
            *)   mac_ip[$local_mac]="$local_ip" ;;                                        # IPv4: always wins
        esac
    done < <(ip neigh show 2>/dev/null | grep -iwE 'REACHABLE|STALE|DELAY|PROBE')

    # Pass 2: resolve each MAC's IP to a hostname and post.
    for mac in "${!mac_ip[@]}"; do
        scanned=$(( scanned + 1 ))
        ip="${mac_ip[$mac]}"
        # getent reads /etc/hosts + DNS (so 172.16.2.45 -> rpi-serv-001.local);
        # strip a trailing .local for a clean name. Empty for unidentified devices.
        # getent exits 2 on not-found, so `|| true` keeps `set -e` from killing us.
        host="$(getent hosts "$ip" 2>/dev/null | awk '{print $2}' || true)"
        host="${host%.local}"
        [[ -n "$host" ]] && named=$(( named + 1 ))
        if post_mac "$mac" "$ip" "$host"; then
            posted=$(( posted + 1 ))
        fi
    done
    unset mac_ip

    # One summary line per scan. "named" = devices resolved to a hostname (the
    # fleet); the rest are unidentified consumer devices kept out of Machines.
    log "scan: ${scanned} neighbors, ${named} named, ${posted} newly posted, ${#last_post[@]} tracked total"
    sleep "$INTERVAL"
done
