#!/usr/bin/env python3
import os
import shutil
from pathlib import Path
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]
INSTALLER = ROOT / "install.sh"


class InstallationTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.data = Path(self.tmp.name) / "data"
        self.dest = (
            self.data / "gnome-shell" / "extensions" / "port-killer@dev"
        )

    def run_install(self, fail=False):
        env = os.environ.copy()
        env["XDG_DATA_HOME"] = str(self.data)
        env.pop("PORT_KILLER_TEST_FAIL_AFTER_BACKUP", None)
        if fail:
            env["PORT_KILLER_TEST_FAIL_AFTER_BACKUP"] = "1"

        return subprocess.run(
            ["bash", str(INSTALLER), "--extension-only"],
            env=env,
            capture_output=True,
            text=True,
            timeout=30,
        )

    def test_first_install(self):
        result = self.run_install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue((self.dest / "extension.js").is_file())
        self.assertTrue(
            (self.dest / "schemas" / "gschemas.compiled").is_file()
        )

    def test_reinstall_retains_backup(self):
        self.assertEqual(self.run_install().returncode, 0)
        (self.dest / "marker").write_text("old")

        result = self.run_install()
        self.assertEqual(result.returncode, 0, result.stderr)

        backups = list(self.dest.parent.glob(".port-killer-backup.*"))
        self.assertEqual(len(backups), 1)
        self.assertEqual((backups[0] / "marker").read_text(), "old")

    def test_helper_failure_restores_extension(self):
        self.assertEqual(self.run_install().returncode, 0)
        (self.dest / "marker").write_text("old")

        isolated = Path(self.tmp.name) / "isolated-repository"
        isolated.mkdir()
        shutil.copy2(ROOT / "install.sh", isolated / "install.sh")
        shutil.copytree(ROOT / "extension", isolated / "extension")

        scripts = isolated / "scripts"
        scripts.mkdir()

        # Exercise the real ELF validator in the isolated installer.
        shutil.copy2(
            ROOT / "scripts" / "check-helper-elf.py",
            scripts / "check-helper-elf.py",
        )

        helper_script = scripts / "install-helper.sh"
        helper_script.write_text(
            "#!/usr/bin/env bash\\n"
            "echo simulated-helper-failure >&2\\n"
            "exit 42\\n"
        )
        helper_script.chmod(0o755)

        env = os.environ.copy()
        env["XDG_DATA_HOME"] = str(self.data)
        env.pop("PORT_KILLER_TEST_FAIL_AFTER_BACKUP", None)

        result = subprocess.run(
            [
                "bash",
                str(isolated / "install.sh"),
                "--helper-binary",
                "/bin/true",
            ],
            env=env,
            capture_output=True,
            text=True,
            timeout=30,
        )

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("simulated-helper-failure", result.stderr)
        self.assertEqual(
            (self.dest / "marker").read_text(),
            "old",
        )

    def test_incompatible_helper_preserves_extension(self):
        self.assertEqual(self.run_install().returncode, 0)
        (self.dest / "marker").write_text("old")

        with tempfile.NamedTemporaryFile() as invalid:
            invalid.write(b"not-an-elf")
            invalid.flush()

            env = os.environ.copy()
            env["XDG_DATA_HOME"] = str(self.data)

            result = subprocess.run(
                [
                    "bash",
                    str(INSTALLER),
                    "--helper-binary",
                    invalid.name,
                ],
                env=env,
                capture_output=True,
                text=True,
                timeout=30,
            )

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("ELF validation failed", result.stderr)
        self.assertEqual(
            (self.dest / "marker").read_text(),
            "old",
        )

    def test_failure_restores_previous_version(self):
        self.assertEqual(self.run_install().returncode, 0)
        (self.dest / "marker").write_text("old")

        result = self.run_install(fail=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual((self.dest / "marker").read_text(), "old")

        result = self.run_install()
        self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == "__main__":
    unittest.main()
