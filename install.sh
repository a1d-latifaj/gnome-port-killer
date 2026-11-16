#!/usr/bin/env bash
set -e

UUID="port-killer@dev"
DEST="$HOME/.local/share/gnome-shell/extensions/$UUID"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

mkdir -p "$DEST"
cp "$SCRIPT_DIR/extension.js" "$DEST/"
cp "$SCRIPT_DIR/stylesheet.css" "$DEST/"
cp "$SCRIPT_DIR/metadata.json" "$DEST/"

echo "U instalua $UUID në $DEST"
echo "Tani bëj Log Out / Log In dhe aktivizoje me:"
echo "  gnome-extensions enable $UUID"
