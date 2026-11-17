#!/usr/bin/env bash
set -euo pipefail

UUID="port-killer@dev"
DEST="${XDG_DATA_HOME:-$HOME/.local/share}/gnome-shell/extensions/$UUID"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

SOURCE_DIR="$SCRIPT_DIR/extension"

command -v glib-compile-schemas >/dev/null || { echo "Missing glib-compile-schemas" >&2; exit 1; }
glib-compile-schemas --strict --dry-run "$SOURCE_DIR/schemas"
mkdir -p "$DEST/schemas"
cp "$SOURCE_DIR/extension.js" "$DEST/"
cp "$SOURCE_DIR/sockets.js" "$SOURCE_DIR/runtime.js" "$DEST/"
cp "$SOURCE_DIR/stylesheet.css" "$DEST/"
cp "$SOURCE_DIR/metadata.json" "$DEST/"
cp "$SOURCE_DIR/schemas/"*.gschema.xml "$DEST/schemas/"
glib-compile-schemas --strict "$DEST/schemas"

echo "Installed $UUID to $DEST"
echo "Log Out / Log In and enable it with: gnome-extensions enable $UUID"

if [[ ! -x /usr/local/libexec/port-killer-events ]]; then
    echo "Live events need the Rust helper: ./scripts/install-helper.sh"
fi
