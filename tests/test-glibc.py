#!/usr/bin/env python3
"""Regression tests for GLIBC compatibility validation."""

import importlib.util
from pathlib import Path
import unittest
from unittest.mock import patch
import subprocess


ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts" / "check-glibc.py"

spec = importlib.util.spec_from_file_location("check_glibc", SCRIPT)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class GlibcTests(unittest.TestCase):
    def test_extracts_versions(self):
        output = """
        Name: GLIBC_2.34
        Name: GLIBC_2.2.5
        Name: GLIBC_2.39
        """
        self.assertEqual(
            module.required_versions(output),
            {(2, 34), (2, 2), (2, 39)},
        )

    def test_rejects_missing_versions(self):
        with patch.object(
            module.subprocess,
            "run",
            return_value=subprocess.CompletedProcess(
                args=[], returncode=0, stdout="No version information"
            ),
        ):
            with self.assertRaises(ValueError):
                module.validate("fake-binary", (2, 39))

    def test_rejects_newer_glibc(self):
        with patch.object(
            module.subprocess,
            "run",
            return_value=subprocess.CompletedProcess(
                args=[], returncode=0, stdout="Name: GLIBC_2.40"
            ),
        ):
            with self.assertRaises(ValueError):
                module.validate("fake-binary", (2, 39))

    def test_accepts_supported_glibc(self):
        with patch.object(
            module.subprocess,
            "run",
            return_value=subprocess.CompletedProcess(
                args=[], returncode=0, stdout="Name: GLIBC_2.34"
            ),
        ):
            module.validate("fake-binary", (2, 39))


if __name__ == "__main__":
    unittest.main()
