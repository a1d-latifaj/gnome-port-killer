#!/usr/bin/env python3
"""Check whether the host supports GNOME Port Killer installation."""

import argparse
from pathlib import Path
import platform
import re
import shutil
import subprocess
import sys


SUPPORTED_UBUNTU = {"24.04", "26.04"}
SUPPORTED_GNOME = {46, 47, 48, 49, 50}
SUPPORTED_ARCH = {"x86_64", "aarch64"}


def read_os_release(path=Path("/etc/os-release")):
    values = {}

    for line in path.read_text().splitlines():
        if not line or line.startswith("#") or "=" not in line:
            continue

        key, value = line.split("=", 1)
        values[key] = value.strip().strip('"')

    return values


def detect_gnome():
    executable = shutil.which("gnome-shell")

    if not executable:
        return None

    try:
        result = subprocess.run(
            [executable, "--version"],
            capture_output=True,
            text=True,
            timeout=5,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired):
        return None

    match = re.search(
        r"GNOME Shell\s+(\d+)",
        result.stdout + result.stderr,
    )

    return int(match.group(1)) if match else None


def check(name, condition, detail):
    status = "PASS" if condition else "FAIL"
    print(f"[{status}] {name}: {detail}")
    return bool(condition)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--mode",
        choices=("build", "prebuilt", "extension-only"),
        default="build",
    )
    args = parser.parse_args()

    try:
        os_info = read_os_release()
    except OSError as error:
        print(f"Cannot read OS information: {error}", file=sys.stderr)
        return 1

    ubuntu_version = os_info.get("VERSION_ID", "unknown")
    ubuntu = os_info.get("ID") == "ubuntu"
    arch = platform.machine()
    gnome = detect_gnome()

    results = []

    print("GNOME Port Killer — Installation Preflight")
    print(f"Mode: {args.mode}")
    print()

    results.append(check(
        "Operating system",
        ubuntu and ubuntu_version in SUPPORTED_UBUNTU,
        f"{os_info.get('PRETTY_NAME', 'unknown')}",
    ))

    results.append(check(
        "Architecture",
        arch in SUPPORTED_ARCH,
        arch,
    ))

    results.append(check(
        "GNOME Shell",
        gnome in SUPPORTED_GNOME,
        str(gnome) if gnome is not None else "not detected",
    ))

    required = ["glib-compile-schemas"]

    if args.mode != "extension-only":
        results.append(check(
            "Kernel BTF",
            Path("/sys/kernel/btf/vmlinux").is_file(),
            "/sys/kernel/btf/vmlinux",
        ))

        required.extend(["systemctl", "sudo", "python3", "sha256sum"])

    if args.mode == "build":
        required.extend(["cargo", "rustc", "clang", "bpftool"])

    for command in required:
        results.append(check(
            f"Dependency: {command}",
            shutil.which(command) is not None,
            command,
        ))

    print()

    if all(results):
        print("RESULT: PASS — Host satisfies preliminary requirements.")
        return 0

    print("RESULT: FAIL — Resolve the failed checks before installation.")
    return 1


if __name__ == "__main__":
    sys.exit(main())
