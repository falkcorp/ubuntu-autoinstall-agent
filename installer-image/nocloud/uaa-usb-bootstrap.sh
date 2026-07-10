#!/bin/bash
# file: installer-image/nocloud/uaa-usb-bootstrap.sh
# version: 1.0.0
# guid: 9e4c7a20-1b5f-4d38-8a6e-2c0d9f471b63
# last-edited: 2026-07-09
#
# USB auto-bootstrap: turns the SSH-ready remastered Ubuntu Server USB into a
# zero-touch installer, mirroring the netboot flow. Runs from the LIVE session
# (cloud-init runcmd via make-ssh-ready-iso.sh --autoinstall) ONLY when the
# kernel cmdline carries `uaa.autoinstall`. On boot it:
#   1. curls the static agent binary from the server -> /usr/local/bin/uaa
#   2. fetches the per-host InstallationConfig: uaa.config= from the cmdline if
#      present, else the server's MAC-resolved endpoint (identity = wire MAC,
#      same as the netboot cloud-init seed) -> /run/uaa-config.yaml
#   3. runs `uaa install --config /run/uaa-config.yaml --report-url ...`
#   4. on success, best-effort efibootmgr: BootOrder = network #1, ubuntu #2
#   5. reports status and powers off (loop-safe). A config-fetch or install
#      FAILURE never reboot-loops: poweroff, or shell if uaa.on_done=shell.
#
# Tunables (env, or uaa.* kernel cmdline tokens where noted):
#   UAA_AGENT_URL           agent binary URL   (default http://172.16.2.30/uaa/uaa-amd64)
#   UAA_CONFIG_RESOLVE_URL  MAC-resolved config endpoint
#                           (default http://172.16.2.30:25000/autoinstall/uaa-config)
#   UAA_REPORT_URL          install status webhook
#                           (default http://172.16.2.30:25000/api/webhook)
#   uaa.config=<url>        explicit per-host config URL (cmdline)
#   uaa.on_done=poweroff|reboot|shell   post-run action (cmdline, default poweroff)

set -uo pipefail

AGENT=/usr/local/bin/uaa
CONF=/run/uaa-config.yaml
AGENT_URL="${UAA_AGENT_URL:-http://172.16.2.30/uaa/uaa-amd64}"
REPORT_URL="${UAA_REPORT_URL:-http://172.16.2.30:25000/api/webhook}"
REPORT_BASE="${UAA_REPORT_BASE:-http://172.16.2.30/cloud-init}"

log() { echo "[uaa-usb-bootstrap] $*" | tee /dev/kmsg 2>/dev/null || echo "[uaa-usb-bootstrap] $*"; }

# Parse a key=value token out of the kernel command line.
cmdline_value() {
    local key="$1" tok
    for tok in $(cat /proc/cmdline); do
        case "$tok" in
            "${key}="*) printf '%s' "${tok#${key}=}"; return 0 ;;
        esac
    done
    return 1
}

report_status() {
    # Best-effort status ping; never fatal.
    local state="$1" msg="$2"
    curl -fsSL --max-time 10 "${REPORT_BASE}/reporting.sh" -o /run/reporting.sh 2>/dev/null \
        && bash -c "source /run/reporting.sh && send_status_update '${state}' 0 '${msg}'" 2>/dev/null \
        || true
}

finish() {
    # $1 = poweroff|reboot|shell  (default from uaa.on_done, else poweroff)
    local action="$1"
    case "$action" in
        reboot)  log "rebooting"; systemctl reboot ;;
        shell)   log "leaving live session up for debugging (SSH-ready)"; ;;
        *)       log "powering off (loop-safe)"; systemctl poweroff ;;
    esac
}

