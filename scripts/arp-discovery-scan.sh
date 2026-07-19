#!/usr/bin/env bash
# file: scripts/arp-discovery-scan.sh
# version: 1.1.0
# guid: 8b1f4a37-2d90-4e6c-9a58-3f0c7b6e15d4
# last-edited: 2026-07-19
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

post_mac() {
    local mac="$1" now prev
    now="$(date +%s)"
    prev="${last_post[$mac]:-0}"
    (( now - prev < TTL )) && return 1
    if curl -fsS -m 3 -X POST -H 'Content-Type: application/json' \
        --data "{\"mac\":\"${mac}\"}" "$INGEST_URL" >/dev/null 2>&1; then
        last_post[$mac]="$now"
        return 0
    fi
    log "POST failed for ${mac} (will retry next scan)"
    return 2
}

log "scanning ip neigh → ${INGEST_URL} every ${INTERVAL}s (throttle ${TTL}s/MAC)"

while true; do
    scanned=0 posted=0
    # Only entries the kernel has an lladdr for and considers valid; FAILED /
    # INCOMPLETE carry no usable MAC. Covers IPv4 ARP and IPv6 NDP alike; a
    # device present under both collapses to one MAC in the inbox.
    while read -r mac; do
        [[ -z "$mac" ]] && continue
        scanned=$(( scanned + 1 ))
        if post_mac "${mac,,}"; then
            posted=$(( posted + 1 ))
        fi
    done < <(ip neigh show 2>/dev/null \
        | grep -iwE 'REACHABLE|STALE|DELAY|PROBE' \
        | grep -ioE 'lladdr ([0-9a-f]{2}:){5}[0-9a-f]{2}' \
        | awk '{print $2}' \
        | sort -u)

    # One summary line per scan so the journal shows it working. "newly posted"
    # drops to ~0 after the first scan (throttle window) — that is correct, not a
    # failure; "tracked total" is the count of distinct MACs recorded so far.
    log "scan: ${scanned} neighbors, ${posted} newly posted, ${#last_post[@]} tracked total"
    sleep "$INTERVAL"
done
