#!/usr/bin/env bash
set -euo pipefail

UUID="port-killer@dev"
DEST="${XDG_DATA_HOME:-$HOME/.local/share}/gnome-shell/extensions/$UUID"

gnome-extensions disable "$UUID" 2>/dev/null || true
rm -rf "$DEST"
echo "Removed $UUID."

echo "To remove the system event monitor too: ./scripts/uninstall-helper.sh"