# Best-effort UEFI boot order: network entries first, ubuntu second, rest after.
# Non-fatal by design — legacy-BIOS hosts (e.g. U1's legacy-OpROM IMSM array)
# have no EFI variables and efibootmgr just fails; that's fine.
set_boot_order() {
    command -v efibootmgr >/dev/null 2>&1 || { log "efibootmgr not present; skipping boot order"; return 0; }
    local entries net ubuntu rest order
    entries="$(efibootmgr 2>/dev/null)" || { log "efibootmgr unreadable (legacy BIOS?); skipping boot order"; return 0; }
    net="$(echo "$entries"    | sed -n 's/^Boot\([0-9A-Fa-f]\{4\}\)\*\{0,1\}[[:space:]].*\(PXE\|[Nn]etwork\|IPv[46]\).*/\1/p' | tr '\n' ',' )"
    ubuntu="$(echo "$entries" | sed -n 's/^Boot\([0-9A-Fa-f]\{4\}\)\*\{0,1\}[[:space:]][Uu]buntu.*/\1/p' | tr '\n' ',' )"
    rest="$(echo "$entries"   | sed -n 's/^Boot\([0-9A-Fa-f]\{4\}\)\*\{0,1\}[[:space:]].*/\1/p' | tr '\n' ',')"
    # Compose net,ubuntu,then remaining entries (dedup, preserve order).
    order="$(echo "${net}${ubuntu}${rest}" | tr ',' '\n' | grep -v '^$' | awk '!seen[$0]++' | paste -sd, -)"
    [ -n "$order" ] || { log "no EFI boot entries found; skipping boot order"; return 0; }
    if efibootmgr -o "$order" >/dev/null 2>&1; then
        log "UEFI BootOrder set: $order (network first, ubuntu second)"
    else
        log "efibootmgr -o $order failed (non-fatal)"
    fi
    return 0
}

ON_DONE="$(cmdline_value uaa.on_done || echo poweroff)"

# ── 1. fetch the static agent ────────────────────────────────────────────────
log "fetching agent binary: ${AGENT_URL}"
if ! curl -fsSL --retry 5 --retry-delay 3 --max-time 120 "${AGENT_URL}" -o "${AGENT}"; then
    log "FAILED to fetch agent from ${AGENT_URL}"
    report_status failed "uaa-usb-bootstrap: could not fetch agent ${AGENT_URL}"
    finish "${ON_DONE}"
    exit 1
fi
chmod +x "${AGENT}"

# ── 2. fetch the per-host config (cmdline override, else MAC-resolved) ──────
CONFIG_URL="$(cmdline_value uaa.config || true)"
if [ -z "${CONFIG_URL}" ]; then
    # No explicit per-host config on the cmdline (the USB is generic — unlike
    # netboot there is no per-MAC cmdline). Use the server's MAC-resolved
    # endpoint: it maps OUR wire MAC (from its ARP/NDP neighbor table) to the
    # per-host uaa.yaml. One USB works for every machine.
    CONFIG_URL="${UAA_CONFIG_RESOLVE_URL:-http://172.16.2.30:25000/autoinstall/uaa-config}"
    log "no uaa.config= on cmdline; using MAC-resolved endpoint ${CONFIG_URL}"
fi
log "fetching per-host config: ${CONFIG_URL}"
if ! curl -fsSL --retry 5 --retry-delay 3 --max-time 60 "${CONFIG_URL}" -o "${CONF}"; then
    log "FAILED to fetch config from ${CONFIG_URL}"
    report_status failed "uaa-usb-bootstrap: could not fetch ${CONFIG_URL}"
    # A config-fetch failure must NOT loop-reinstall — halt for inspection.
    finish "${ON_DONE}"
    exit 1
fi

HOST="$(awk -F': *' '/^hostname:/ {print $2; exit}' "${CONF}" 2>/dev/null || echo unknown)"

# ── 3. run the install ───────────────────────────────────────────────────────
log "starting ZFS-on-LUKS install for host '${HOST}'"
report_status running "uaa-usb-bootstrap: installing ${HOST}"

if "${AGENT}" install --config "${CONF}" --report-url "${REPORT_URL}"; then
    log "install completed OK for ${HOST}"
    # ── 4. best-effort boot order (network #1, ubuntu #2) ───────────────────
    set_boot_order
    # ── 5. report + poweroff (loop-safe) ────────────────────────────────────
    report_status success "uaa-usb-bootstrap: ${HOST} installed"
    finish "${ON_DONE}"
else
    rc=$?
    log "installer exited ${rc} for ${HOST}"
    report_status failed "uaa-usb-bootstrap: ${HOST} install failed (rc=${rc})"
    # Do NOT reboot-loop on a broken install; power off (or stay up with
    # uaa.on_done=shell) so an operator can inspect over SSH.
    finish "${ON_DONE}"
    exit "${rc}"
fi
