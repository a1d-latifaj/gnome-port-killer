#!/usr/bin/env bash
set -Eeuo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
UUID="port-killer@dev"
EXTENSIONS="${XDG_DATA_HOME:-$HOME/.local/share}/gnome-shell/extensions"
DEST="$EXTENSIONS/$UUID"
SOURCE="$ROOT/extension"

MODE="build"
HELPER_BINARY=""

usage() {
    echo "Usage: $0 [--extension-only | --helper-binary PATH]"
}

case "${1:-}" in
    "")
        [[ $# -eq 0 ]] || { usage >&2; exit 2; }
        ;;
    --extension-only)
        [[ $# -eq 1 ]] || { usage >&2; exit 2; }
        MODE="extension-only"
        ;;
    --helper-binary)
        [[ $# -eq 2 ]] || { usage >&2; exit 2; }
        MODE="prebuilt"
        HELPER_BINARY="$2"
        ;;
    *)
        usage >&2
        exit 2
        ;;
esac

# Validate compatibility before creating installation directories.
if [[ -f "$ROOT/scripts/preflight.py" ]]; then
    command -v python3 >/dev/null || {
        echo "Missing python3 required for compatibility checks" >&2
        exit 1
    }

    python3 "$ROOT/scripts/preflight.py" --mode "$MODE"
fi

command -v glib-compile-schemas >/dev/null || {
    echo "Missing glib-compile-schemas" >&2
    exit 1
}

[[ -d "$SOURCE/schemas" ]] || {
    echo "Missing extension schemas" >&2
    exit 1
}

mkdir -p "$EXTENSIONS"

STAGING="$(mktemp -d "$EXTENSIONS/.port-killer-stage.XXXXXXXX")"
BACKUP=""
OLD_MOVED=0
NEW_INSTALLED=0
COMMITTED=0

cleanup() {
    local code=$?
    trap - EXIT ERR INT TERM

    if [[ "$COMMITTED" -eq 0 ]]; then
        if [[ "$NEW_INSTALLED" -eq 1 ]]; then
            echo "Removing incomplete GNOME extension..." >&2
            rm -rf -- "$DEST"
        fi

        if [[ "$OLD_MOVED" -eq 1 ]]; then
            echo "Restoring previous GNOME extension..." >&2
            if [[ -e "$DEST" || -L "$DEST" ]]; then
                echo "Rollback blocked: destination already exists: $DEST" >&2
            else
                mv -- "$BACKUP" "$DEST" || {
                    echo "Rollback failed; previous version remains at: $BACKUP" >&2
                }
            fi
        fi
    fi

    if [[ -d "$STAGING" ]]; then
        rm -rf -- "$STAGING"
    fi

    exit "$code"
}

trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

install -m 0644 \
    "$SOURCE/extension.js" \
    "$SOURCE/sockets.js" \
    "$SOURCE/runtime.js" \
    "$SOURCE/stylesheet.css" \
    "$SOURCE/metadata.json" \
    "$STAGING/"

mkdir -p "$STAGING/schemas"
install -m 0644 "$SOURCE/schemas/"*.gschema.xml "$STAGING/schemas/"

glib-compile-schemas --strict --dry-run "$STAGING/schemas"
glib-compile-schemas --strict "$STAGING/schemas"

if [[ "$MODE" == "build" ]]; then
    "$ROOT/scripts/build-helper.sh"
    HELPER_BINARY="$ROOT/helper/target/release/port-killer-events"
fi


if [[ -L "$DEST" || ( -e "$DEST" && ! -d "$DEST" ) ]]; then
    echo "Extension destination is not a regular directory" >&2
    exit 1
fi

if [[ -d "$DEST" ]]; then
    BACKUP="$(mktemp -d "$EXTENSIONS/.port-killer-backup.XXXXXXXX")"
    rmdir "$BACKUP"
    mv -- "$DEST" "$BACKUP"
    OLD_MOVED=1
fi

# Test-only fault injection. No effect unless explicitly enabled.
if [[ "${PORT_KILLER_TEST_FAIL_AFTER_BACKUP:-0}" == "1" ]]; then
    echo "Simulated failure after extension backup" >&2
    exit 97
fi

mv -- "$STAGING" "$DEST"
NEW_INSTALLED=1

if [[ "$MODE" != "extension-only" ]]; then
    "$ROOT/scripts/install-helper.sh" "$HELPER_BINARY"
fi

COMMITTED=1
trap - EXIT INT TERM

echo "GNOME extension installed: $UUID"
echo "Location: $DEST"

if [[ -n "$BACKUP" ]]; then
    echo "Previous extension retained at: $BACKUP"
fi

echo "Enable with: gnome-extensions enable $UUID"
echo "A fresh GNOME Shell session may be required."
