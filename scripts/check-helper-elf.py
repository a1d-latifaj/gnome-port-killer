#!/usr/bin/env python3
"""Validate helper ELF format and host architecture without binutils."""

import argparse
import platform
import struct
import sys
from pathlib import Path


MACHINES = {"x86_64": 62, "aarch64": 183}
ALIASES = {"amd64": "x86_64", "arm64": "aarch64"}


def validate(path, architecture=None):
    host = (architecture or platform.machine()).lower()
    host = ALIASES.get(host, host)

    if host not in MACHINES:
        raise ValueError(f"Unsupported architecture: {host}")

    with Path(path).open("rb") as stream:
        header = stream.read(64)

    if len(header) < 64 or header[:4] != b"\x7fELF":
        raise ValueError("Not a complete ELF64 header")

    if header[4] != 2:
        raise ValueError("Expected 64-bit ELF")

    if header[5] != 1:
        raise ValueError("Expected little-endian ELF")

    if header[6] != 1:
        raise ValueError("Unsupported ELF identification version")

    elf_type, machine, version = struct.unpack_from("<HHI", header, 16)

    if elf_type not in (2, 3):
        raise ValueError("ELF is not executable")

    if version != 1:
        raise ValueError("Unsupported ELF version")

    if machine != MACHINES[host]:
        raise ValueError(
            f"ELF machine {machine} does not match host {host}"
        )

    print(f"ELF validation: PASS ({host})")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("binary")
    args = parser.parse_args()

    try:
        validate(args.binary)
    except (OSError, ValueError) as error:
        print(f"ELF validation failed: {error}", file=sys.stderr)
        return 1

    return 0


if __name__ == "__main__":
    sys.exit(main())
