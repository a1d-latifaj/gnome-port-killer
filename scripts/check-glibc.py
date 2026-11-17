#!/usr/bin/env python3
"""Validate a Linux ELF binary's required GLIBC symbol versions."""

import argparse
import re
import subprocess
import sys


def required_versions(readelf_output):
    return {
        tuple(map(int, match))
        for match in re.findall(
            r"GLIBC_(\d+)\.(\d+)(?:\.\d+)?",
            readelf_output,
        )
    }


def validate(binary, maximum):
    result = subprocess.run(
        ["readelf", "--version-info", binary],
        capture_output=True,
        text=True,
        check=True,
    )

    versions = required_versions(result.stdout)

    if not versions:
        raise ValueError(
            "No GLIBC symbol versions found; refusing to pass validation"
        )

    highest = max(versions)

    print(
        "Highest required GLIBC:",
        ".".join(map(str, highest)),
    )

    if highest > maximum:
        raise ValueError(
            "Binary requires GLIBC newer than "
            + ".".join(map(str, maximum))
        )


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("binary")
    parser.add_argument("--max-version", default="2.39")
    args = parser.parse_args()

    if not re.fullmatch(r"\d+\.\d+", args.max_version):
        parser.error("--max-version must be MAJOR.MINOR")

    maximum = tuple(map(int, args.max_version.split(".")))

    try:
        validate(args.binary, maximum)
    except (OSError, subprocess.CalledProcessError, ValueError) as error:
        print("GLIBC validation failed:", error, file=sys.stderr)
        return 1

    print("GLIBC compatibility: PASS")
    return 0


if __name__ == "__main__":
    sys.exit(main())
