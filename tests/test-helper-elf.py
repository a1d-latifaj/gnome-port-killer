#!/usr/bin/env python3
"""Tests for helper ELF architecture validation."""

import importlib.util
import struct
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts" / "check-helper-elf.py"

spec = importlib.util.spec_from_file_location("helper_elf", SCRIPT)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class HelperElfTests(unittest.TestCase):
    def make_elf(self, machine=62, elf_type=3, bits=2):
        header = bytearray(64)
        header[:4] = b"\x7fELF"
        header[4] = bits
        header[5] = 1
        header[6] = 1
        struct.pack_into("<HHI", header, 16, elf_type, machine, 1)

        temporary = tempfile.NamedTemporaryFile(delete=False)
        temporary.write(header)
        temporary.close()
        self.addCleanup(Path(temporary.name).unlink)
        return temporary.name

    def test_accepts_x86_64(self):
        module.validate(self.make_elf(), "x86_64")

    def test_accepts_aarch64(self):
        module.validate(self.make_elf(machine=183), "aarch64")

    def test_rejects_wrong_architecture(self):
        with self.assertRaises(ValueError):
            module.validate(self.make_elf(machine=183), "x86_64")

    def test_rejects_non_executable_elf(self):
        with self.assertRaises(ValueError):
            module.validate(self.make_elf(elf_type=1), "x86_64")

    def test_rejects_32_bit_elf(self):
        with self.assertRaises(ValueError):
            module.validate(self.make_elf(bits=1), "x86_64")

    def test_rejects_invalid_file(self):
        with tempfile.NamedTemporaryFile() as temporary:
            temporary.write(b"not an ELF")
            temporary.flush()
            with self.assertRaises(ValueError):
                module.validate(temporary.name, "x86_64")


if __name__ == "__main__":
    unittest.main()
