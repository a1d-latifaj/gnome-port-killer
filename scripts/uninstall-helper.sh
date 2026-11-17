#!/usr/bin/env bash
set -euo pipefail
sudo systemctl disable --now port-killer-events.service
sudo rm -f /etc/systemd/system/port-killer-events.service /usr/local/libexec/port-killer-events
sudo systemctl daemon-reload
