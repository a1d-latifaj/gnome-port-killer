#!/usr/bin/env python3
import os
import socket
import subprocess
import threading
import time

ROOT = "/home/aidlatifaj/projects/gnome-port-killer"
BINARY = f"{ROOT}/helper/target/debug/port-killer-events"

events = []
lock = threading.Lock()
ready = threading.Event()

def read_events(process):
    for line in process.stdout:
        line = line.strip()
        with lock:
            events.append((time.monotonic(), line))
        if line == "ready":
            ready.set()

def snapshot():
    with lock:
        return len(events)

def report(label, start, end):
    with lock:
        selected = [event for _, event in events[start:end]]
    print(f"{label}: UDP={selected.count('change UDP')}, "
          f"TCP={selected.count('change TCP')}, "
          f"UNIX={selected.count('change UNIX')}, "
          f"RESYNC={selected.count('resync')}", flush=True)

process = subprocess.Popen(
    ["sudo", BINARY, "--debug-events"],
    cwd=ROOT,
    stdout=subprocess.PIPE,
    stderr=subprocess.STDOUT,
    text=True,
    bufsize=1,
)

thread = threading.Thread(
    target=read_events,
    args=(process,),
    daemon=True,
)
thread.start()

try:
    if not ready.wait(timeout=10):
        raise RuntimeError("Monitor did not become ready")

    print("Monitor ready.", flush=True)

    start = snapshot()
    time.sleep(3)
    end = snapshot()
    report("BASELINE", start, end)

    start = snapshot()

    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as sock:
        sock.bind(("127.0.0.1", 0))
        port = sock.getsockname()[1]
        print(f"UDP_BIND port={port}", flush=True)
        time.sleep(3)

    print(f"UDP_CLOSE port={port}", flush=True)
    time.sleep(2)

    end = snapshot()
    report("TRACKED UDP", start, end)

    start = snapshot()

    for _ in range(100):
        with socket.socket(socket.AF_INET, socket.SOCK_DGRAM):
            pass

    time.sleep(2)

    end = snapshot()
    report("100 UNBOUND UDP CLOSES", start, end)

finally:
    process.terminate()
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait()
