# Port & Process Killer (GNOME Shell Extension)

A compact, developer-focused GNOME Shell extension for monitoring active local sockets (`TCP`, `UDP`, `UNIX`) in real time and terminating processes directly from the Top Bar.

## Features

- **Real-time Socket Monitoring**: Event-driven socket tracking using `ss` (`ss -tE`, `ss -tlpnH`, `ss -ulpnH`, `ss -xlpnH`) with a live badge counter in the GNOME Top Bar.
- **Docker Container Detection**: Automatically resolves `docker-proxy` / `rootlessport` listeners to their active Docker container names (e.g., `docker (postgres)`).
- **Optional Desktop Notifications**: Toggle `Notify: ON / OFF` from the menu header to receive native GNOME desktop alerts whenever a TCP/UDP port opens or closes.
- **Live Search & Protocol Filtering**: Instant search by port, process name, Docker container name, PID, command line, or bind address, plus quick protocol toggles (`ALL`, `TCP`, `UDP`, `UNIX`).
- **Process Details**: Displays PID, PPID, RSS memory usage, uptime, bind address/socket path, and truncated command line.
- **Quick Actions**: `Open` (`http://localhost:<port>`), `Copy URL`, `Copy PID`, graceful `Kill` (`SIGTERM`), `Force Kill` (`SIGKILL`), and bulk `Kill All` / `Force All`.
- **Native GNOME Styling**: Built with GNOME Shell `St` widgets and compact spacing that adapts naturally to Light/Dark themes.
- **System Appearance**: Compact charcoal cards in Dark mode and neutral white cards in Light mode. Follows GNOME Settings → Appearance automatically, including changes while the menu is open.

## Installation

```bash
git clone https://github.com/a1d-latifaj/gnome-port-killer.git
cd gnome-port-killer
./install.sh
```

Then **Log Out / Log In** to reload GNOME Shell extensions, and enable it if needed:

```bash
gnome-extensions enable port-killer@dev
```

## Uninstall

```bash
cd ~/projects/gnome-port-killer
./uninstall.sh
```

## License

This project is licensed under the [MIT License](LICENSE).
