#!/usr/bin/env bash
set -euo pipefail
PROJECT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
if [[ $# -gt 1 ]]; then echo "Usage: $0 [prebuilt-binary]" >&2; exit 2; fi
if [[ ! -r /sys/kernel/btf/vmlinux ]]; then
    echo "Kernel BTF is required (/sys/kernel/btf/vmlinux)." >&2
    exit 1
fi
if [[ $# -eq 1 ]]; then
    HELPER_BINARY="$(realpath "$1")"
else
    "$PROJECT_DIR/scripts/build-helper.sh"
    HELPER_BINARY="$PROJECT_DIR/helper/target/release/port-killer-events"
fi
[[ -x "$HELPER_BINARY" ]] || { echo "Helper binary is not executable" >&2; exit 1; }
# Privileged installation copies only a binary and a fixed service definition.
# The service never reads BPF programs or commands from this writable checkout.
sudo install -d -m 0755 /usr/local/libexec
sudo install -o root -g root -m 0755 "$HELPER_BINARY" /usr/local/libexec/port-killer-events
sudo install -o root -g root -m 0644 "$PROJECT_DIR/packaging/port-killer-events.service" /etc/systemd/system/port-killer-events.service
sudo systemctl daemon-reload
sudo systemctl enable port-killer-events.service
sudo systemctl restart port-killer-events.service
sudo systemctl --no-pager --full status port-killer-events.service
