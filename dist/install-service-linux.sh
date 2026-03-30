#!/bin/bash
# Script para instalar ludusavi-daemon como servicio de systemd
# Equivalente a UseSystemd() de EmuSync
# Funciona en Linux normal y Steam Deck

set -e

BINARY_PATH="${1:-./ludusavi-daemon}"
SERVICE_FILE="./ludusavi-daemon.service"
SYSTEMD_USER_DIR="$HOME/.config/systemd/user"

# Comprueba que el binario existe
if [ ! -f "$BINARY_PATH" ]; then
    echo "Error: binary not found at $BINARY_PATH"
    echo "Usage: $0 /path/to/ludusavi-daemon"
    exit 1
fi

# Instala el binario
echo "Installing binary..."
mkdir -p "$HOME/.local/bin"
cp "$BINARY_PATH" "$HOME/.local/bin/ludusavi-daemon"
chmod +x "$HOME/.local/bin/ludusavi-daemon"

# Crea el directorio de servicios de usuario si no existe
mkdir -p "$SYSTEMD_USER_DIR"

# Copia el fichero de servicio con la ruta correcta
echo "Installing service file..."
sed "s|/usr/local/bin/ludusavi-daemon|$HOME/.local/bin/ludusavi-daemon|g" \
    "$SERVICE_FILE" > "$SYSTEMD_USER_DIR/ludusavi-daemon.service"

# Recarga systemd y activa el servicio
echo "Enabling service..."
systemctl --user daemon-reload
systemctl --user enable ludusavi-daemon
systemctl --user start ludusavi-daemon

echo ""
echo "Done. Service status:"
systemctl --user status ludusavi-daemon --no-pager

echo ""
echo "Useful commands:"
echo "  systemctl --user status ludusavi-daemon   # ver estado"
echo "  systemctl --user stop ludusavi-daemon     # parar"
echo "  systemctl --user start ludusavi-daemon    # arrancar"
echo "  journalctl --user -u ludusavi-daemon -f   # ver logs"
