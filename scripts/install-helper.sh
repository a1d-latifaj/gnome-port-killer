#!/usr/bin/env bash
set -Eeuo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SERVICE="port-killer-events.service"
DEST="/usr/local/libexec/port-killer-events"
UNIT="/etc/systemd/system/$SERVICE"
SOURCE_UNIT="$ROOT/packaging/$SERVICE"

if [[ $# -gt 1 ]]; then
    echo "Usage: $0 [prebuilt-binary]" >&2
    exit 2
fi

[[ -r /sys/kernel/btf/vmlinux ]] || {
    echo "Kernel BTF is required." >&2
    exit 1
}

if [[ $# -eq 1 ]]; then
    BINARY="$(realpath "$1")"
else
    "$ROOT/scripts/build-helper.sh"
    BINARY="$ROOT/helper/target/release/port-killer-events"
fi

[[ -x "$BINARY" ]] || {
    echo "Missing executable helper: $BINARY" >&2
    exit 1
}

[[ -f "$SOURCE_UNIT" ]] || {
    echo "Missing systemd unit" >&2
    exit 1
}

BACKUP_ROOT="${XDG_STATE_HOME:-$HOME/.local/state}/gnome-port-killer/backups"
mkdir -p "$BACKUP_ROOT"

BACKUP="$(mktemp -d "$BACKUP_ROOT/helper.XXXXXXXX")"
chmod 700 "$BACKUP"

WAS_ACTIVE=0
WAS_ENABLED=0
HAD_BINARY=0
HAD_UNIT=0
APPLY_STARTED=0
COMMITTED=0

systemctl is-active --quiet "$SERVICE" && WAS_ACTIVE=1 || true
systemctl is-enabled --quiet "$SERVICE" && WAS_ENABLED=1 || true

if sudo test -f "$DEST"; then
    sudo cp -p "$DEST" "$BACKUP/previous-binary"
    HAD_BINARY=1
fi

if sudo test -f "$UNIT"; then
    sudo cp -p "$UNIT" "$BACKUP/previous-unit"
    HAD_UNIT=1
fi

rollback() {
    local code=$?
    trap - EXIT INT TERM

    if [[ "$COMMITTED" -eq 1 || "$APPLY_STARTED" -eq 0 ]]; then
        exit "$code"
    fi

    echo "Helper installation failed. Rolling back..." >&2

    sudo systemctl stop "$SERVICE" || true

    if [[ "$HAD_BINARY" -eq 1 ]]; then
        sudo install -o root -g root -m 0755 \
            "$BACKUP/previous-binary" "$DEST" || true
    else
        sudo rm -f "$DEST" || true
    fi

    if [[ "$HAD_UNIT" -eq 1 ]]; then
        sudo install -o root -g root -m 0644 \
            "$BACKUP/previous-unit" "$UNIT" || true
    else
        sudo rm -f "$UNIT" || true
    fi

    sudo systemctl daemon-reload || true

    if [[ "$WAS_ENABLED" -eq 1 ]]; then
        sudo systemctl enable "$SERVICE" || true
    else
        sudo systemctl disable "$SERVICE" || true
    fi

    if [[ "$WAS_ACTIVE" -eq 1 ]]; then
        sudo systemctl start "$SERVICE" || true
    fi

    echo "Rollback backup: $BACKUP" >&2
    exit "$code"
}

trap rollback EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

APPLY_STARTED=1

sudo install -d -m 0755 /usr/local/libexec
sudo install -o root -g root -m 0755 "$BINARY" "$DEST"
sudo install -o root -g root -m 0644 "$SOURCE_UNIT" "$UNIT"

sudo systemctl daemon-reload
sudo systemctl enable "$SERVICE"
sudo systemctl restart "$SERVICE"

sleep 2

sudo systemctl is-active --quiet "$SERVICE"

EXPECTED="$(sha256sum "$BINARY" | awk '{print $1}')"
ACTUAL="$(sha256sum "$DEST" | awk '{print $1}')"

[[ "$EXPECTED" == "$ACTUAL" ]] || {
    echo "Installed binary checksum mismatch" >&2
    exit 1
}

# Verify the installed helper through its watch protocol.
python3 "$ROOT/scripts/check-helper-ready.py" "$DEST"

COMMITTED=1
trap - EXIT INT TERM

echo "Helper installed and verified."
echo "Rollback backup retained at: $BACKUP"
