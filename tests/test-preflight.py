#!/usr/bin/env python3
"""Unit tests for installation compatibility checks."""

import importlib.util
from pathlib import Path
from unittest import mock
import unittest


ROOT = Path(__file__).resolve().parents[1]
PREFLIGHT = ROOT / "scripts" / "preflight.py"

spec = importlib.util.spec_from_file_location("preflight", PREFLIGHT)
preflight = importlib.util.module_from_spec(spec)
spec.loader.exec_module(preflight)


class PreflightTests(unittest.TestCase):
    def test_supported_ubuntu_versions(self):
        self.assertIn("24.04", preflight.SUPPORTED_UBUNTU)
        self.assertIn("26.04", preflight.SUPPORTED_UBUNTU)

    def test_supported_gnome_versions(self):
        self.assertIn(46, preflight.SUPPORTED_GNOME)
        self.assertIn(50, preflight.SUPPORTED_GNOME)
        self.assertNotIn(45, preflight.SUPPORTED_GNOME)

    def test_supported_architectures(self):
        self.assertIn("x86_64", preflight.SUPPORTED_ARCH)
        self.assertIn("aarch64", preflight.SUPPORTED_ARCH)

    def test_os_release_parsing(self):
        import tempfile

        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "os-release"
            path.write_text(
                'ID=ubuntu\n'
                'VERSION_ID="24.04"\n'
                'PRETTY_NAME="Ubuntu 24.04 LTS"\n'
            )

            result = preflight.read_os_release(path)

        self.assertEqual(result["ID"], "ubuntu")
        self.assertEqual(result["VERSION_ID"], "24.04")

    def test_missing_gnome_shell(self):
        with mock.patch.object(
            preflight.shutil,
            "which",
            return_value=None,
        ):
            self.assertIsNone(preflight.detect_gnome())


if __name__ == "__main__":
    unittest.main()
