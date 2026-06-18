#!/usr/bin/env bash
# install-cron.sh — idempotent installer for consign-drain systemd-user timer
# Usage: bash contrib/install-cron.sh
# Run from any directory; paths are resolved relative to script location.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
UNIT_SRC_DIR="${SCRIPT_DIR}/../contrib"
SYSTEMD_USER_DIR="${HOME}/.config/systemd/user"

# Copy unit files
echo "[install-cron] Installing unit files to ${SYSTEMD_USER_DIR}/"
mkdir -p "$SYSTEMD_USER_DIR"
cp "${SCRIPT_DIR}/consign-drain.service" "${SYSTEMD_USER_DIR}/consign-drain.service"
cp "${SCRIPT_DIR}/consign-drain.timer"   "${SYSTEMD_USER_DIR}/consign-drain.timer"

# Make wrapper executable
chmod +x "${SCRIPT_DIR}/consign-cron.sh"

# Reload systemd user daemon
echo "[install-cron] Reloading systemd user daemon..."
systemctl --user daemon-reload

# Enable and start the timer
echo "[install-cron] Enabling and starting consign-drain.timer..."
systemctl --user enable --now consign-drain.timer

echo "[install-cron] Done. Timer status:"
systemctl --user status consign-drain.timer --no-pager || true

echo ""
echo "To verify: systemctl --user list-timers | grep consign"
echo "To disable: systemctl --user disable --now consign-drain.timer"
echo "To run manually: systemctl --user start consign-drain.service"
