#!/usr/bin/env python3
import argparse
import os
import selectors
import subprocess
import sys
import time


def check_ready(binary, timeout):
    process = subprocess.Popen(
        [binary, "--watch"],
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        bufsize=0,
    )

    selector = selectors.DefaultSelector()
    selector.register(process.stdout, selectors.EVENT_READ)
    deadline = time.monotonic() + timeout
    response = bytearray()

    try:
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError("Timed out waiting for helper readiness")

            events = selector.select(remaining)
            if not events:
                raise TimeoutError("Timed out waiting for helper readiness")

            chunk = os.read(process.stdout.fileno(), 4096)
            if not chunk:
                raise RuntimeError("Helper closed stream before readiness")

            response.extend(chunk)

            if len(response) > 4096:
                raise RuntimeError("Helper readiness response too large")

            if b"\n" in response:
                first_line = response.split(b"\n", 1)[0]
                if first_line != b"ready":
                    raise RuntimeError(
                        "Unexpected readiness response: %r" % first_line
                    )
                return
    finally:
        selector.close()
        process.stdout.close()

        if process.poll() is None:
            process.terminate()

        try:
            process.wait(timeout=2)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("binary")
    parser.add_argument("--timeout", type=float, default=10)
    args = parser.parse_args()

    if args.timeout <= 0:
        parser.error("--timeout must be positive")

    try:
        check_ready(args.binary, args.timeout)
    except (OSError, RuntimeError, TimeoutError) as error:
        print("Helper readiness failed:", error, file=sys.stderr)
        return 1

    print("Helper readiness: PASS")
    return 0


if __name__ == "__main__":
    sys.exit(main())
