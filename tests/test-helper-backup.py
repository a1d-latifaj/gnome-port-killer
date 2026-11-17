#!/usr/bin/env python3
"""Regression checks for privileged helper rollback backups."""

from pathlib import Path
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


if __name__ == "__main__":
    unittest.main()
