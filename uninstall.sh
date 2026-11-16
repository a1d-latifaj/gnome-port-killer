#!/usr/bin/env bash
set -e

UUID="port-killer@dev"
DEST="$HOME/.local/share/gnome-shell/extensions/$UUID"

gnome-extensions disable "$UUID" 2>/dev/null || true
rm -rf "$DEST"
echo "U hoq $UUID."
