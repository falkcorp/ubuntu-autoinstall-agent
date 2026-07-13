#!/usr/bin/env bash
# file: scripts/server-deploy.sh
# version: 1.2.0
# guid: 9e2f4a71-0b6d-4c8e-9a3f-1d5c7e8b2f60
# last-edited: 2026-07-13
#
# Repeatable build+deploy for the uaa constellation control daemon on the server
# (172.16.2.30). Lives in the repo so `git pull` always ships the latest version of
# itself. Intended to be run BY THE OPERATOR on the server, not by an agent — the
# uaa-control systemd units already say so (crates/uaa-control/systemd/*.service),
# this script just automates the human steps described there.
#
# First run (once, needs your sudo password):
#   sudo ./scripts/server-deploy.sh --bootstrap
#     - creates the `uaa` system user/group + /var/lib/uaa
#     - installs /etc/sudoers.d/uaa-deploy (NOPASSWD for the exact commands below,
#       so every later run of this script needs no password)
#     - installs the uaa-control systemd units + daemon-reload
#
# Every later run (no sudo password needed):
#   ./scripts/server-deploy.sh              # git pull, cargo build --release, stage
#                                            # binaries, restart uaa-control.service
#                                            # only (does NOT touch :25000 traffic)
#   ./scripts/server-deploy.sh --status      # show both services + health endpoints
#   ./scripts/server-deploy.sh --cutover     # stop the Python autoinstall-agent and
#                                            # hand :25000 to uaa-control.socket
#                                            # (auto-rolls back if the health check
#                                            # fails). This is the one irreversible-
#                                            # feeling step; everything else is safe
#                                            # to re-run at will.
#   ./scripts/server-deploy.sh --rollback    # reverse --cutover
#
# Why the default run does not start uaa-control.service before --cutover:
# uaa-control.service has `Requires=uaa-control.socket` (crates/uaa-control/systemd),
# so starting the service always pulls in the socket first. Pre-cutover the socket's
# :25000 is already held by the Python autoinstall-agent.service, so the socket job
# fails outright — not a crash-loop, systemd just refuses the dependency and the
# Python service is never touched. The default run detects this (socket inactive) and
# leaves uaa-control stopped after staging the binary; nothing to fix, that's expected
# until you run --cutover. See crates/uaa-control/src/listeners.rs for the bind logic.

set -euo pipefail

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
UNIT_SERVICE="$REPO_DIR/crates/uaa-control/systemd/uaa-control.service"
UNIT_SOCKET="$REPO_DIR/crates/uaa-control/systemd/uaa-control.socket"
SUDOERS_FILE="/etc/sudoers.d/uaa-deploy"
BIN_UAA_CONTROL="$REPO_DIR/target/release/uaa-control"
BIN_UAA_AGENT="$REPO_DIR/target/release/ubuntu-autoinstall-agent"

usage() {
    sed -n '2,40p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
}

log() { echo "==> $*"; }

require_root() {
    if [[ "$(id -u)" -ne 0 ]]; then
        echo "ERROR: --bootstrap must be run with sudo (one-time root setup)." >&2
        exit 1
    fi
}

bootstrap() {
    require_root

    log "ensuring uaa system group/user"
    getent group uaa >/dev/null || groupadd --system uaa
    getent passwd uaa >/dev/null || useradd --system --gid uaa --home-dir /var/lib/uaa \
        --shell /usr/sbin/nologin --comment "uaa constellation control daemon" uaa

    log "ensuring /var/lib/uaa (registry snapshot + WAL state dir)"
    install -d -o uaa -g uaa -m 0750 /var/lib/uaa

    log "installing systemd units"
    cp "$UNIT_SERVICE" /etc/systemd/system/uaa-control.service
    cp "$UNIT_SOCKET" /etc/systemd/system/uaa-control.socket
    systemctl daemon-reload

    log "installing $SUDOERS_FILE"
    local tmp
    tmp="$(mktemp)"
    cat >"$tmp" <<EOF
# file: $SUDOERS_FILE
# Installed by scripts/server-deploy.sh --bootstrap. Scopes NOPASSWD to the exact
# commands the repeatable deploy run needs, matching the pattern already used for
# audiobook-organizer on this host. jdfalk already has unrestricted (ALL:ALL) ALL
# sudo with a password; this only removes the password prompt for routine re-deploys.
jdfalk ALL=(root) NOPASSWD: /usr/bin/install -o root -g root -m 0755 $BIN_UAA_CONTROL /usr/local/bin/uaa-control
jdfalk ALL=(root) NOPASSWD: /usr/bin/install -o root -g root -m 0755 $BIN_UAA_AGENT /usr/local/bin/ubuntu-autoinstall-agent
jdfalk ALL=(root) NOPASSWD: /usr/bin/cp $UNIT_SERVICE /etc/systemd/system/uaa-control.service
jdfalk ALL=(root) NOPASSWD: /usr/bin/cp $UNIT_SOCKET /etc/systemd/system/uaa-control.socket
jdfalk ALL=(root) NOPASSWD: /usr/bin/systemctl daemon-reload
jdfalk ALL=(root) NOPASSWD: /usr/bin/systemctl start uaa-control.service, /usr/bin/systemctl stop uaa-control.service, /usr/bin/systemctl restart uaa-control.service, /usr/bin/systemctl status uaa-control.service
jdfalk ALL=(root) NOPASSWD: /usr/bin/systemctl start uaa-control.socket, /usr/bin/systemctl stop uaa-control.socket, /usr/bin/systemctl restart uaa-control.socket, /usr/bin/systemctl status uaa-control.socket
jdfalk ALL=(root) NOPASSWD: /usr/bin/systemctl enable uaa-control.service, /usr/bin/systemctl disable uaa-control.service, /usr/bin/systemctl enable uaa-control.socket, /usr/bin/systemctl disable uaa-control.socket
jdfalk ALL=(root) NOPASSWD: /usr/bin/journalctl -u uaa-control.service, /usr/bin/journalctl -fu uaa-control.service
EOF
    visudo -cf "$tmp"
    install -o root -g root -m 0440 "$tmp" "$SUDOERS_FILE"
    rm -f "$tmp"

    log "bootstrap complete. Run './scripts/server-deploy.sh' (no sudo) from now on."
}

