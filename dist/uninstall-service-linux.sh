#!/bin/bash
# Script para desinstalar ludusavi-daemon

set -e

SERVICE_NAME="ludusavi-daemon"
SYSTEMD_USER_DIR="$HOME/.config/systemd/user"

echo "Stopping service..."
systemctl --user stop $SERVICE_NAME 2>/dev/null || true

echo "Disabling service..."
systemctl --user disable $SERVICE_NAME 2>/dev/null || true

echo "Removing service file..."
rm -f "$SYSTEMD_USER_DIR/$SERVICE_NAME.service"

echo "Removing binary..."
rm -f "$HOME/.local/bin/$SERVICE_NAME"

echo "Reloading systemd..."
systemctl --user daemon-reload

echo "Done. Ludusavi daemon has been uninstalled."
