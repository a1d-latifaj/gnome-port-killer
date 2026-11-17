#!/usr/bin/env bash

cd /home/aidlatifaj/projects/gnome-port-killer || exit 1

echo "=== VERIFY ACTIVE SERVICE ==="
systemctl is-active --quiet port-killer-events.service || exit 1

echo "=== VERIFY LOCAL BINARY ==="
test -x helper/target/debug/port-killer-events || exit 1

echo "=== VERIFY DEBUG MODE ==="
grep -q -- '"--debug-events"' helper/src/main.rs || exit 1

echo "=== RUN CONTROLLED UDP TEST ==="
sudo -v || exit 1
python3 experiments/udp-filter-test/controlled_test.py
