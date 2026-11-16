# Event monitor

## Components

`extension/` remains a GNOME Shell JavaScript extension. The Rust executable has a system daemon mode (`--serve`), an unprivileged stream client (`--watch`) and a short probe-attachment check (`--check`). The daemon uses Aya to load an embedded CO-RE object directly. The kernel program is C compiled by Clang at build time; userspace is Rust. It neither runs bpftrace nor loads programs from user-writable files. Clang, bpftool, Rust and Cargo are build tools only.

```mermaid
flowchart LR
    Kernel[Kernel bind/listen/close events] --> BPF[Embedded eBPF ring buffer]
    BPF --> Rust[Native Rust Aya daemon]
    Rust --> Socket[/run/port-killer/events.sock]
    Socket --> Watch[Unprivileged Rust client]
    Watch --> GNOME[GNOME extension]
    Docker[docker events] --> GNOME
    GNOME --> Snapshot[Async ss and proc snapshot]
    Snapshot --> UI[Socket cards and badge]
```

The extension connects the monitors before its first scan. A `ready` notification follows successful attachment of all probes and requests a snapshot to cover startup races. Socket event bursts share one 180 ms debounce. Native changes carry a protocol hint, so a UDP event scans only UDP and preserves cached TCP/UNIX entries. DEV scans only TCP and ignores background UDP/UNIX events; switching to ALL requests a fresh complete view. Process metadata is loaded only for the selected view while the menu is open. Events received while a scan is running cause one follow-up scan; they are not discarded. There is no recurring scan timer. Command deadlines and event debouncing use one-shot timers.

Successful bind events publish UDP and UNIX datagram changes. A BTF/CO-RE read of the successful syscall file descriptor identifies socket family and type. UNIX listen and TCP listener state events publish stream listener changes. TCP listener transitions detect closes even outside the originating process. `__sock_release` detects UDP and bound UNIX datagram/listener closure, including process exit and `close_range`. Netlink binds and connected UNIX stream closes are ignored to avoid feedback from `ss`, Docker client requests and event clients themselves.

The helper sends `waiting`, `ready`, protocol hints (`change TCP`, `change UDP`, `change UNIX`) and `resync` lines. A global 256 KiB BPF ring buffer replaces per-CPU bpftrace buffers. The Rust loop drains at most 4096 records per batch, merges protocol hints and blocks indefinitely on ring readiness. A kernel counter records dropped events; changes trigger `resync`. No PIDs, ports, addresses, commands or packet contents travel through its world-readable local socket. Local users can observe generic system socket activity, but cannot send the daemon commands. The client cap is 32; slow/disconnected clients are dropped. Socket writes have deadlines. Service restarts disconnect subscribers; the UI reports it and ↻ reconnects and resynchronizes.

Docker events invalidate the cached published-port mapping and request a snapshot. Events may arrive before all listener changes are visible; subsequent kernel events request another snapshot. An unavailable Docker CLI does not prevent socket monitoring.

## Privileges

The fixed system service runs as root to attach BPF probes. It never runs process kills, Docker actions, user-supplied probes, or commands requested through its socket. `Stop Container` runs in the unprivileged extension context using the user's Docker permissions.

The installer writes a root-owned executable to `/usr/local/libexec/port-killer-events` and a fixed unit to `/etc/systemd/system/port-killer-events.service`. systemd creates the root-owned runtime directory. The service uses a read-only filesystem, a private temporary directory, a bounded restart policy and a control group so it is stopped as a unit. The Rust binary has no setuid bit or file capabilities. Running the installer requires sudo authentication.

## Build and probe checks

```bash
cargo fetch --manifest-path helper/Cargo.toml --locked
./scripts/build-helper.sh
sudo helper/target/release/port-killer-events --check
```

The build script generates type definitions from BTF with bpftool and compiles `helper/bpf/monitor.bpf.c` using Clang. The resulting ELF object is embedded in the Rust executable. Aya applies CO-RE relocations using the running kernel BTF. Rebuild and reinstall the helper after changing the BPF source. Kernel BTF and the probe names/fields used by `monitor.bpf.c` are required. A kernel change may require updating those probes; the service does not pretend events are live when attachment fails.

## Live test

Install the helper and extension, then open a fresh development GNOME session. Verify service health and subscribe from a normal terminal:

```bash
systemctl status port-killer-events.service
journalctl -u port-killer-events.service -b --no-pager
/usr/local/libexec/port-killer-events --watch
```

Run the optional live event check from another terminal:

```bash
python3 tests/live-events.py
```

This check opens its own temporary TCP, UDP and UNIX listeners and checks that each open/close produces an invalidation. It also checks that no invalidations are triggered by `ss` alone. Because the event stream is system-wide, unrelated applications can produce events during the test; treat a busy-system idle result as inconclusive, not as proof of a feedback loop.

For UI validation, use DEV for TCP applications and switch to ALL for UDP/UNIX checks. Open a development server on a port below 32768, verify that it appears, then stop it and verify that it disappears while the menu remains open. Check IPv4, IPv6 when available, UDP, filesystem and abstract UNIX sockets. Replace one listener with another so the total socket count stays unchanged. Verify that the badge updates without opening the menu, and test preference persistence after closing/reopening the development session.

Test container stop only against a disposable container: Cancel must leave it running; Stop Container must target the displayed container; permission failures must appear in a notification. The automated suites mock stop commands and do not stop real containers.

## Failure behavior

A failed snapshot retains the last successful list and does not emit false close notifications. Missing/disconnected event streams show a warning; there is no periodic fallback. No socket scans occur solely to update memory usage or uptime. Loss notifications, reconnect, menu open and explicit refresh resynchronize the snapshot.

This is an event-triggered current-state view, not a complete socket history. Very short-lived sockets can be gone before the snapshot. Ordinary socket lifecycle events are covered; ownership-only changes such as inheriting an already open listener across fork/exec may require a manual/menu refresh. Unsupported kernel probes, network namespaces outside the desktop's namespace, and event losses need the same explicit operational checks as other BPF tooling.

## Developer view

DEV includes TCP sockets belonging to known runtimes/databases, Docker proxies or common development ports. Desktop/browser service names are excluded before the port heuristic. It is deliberately a heuristic. ALL shows every discovered protocol and does not erase the cached data.

Each TCP card has Show in DEV / Hide from DEV. Overrides are keyed by protocol, port and process name, so they survive a restart with a new PID. Matching processes sharing that name and port share the override. Notification baselines reset on preference changes to avoid announcing old services as new. DEV view changes, includes and excludes are stored in GSettings alongside the existing notification and protocol preferences.

## Performance measurement

Measure the installed service cgroup after startup and a warmup period. Record both `memory.current` and `cpu.stat` over an idle interval. The native service should have a single daemon process rather than a bpftrace child. Live-event tests measure functional coverage; a performance sample is machine/workload-specific and does not imply zero CPU under events. Refreshing a protocol still uses one asynchronous ss snapshot; this implementation does not claim a fully incremental socket database.

Observed on the development host on 2026-11-16 after the live checks: the previous bpftrace service used 67.07 MiB of cgroup memory; native Aya used 4.45 MiB. Separate 3-second idle samples showed 3.025 ms and 0 ms of service CPU respectively. These are short observations of the service cgroup, not whole-system or kernel BPF overhead measurements, and not a guarantee for other workloads.
