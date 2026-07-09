#!/usr/bin/env bash
# file: scripts/capture-uni-boot.sh
# version: 1.2.0
# guid: 3e2a7c41-9f8d-4b6e-8a1c-5d0f6b2e7a93
# last-edited: 2026-07-08

# Run this ON THE SERVER (172.16.2.30) as jdfalk, with sudo available for the
# tcpdump capture. It starts a tcpdump capture, a dnsmasq journal tail, and an
# nginx access.log tail all in parallel, tagged with a shared timestamp, and
# stops everything cleanly on Ctrl+C.
#
# Usage:
#   ./capture-uni-boot.sh
# Then power on unimatrixone (from wherever you run ipmitool) and watch your
# own SOL session. Press Ctrl+C here once the attempt is done (success, stall,
# or timeout) to stop all captures and print a summary.
#
# Captures run under nohup so a dropped SSH session (which killed a plain
# `journalctl -f` tail last time) won't lose them — reconnect and check
# ~/uni-boot-capture/, or `kill $(cat pids-<stamp>.txt)` to stop manually.

set -euo pipefail

IFACE="enp8s0f0"
MAC="ac:1f:6b:40:fc:e2"
OUTDIR="${HOME}/uni-boot-capture"
STAMP="$(date +%Y%m%d-%H%M%S)"
PCAP="${OUTDIR}/uni-ipv6-boot-${STAMP}.pcap"
DNSMASQ_LOG="${OUTDIR}/dnsmasq-${STAMP}.log"
NGINX_LOG="${OUTDIR}/nginx-access-${STAMP}.log"

mkdir -p "${OUTDIR}"

echo "==> Capturing to ${OUTDIR} (stamp: ${STAMP})"
echo "==> Starting tcpdump (needs sudo password)..."
sudo -v

nohup sudo tcpdump -i "${IFACE}" -w "${PCAP}" \
  "ether host ${MAC} or icmp6" > "${OUTDIR}/tcpdump-stderr-${STAMP}.log" 2>&1 &
TCPDUMP_PID=$!

nohup journalctl -u dnsmasq -f --since "now" > "${DNSMASQ_LOG}" 2>&1 &
DNSMASQ_PID=$!

nohup tail -F /var/log/nginx/access.log > "${NGINX_LOG}" 2>&1 &
NGINX_PID=$!

echo "${TCPDUMP_PID} ${DNSMASQ_PID} ${NGINX_PID}" > "${OUTDIR}/pids-${STAMP}.txt"

# Give tcpdump a moment to open the interface and confirm it's still alive
# before we tell the user it's safe to power on.
sleep 1
if ! kill -0 "${TCPDUMP_PID}" 2>/dev/null; then
  echo "==> tcpdump died immediately, see ${OUTDIR}/tcpdump-stderr-${STAMP}.log:"
  cat "${OUTDIR}/tcpdump-stderr-${STAMP}.log"
  exit 1
fi

cleanup() {
  trap - EXIT INT TERM
  echo ""
  echo "==> Stopping captures..."
  sudo kill -INT "${TCPDUMP_PID}" 2>/dev/null || true
  kill "${DNSMASQ_PID}" "${NGINX_PID}" 2>/dev/null || true
  wait "${TCPDUMP_PID}" 2>/dev/null || true
  wait "${DNSMASQ_PID}" 2>/dev/null || true
  wait "${NGINX_PID}" 2>/dev/null || true

  echo ""
  echo "==> Summary"
  echo "pcap:    ${PCAP}"
  echo "dnsmasq: ${DNSMASQ_LOG}"
  echo "nginx:   ${NGINX_LOG}"
  echo ""
  echo "-- packets involving ${MAC} or icmp6 --"
  sudo tcpdump -r "${PCAP}" -n 2>/dev/null | head -50 || echo "(no packets captured, or tcpdump -r failed)"
  echo ""
  echo "-- dnsmasq log lines --"
  wc -l "${DNSMASQ_LOG}"
  echo ""
  echo "-- nginx access log lines --"
  wc -l "${NGINX_LOG}"
}
trap cleanup EXIT INT TERM

echo "==> Running. Power on unimatrixone now and watch your SOL session."
echo "==> Press Ctrl+C here when the boot attempt is done."
wait "${TCPDUMP_PID}"
