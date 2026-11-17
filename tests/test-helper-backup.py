#!/usr/bin/env python3
"""Regression checks for privileged helper rollback backups."""

from pathlib import Path
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]
INSTALLER = ROOT / "scripts" / "install-helper.sh"


class HelperBackupTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.script = INSTALLER.read_text()

    def test_backup_location_is_root_owned(self):
        self.assertIn(
            'BACKUP_ROOT="/var/lib/gnome-port-killer/backups"',
            self.script,
        )
        self.assertIn(
            "sudo install -d -o root -g root -m 0700",
            self.script,
        )
        self.assertIn(
            'BACKUP="$(sudo mktemp -d "$BACKUP_ROOT/helper.XXXXXXXX")"',
            self.script,
        )

    def test_backup_files_are_root_owned(self):
        self.assertIn(
            'sudo install -o root -g root -m 0700 '
            '"$DEST" "$BACKUP/previous-binary"',
            self.script,
        )
        self.assertIn(
            'sudo install -o root -g root -m 0600 '
            '"$UNIT" "$BACKUP/previous-unit"',
            self.script,
        )

    def test_rollback_uses_privileged_backup(self):
        self.assertIn(
            '"$BACKUP/previous-binary" "$DEST"',
            self.script,
        )
        self.assertIn(
            '"$BACKUP/previous-unit" "$UNIT"',
            self.script,
        )
        self.assertNotIn(
            '${XDG_STATE_HOME:-$HOME/.local/state}',
            self.script,
        )


class HelperRollbackBehaviorTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        script = INSTALLER.read_text()
        start = script.index("rollback() {")
        end = script.index("\ntrap rollback EXIT", start)
        cls.rollback_function = script[start:end]

    def run_rollback(self, failing_operation):
        with tempfile.TemporaryDirectory() as directory:
            script = f"""
set -Eeuo pipefail

sudo() {{
    printf '%s\\n' "$*" >> "{directory}/operations"
    case "$*" in
        *"{failing_operation}"*) return 1 ;;
    esac
    return 0
}}

SERVICE="port-killer-events.service"
DEST="/tmp/mock-port-killer-events"
UNIT="/tmp/mock-port-killer-events.service"
BACKUP="/tmp/mock-backup"

WAS_ACTIVE=1
WAS_ENABLED=1
HAD_BINARY=1
HAD_UNIT=1
APPLY_STARTED=1
COMMITTED=0

{self.rollback_function}

trap rollback EXIT
false
"""
            result = subprocess.run(
                ["bash", "-c", script],
                capture_output=True,
                text=True,
                timeout=10,
            )

            operations = Path(directory, "operations").read_text()
            return result, operations

    def test_failed_restore_reports_incomplete_rollback(self):
        result, operations = self.run_rollback("previous-binary")

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("ROLLBACK ERROR:", result.stderr)
        self.assertIn("CRITICAL: Rollback incomplete", result.stderr)
        self.assertIn("previous-binary", operations)

    def test_rollback_continues_after_failed_stop(self):
        result, operations = self.run_rollback("systemctl stop")

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("ROLLBACK ERROR:", result.stderr)
        self.assertIn("CRITICAL: Rollback incomplete", result.stderr)
        self.assertIn("systemctl start", operations)


if __name__ == "__main__":
    unittest.main()
