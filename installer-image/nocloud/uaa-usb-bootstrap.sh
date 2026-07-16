#!/bin/bash
# file: installer-image/nocloud/uaa-usb-bootstrap.sh
# version: 1.2.0
# guid: 9e4c7a20-1b5f-4d38-8a6e-2c0d9f471b63
# last-edited: 2026-07-14
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
#   5. uploads final logs (dmesg + journalctl + every /var/log/*.log, via
#      reporting.sh's upload_logs — see upload_final_logs below), reports
#      status, and powers off (loop-safe). A config-fetch or install FAILURE
#      never reboot-loops: poweroff, or shell if uaa.on_done=shell.
#
# 2026-07-14: added upload_final_logs() on every terminal path (agent-fetch
# failure, config-fetch failure, install success, install failure).
# reporting.sh's upload_logs() already existed and already worked server-side
# — this script just never called it, so every failure required SSHing into
# the live session to read /var/log/cloud-init-output.log by hand. Once this
# is verified working, `--on-done shell` should go back to the poweroff
# default: the logs are on the server either way now, no need to hold the
# session open for physical/SSH access.
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

upload_final_logs() {
    # Best-effort: push dmesg + journalctl + every /var/log/*.log (including
    # cloud-init-output.log, where the actual `uaa install` error lands) to
    # the server as webhook file attachments, so a failure is debuggable from
    # the server without SSHing into the live session. Called on every
    # terminal path — success or failure — not just failure, since "what
    # actually happened" is worth having on disk either way.
    log "uploading final logs (dmesg, journalctl, /var/log/*.log)"
    curl -fsSL --max-time 10 "${REPORT_BASE}/reporting.sh" -o /run/reporting.sh 2>/dev/null \
        && bash -c "source /run/reporting.sh && upload_logs" 2>/dev/null \
        || log "could not fetch reporting.sh / upload final logs (best-effort, continuing)"
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

finish_failure() {
    # Failure paths must NEVER reboot: uaa.on_done=reboot is baked into the USB
    # cmdline, so a reboot lands right back in this bootstrap -> infinite
    # fetch/wipe/reinstall loop. Coerce reboot->poweroff; only 'shell' (stay up
    # for SSH inspection) is honored as-is.
    local action="$1"
    case "$action" in
        shell) finish shell ;;
        *)     [ "$action" = reboot ] && log "on_done=reboot ignored on FAILURE (would loop) — powering off"
               finish poweroff ;;
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
    upload_final_logs
    finish_failure "${ON_DONE}"
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
    upload_final_logs
    # A config-fetch failure must NOT loop-reinstall — halt for inspection.
    finish_failure "${ON_DONE}"
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
    # ── 5. upload logs + report + poweroff (loop-safe) ──────────────────────
    upload_final_logs
    report_status success "uaa-usb-bootstrap: ${HOST} installed"
    finish "${ON_DONE}"
else
    rc=$?
    log "installer exited ${rc} for ${HOST}"
    report_status failed "uaa-usb-bootstrap: ${HOST} install failed (rc=${rc})"
    upload_final_logs
    # Do NOT reboot-loop on a broken install; power off (or stay up with
    # uaa.on_done=shell) so an operator can inspect over SSH.
    finish_failure "${ON_DONE}"
    exit "${rc}"
fi
