#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
UUID="port-killer@dev"
DEST="${XDG_DATA_HOME:-$HOME/.local/share}/gnome-shell/extensions/$UUID"

if [[ $# -gt 1 || ( $# -eq 1 && "$1" != "--with-helper" ) ]]; then
    echo "Usage: $0 [--with-helper]" >&2
    exit 2
fi

if [[ -L "$DEST" ]]; then
    echo "Refusing to remove symlink destination: $DEST" >&2
    exit 1
fi

gnome-extensions disable "$UUID" 2>/dev/null || true

if [[ -d "$DEST" ]]; then
    rm -rf -- "$DEST"
fi

echo "GNOME extension removed: $UUID"

if [[ "${1:-}" == "--with-helper" ]]; then
    "$ROOT/scripts/uninstall-helper.sh"
else
    echo "System helper retained."
    echo "To remove it: ./uninstall.sh --with-helper"
fi
