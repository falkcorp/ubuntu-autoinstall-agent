#!/bin/bash
# file: installer-image/uaa-autoinstall.sh
# version: 1.2.0
# guid: 4b8e1d02-6f3a-4c77-9e21-5a0c8b6d1f34
# last-edited: 2026-07-09
#
# Boot-time entrypoint for the custom ZFS-on-LUKS installer image (Option 2).
# iPXE boots the overlaid 26.04 live-server with a kernel cmdline like:
#
#   uaa.autoinstall uaa.config=http://172.16.2.30/cloud-init/len-serv-003.yaml \
#     uaa.on_done=poweroff ip=dhcp
#
# This script (run by uaa-autoinstall.service, gated on ConditionKernelCommandLine=
# uaa.autoinstall) reads uaa.config= from /proc/cmdline, fetches the per-host YAML,
# runs the agent's full ZFS-on-LUKS install, reports status, then powers off
# (loop-safe) or reboots per uaa.on_done.

set -uo pipefail

AGENT=/usr/local/bin/uaa
CONF=/run/uaa-config.yaml
REPORT_BASE="${UAA_REPORT_BASE:-http://172.16.2.30/cloud-init}"

log() { echo "[uaa-autoinstall] $*" | tee /dev/kmsg 2>/dev/null || echo "[uaa-autoinstall] $*"; }

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
        shell)   log "dropping to emergency shell for debugging"; systemctl default || exec /bin/bash ;;
        *)       log "powering off (loop-safe)"; systemctl poweroff ;;
    esac
}

finish_failure() {
    # Failure paths must NEVER reboot: with uaa.autoinstall still on the boot
    # cmdline a reboot lands right back here -> infinite fetch/wipe/reinstall
    # loop. Coerce reboot->poweroff; only 'shell' is honored as-is.
    local action="$1"
    case "$action" in
        shell) finish shell ;;
        *)     [ "$action" = reboot ] && log "on_done=reboot ignored on FAILURE (would loop) — powering off"
               finish poweroff ;;
    esac
}

ON_DONE="$(cmdline_value uaa.on_done || echo poweroff)"

CONFIG_URL="$(cmdline_value uaa.config || true)"
if [ -z "${CONFIG_URL}" ]; then
    # No explicit per-host config on the cmdline (e.g. a USB boot, which has no
    # per-MAC cmdline like netboot does). Fall back to the server's MAC-resolved
    # endpoint: the server maps OUR MAC (from its ARP/NDP neighbor table) to our
    # per-host InstallationConfig and serves it — exactly like the netboot
    # /autoinstall/ seed. One generic URL works for every machine; identity is the
    # wire MAC, no client self-reporting needed.
    CONFIG_URL="${UAA_CONFIG_RESOLVE_URL:-http://172.16.2.30:25000/autoinstall/uaa-config}"
    log "no uaa.config= on cmdline; using MAC-resolved config endpoint ${CONFIG_URL}"
fi

log "fetching per-host config: ${CONFIG_URL}"
if ! curl -fsSL --retry 5 --retry-delay 3 --max-time 60 "${CONFIG_URL}" -o "${CONF}"; then
    log "FAILED to fetch config from ${CONFIG_URL}"
    report_status failed "uaa-autoinstall: could not fetch ${CONFIG_URL}"
    # A config-fetch failure should NOT loop-reinstall — halt for inspection.
    finish_failure "${ON_DONE:-poweroff}"
    exit 1
fi

HOST="$(awk -F': *' '/^hostname:/ {print $2; exit}' "${CONF}" 2>/dev/null || echo unknown)"
log "starting ZFS-on-LUKS install for host '${HOST}'"
report_status running "uaa-autoinstall: installing ${HOST}"

if "${AGENT}" install --config "${CONF}"; then
    log "install completed OK for ${HOST}"
    report_status success "uaa-autoinstall: ${HOST} installed"
    finish "${ON_DONE}"
else
    rc=$?
    log "installer exited ${rc} for ${HOST}"
    report_status failed "uaa-autoinstall: ${HOST} install failed (rc=${rc})"
    # Do NOT reboot-loop on a broken install; power off (or drop to shell) so an
    # operator can inspect. Override with uaa.on_done=shell for interactive debug.
    finish_failure "${ON_DONE:-poweroff}"
    exit "${rc}"
fi
