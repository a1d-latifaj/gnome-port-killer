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

# Keep privileged rollback material outside user-writable directories.
BACKUP_ROOT="/var/lib/gnome-port-killer/backups"

sudo install -d -o root -g root -m 0700 \
    /var/lib/gnome-port-killer "$BACKUP_ROOT"

BACKUP="$(sudo mktemp -d "$BACKUP_ROOT/helper.XXXXXXXX")"
sudo chmod 0700 "$BACKUP"

WAS_ACTIVE=0
WAS_ENABLED=0
HAD_BINARY=0
HAD_UNIT=0
APPLY_STARTED=0
COMMITTED=0

systemctl is-active --quiet "$SERVICE" && WAS_ACTIVE=1 || true
systemctl is-enabled --quiet "$SERVICE" && WAS_ENABLED=1 || true

if sudo test -f "$DEST"; then
    sudo install -o root -g root -m 0700 "$DEST" "$BACKUP/previous-binary"
    HAD_BINARY=1
fi

if sudo test -f "$UNIT"; then
    sudo install -o root -g root -m 0600 "$UNIT" "$BACKUP/previous-unit"
    HAD_UNIT=1
fi

rollback() {
    local code=$?
    trap - EXIT INT TERM

    if [[ "$COMMITTED" -eq 1 || "$APPLY_STARTED" -eq 0 ]]; then
        exit "$code"
    fi

    echo "Helper installation failed. Rolling back..." >&2

    local rollback_failed=0

    rollback_step() {
        if ! "$@"; then
            echo "ROLLBACK ERROR: $*" >&2
            rollback_failed=1
        fi
    }

    rollback_step sudo systemctl stop "$SERVICE"

    if [[ "$HAD_BINARY" -eq 1 ]]; then
        rollback_step sudo install -o root -g root -m 0755 \
            "$BACKUP/previous-binary" "$DEST"
    else
        rollback_step sudo rm -f "$DEST"
    fi

    if [[ "$HAD_UNIT" -eq 1 ]]; then
        rollback_step sudo install -o root -g root -m 0644 \
            "$BACKUP/previous-unit" "$UNIT"
    else
        rollback_step sudo rm -f "$UNIT"
    fi

    rollback_step sudo systemctl daemon-reload

    if [[ "$WAS_ENABLED" -eq 1 ]]; then
        rollback_step sudo systemctl enable "$SERVICE"
    else
        rollback_step sudo systemctl disable "$SERVICE"
    fi

    if [[ "$WAS_ACTIVE" -eq 1 ]]; then
        rollback_step sudo systemctl start "$SERVICE"
    fi

    echo "Rollback backup: $BACKUP" >&2

    if [[ "$rollback_failed" -ne 0 ]]; then
        echo "CRITICAL: Rollback incomplete. Manual recovery may be required." >&2
        exit 1
    fi

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
