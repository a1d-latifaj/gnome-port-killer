# Port & Process Killer

A compact GNOME Shell extension for inspecting local TCP, UDP and UNIX sockets, opening development URLs, and stopping processes or Docker containers from the Top Bar.

## Features

- **Developer view:** DEV is the default, focused on TCP runtimes, common development ports, databases and Docker proxies. ALL retains the complete diagnostic view. Show/Hide from DEV overrides persist per process name, protocol and port. Classification is a convenience heuristic, not proof that a service is development-only.
- **Event-driven updates:** a Rust helper bridges Linux eBPF events to GNOME. Native events identify the changed protocol so only that protocol is refreshed. DEV ignores UDP/UNIX events. Process details are read only while the menu is open. Menu opening, explicit refresh and process actions also request snapshots. There is no periodic socket scan.
- **Docker actions:** published proxy listeners show container names. `Stop Container` requires confirmation and targets the full container ID. Docker lifecycle changes are tracked through `docker events`.
- **Saved preferences:** notification state, protocol filter, DEV/ALL selection and developer-view overrides persist in GSettings. Search text stays temporary.
- **Search and details:** filter by protocol, port, process, PID, container, address or command. Inspect PPID, memory and uptime.
- **Process actions:** copy PID, send SIGTERM/SIGKILL, and confirm bulk actions. Docker proxies are excluded from process kills. TCP entries include Open and Copy URL.
- **System appearance:** compact light and dark palettes follow GNOME Settings → Appearance automatically.

## Layout

```text
extension/             GNOME UI, async runtime, parsers, styles and GSettings
helper/                Rust event daemon/client and embedded eBPF program
packaging/             systemd service
scripts/               helper build, installation and removal
tests/                 JavaScript, GJS and live integration checks
docs/                  architecture and verification instructions
```

## Requirements

GNOME Shell 46–50, `ss` (iproute2), and `glib-compile-schemas` are required for the extension. Docker support additionally needs the Docker CLI and access to its daemon.

Live kernel events require Linux BTF at `/sys/kernel/btf/vmlinux`, the probes used by the embedded CO-RE program and the system event service. The backend attaches to bind/listen, TCP listener state changes, and UDP/UNIX socket release; it does not rely on `ss -E` to discover newly opened ports.

The event bridge uses Rust and Aya to load embedded eBPF bytecode directly. The small kernel program is compiled from C with CO-RE relocations. Rust/Cargo, Clang and bpftool are needed **only to build it**. A prebuilt executable needs none of these tools, no bpftrace and no libbpf at runtime. It still needs kernel BTF and permission to attach the probes. Prebuilt binaries must match the machine architecture and support its libc version.

## Install

Clone the repository and install the extension:

```bash
git clone https://github.com/a1d-latifaj/gnome-port-killer.git
cd gnome-port-killer
./install.sh
```

Install the event service using one of the following paths. Its installer uses sudo to copy the root-owned binary/unit and enable/restart the service. It does not log out or restart GNOME.

### Build the helper locally

Install Rust/Cargo first, plus Clang and bpftool. Build on the target architecture; the current build script supports x86_64 and AArch64 and generates kernel type definitions from the build host's BTF.

```bash
# Additional build tools on Ubuntu:
sudo apt install clang bpftool
# Fetch dependencies before the installer's locked, offline build:
cargo fetch --manifest-path helper/Cargo.toml --locked
./scripts/install-helper.sh
```

### Use a prebuilt helper

A compatible prebuilt executable needs no Rust, Cargo, Clang, bpftool or bpftrace on the user's machine:

```bash
./scripts/install-helper.sh /path/to/port-killer-events
```

The prebuilt path accepts an executable you already have; this repository does not automatically download or publish release binaries.

After installing, enable `port-killer@dev` in GNOME Extensions. An already loaded JavaScript module requires a new GNOME Shell session. To test without logging out on GNOME 49/50:

```bash
# Ubuntu: sudo apt install mutter-dev-bin, if missing
dbus-run-session -- gnome-shell --devkit --wayland
```

Enable the extension from a terminal inside that development session. Close and reopen only the development session after JavaScript changes.

If the event service is missing or disconnected, the menu displays a warning. The ↻ button refreshes the list and reconnects the event streams. There is no silent polling fallback.

## Verification

The JavaScript/GSettings suites require Node.js 22+, GJS, Python 3 and GLib schema tools. Rust checks also need the build tools above and fetched crates. Rust socket tests run as a normal user but require permission to create local UNIX sockets.

```bash
python3 tests/run.py
cargo test --manifest-path helper/Cargo.toml --locked --offline
cargo clippy --manifest-path helper/Cargo.toml --locked --offline --all-targets -- -D warnings
```

Build the release helper before checking kernel probe attachment. This check needs administrator access and immediately detaches after validation:

```bash
./scripts/build-helper.sh
sudo helper/target/release/port-killer-events --check
```

See [architecture and live checks](docs/event-monitor.md) for the event flow, privilege boundary and end-to-end verification.

## Limitations

Only sockets with visible owning PIDs are listed. The existing port cutoff (below 32768) is retained. Docker actions require a visible proxy listener; Docker NAT-only publishing without a proxy is not discovered from `ss`. Memory and uptime update when a snapshot is requested, not on a clock. Very short-lived sockets can disappear before the debounced snapshot and are not an audit log.

The BPF program uses Linux kernel probes, including `__sock_release`, whose availability and structure fields must be checked after kernel upgrades. If the tracer fails, the extension reports the disconnected backend and keeps manual refresh available.

## Uninstall

```bash
./uninstall.sh
# Optional: also remove the system event service
./scripts/uninstall-helper.sh
```

GSettings preferences remain available for a future installation.

## License

[MIT](LICENSE).
