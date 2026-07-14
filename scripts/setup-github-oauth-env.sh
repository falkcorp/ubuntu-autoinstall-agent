#!/usr/bin/env bash
# file: scripts/setup-github-oauth-env.sh
# version: 1.0.0
# guid: 7c3f9a41-2e6d-4b8a-9c15-3a8f6d2b91e4
# last-edited: 2026-07-14
#
# Provisions /etc/uaa/uaa-control.env (root:root 600) with the GitHub OAuth
# app's credentials for uaa-control's operator-plane auth (CT-03), then
# restarts the service so it picks them up. Run with sudo on the host that
# runs uaa-control.service (172.16.2.30 / U0).
#
# Usage: sudo bash scripts/setup-github-oauth-env.sh

set -euo pipefail

if [ "$(id -u)" -ne 0 ]; then
  echo "ERROR: must run as root (sudo bash $0)" >&2
  exit 1
fi

ENV_DIR=/etc/uaa
ENV_FILE="${ENV_DIR}/uaa-control.env"
UNIT_FILE=/etc/systemd/system/uaa-control.service

CLIENT_ID="Ov23li3vx528yQZQAeBy"
ORG="falkcorp"
ADMIN_TEAM="uaa-admins"
OPERATOR_TEAM="uaa-operators"

echo "GitHub OAuth client secret (input hidden, paste and press enter):"
read -r -s CLIENT_SECRET
echo
if [ -z "${CLIENT_SECRET}" ]; then
  echo "ERROR: empty secret, aborting" >&2
  exit 1
fi

mkdir -p "${ENV_DIR}"
chmod 700 "${ENV_DIR}"

umask 077
cat > "${ENV_FILE}" <<EOF
UAA_GITHUB_CLIENT_ID=${CLIENT_ID}
UAA_GITHUB_CLIENT_SECRET=${CLIENT_SECRET}
UAA_GITHUB_ORG=${ORG}
UAA_GITHUB_ADMIN_TEAM=${ADMIN_TEAM}
UAA_GITHUB_OPERATOR_TEAM=${OPERATOR_TEAM}
EOF
chown root:root "${ENV_FILE}"
chmod 600 "${ENV_FILE}"
echo "wrote ${ENV_FILE} (mode 600, root:root)"

if [ -f "${UNIT_FILE}" ] && ! grep -q '^EnvironmentFile=.*uaa-control\.env' "${UNIT_FILE}"; then
  sed -i.bak '/^\[Service\]/a EnvironmentFile=-/etc/uaa/uaa-control.env' "${UNIT_FILE}"
  echo "patched ${UNIT_FILE} to load ${ENV_FILE} (backup at ${UNIT_FILE}.bak)"
elif [ -f "${UNIT_FILE}" ]; then
  echo "${UNIT_FILE} already references uaa-control.env, leaving as-is"
else
  echo "WARNING: ${UNIT_FILE} not found; systemd unit not patched" >&2
fi

systemctl daemon-reload
systemctl restart uaa-control
sleep 2
systemctl --no-pager status uaa-control | head -10
echo
echo "Recent log lines:"
journalctl -u uaa-control -n 15 --no-pager
