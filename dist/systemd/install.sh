#!/usr/bin/env bash
set -euo pipefail

# Install zerochain as a systemd user service.

UNIT_NAME="zerochain.service"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
CONFIG_DIR="${HOME}/config/zerochain"
SYSTEMD_DIR="${HOME}/.config/systemd/user"

echo "==> Installing ${UNIT_NAME}..."

mkdir -p "${SYSTEMD_DIR}"
mkdir -p "${CONFIG_DIR}/workspace"

if [ ! -f "${CONFIG_DIR}/env" ]; then
    touch "${CONFIG_DIR}/env"
    echo "    Created empty ${CONFIG_DIR}/env — add API keys there"
fi

cp "${SCRIPT_DIR}/${UNIT_NAME}" "${SYSTEMD_DIR}/${UNIT_NAME}"
systemctl --user daemon-reload
systemctl --user enable --now "${UNIT_NAME}"

echo "==> Done. Status:"
systemctl --user status "${UNIT_NAME}" --no-pager || true
echo ""
echo "    Logs:    journalctl --user -u zerochain -f"
echo "    Config:  ${CONFIG_DIR}/env"
echo "    Data:    ${CONFIG_DIR}/workspace/"