pull_latest() {
    if [[ -n "$(git -C "$REPO_DIR" status --porcelain)" ]]; then
        log "WARNING: $REPO_DIR has local changes, skipping git pull"
        return
    fi
    log "git pull origin main"
    git -C "$REPO_DIR" fetch origin main
    git -C "$REPO_DIR" checkout main
    git -C "$REPO_DIR" pull --ff-only origin main
}

build() {
    log "cargo build --release -p uaa-control -p uaa"
    ( cd "$REPO_DIR" && cargo build --release -p uaa-control -p uaa )
}

stage() {
    log "staging binaries + systemd units"
    sudo install -o root -g root -m 0755 "$BIN_UAA_CONTROL" /usr/local/bin/uaa-control
    sudo install -o root -g root -m 0755 "$BIN_UAA_AGENT" /usr/local/bin/ubuntu-autoinstall-agent
    sudo cp "$UNIT_SERVICE" /etc/systemd/system/uaa-control.service
    sudo cp "$UNIT_SOCKET" /etc/systemd/system/uaa-control.socket
    sudo systemctl daemon-reload
}

restart_control() {
    if systemctl is-active --quiet uaa-control.socket; then
        log "restarting uaa-control.service (socket already active — post-cutover)"
        sudo systemctl restart uaa-control.service
        sleep 1
        systemctl status uaa-control.service || true
    else
        log "uaa-control.socket not active yet (pre-cutover) — binary staged, service left stopped"
        log "run './scripts/server-deploy.sh --cutover' when ready to hand :25000 to uaa-control"
    fi
}

status() {
    echo "--- autoinstall-agent.service (legacy Python, owns :25000 until --cutover) ---"
    systemctl status autoinstall-agent.service || true
    echo
    echo "--- uaa-control.service ---"
    systemctl status uaa-control.service || true
    echo
    echo "--- uaa-control.socket ---"
    systemctl status uaa-control.socket || true
    echo
    echo "--- health checks ---"
    for port in 25000 7443 7444 15001; do
        curl -s -m 2 "http://127.0.0.1:${port}/healthz" && echo || echo "port ${port}: no response"
    done
}

cutover() {
    log "cutover: stopping Python autoinstall-agent.service, handing :25000 to uaa-control.socket"
    sudo systemctl stop autoinstall-agent.service
    sudo systemctl enable uaa-control.socket
    sudo systemctl start uaa-control.socket
    sudo systemctl restart uaa-control.service
    sleep 1
    if curl -s -m 3 -o /dev/null -w '%{http_code}' http://127.0.0.1:25000/healthz | grep -qE '^[23]'; then
        log "cutover OK: :25000 answering via uaa-control"
    else
        log "cutover health check FAILED — rolling back to Python"
        rollback
        exit 1
    fi
}

rollback() {
    log "rollback: stopping uaa-control.socket, restarting Python autoinstall-agent.service"
    sudo systemctl stop uaa-control.socket 2>/dev/null || true
    sudo systemctl disable uaa-control.socket 2>/dev/null || true
    sudo systemctl start autoinstall-agent.service
}

case "${1:-}" in
    --bootstrap) bootstrap ;;
    --status) status ;;
    --cutover) cutover ;;
    --rollback) rollback ;;
    -h|--help) usage ;;
    "")
        pull_latest
        build
        stage
        restart_control
        ;;
    *)
        echo "unknown option: $1" >&2
        usage
        exit 1
        ;;
esac
