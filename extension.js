import St from 'gi://St';
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import Clutter from 'gi://Clutter';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import * as PanelMenu from 'resource:///org/gnome/shell/ui/panelMenu.js';
import * as PopupMenu from 'resource:///org/gnome/shell/ui/popupMenu.js';
import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';

const PROTOS = ['ALL', 'TCP', 'UDP', 'UNIX'];

export default class PortKillerExtension extends Extension {
    enable() {
        this._indicator = new PanelMenu.Button(0.0, this.metadata.name, false);
        this._lastCount = -1;
        this._searchQuery = '';
        this._protoFilter = 'ALL';
        this._notifyEnabled = false;
        this._knownSockets = new Map();
        this._dockerPortMap = new Map();
        this._dockerCacheTime = 0;
        this._cancellable = new Gio.Cancellable();
        this._protoButtons = {};

        this._panelBox = new St.BoxLayout({style_class: 'pk-panel-box'});
        const icon = new St.Icon({
            icon_name: 'utilities-terminal-symbolic',
            style_class: 'system-status-icon',
        });
        this._label = new St.Label({
            text: '0',
            y_align: Clutter.ActorAlign.CENTER,
            style_class: 'pk-panel-label',
        });

        this._panelBox.add_child(icon);
        this._panelBox.add_child(this._label);
        this._indicator.add_child(this._panelBox);

        this._buildMenuSkeleton();

        this._indicator.menu.connect('open-state-changed', (_menu, isOpen) => {
            if (isOpen) {
                this._refreshResults();
                if (this._searchEntry) {
                    GLib.idle_add(GLib.PRIORITY_DEFAULT, () => {
                        if (this._searchEntry && this._indicator?.menu?.isOpen) {
                            global.stage.set_key_focus(this._searchEntry.clutter_text);
                        }
                        return GLib.SOURCE_REMOVE;
                    });
                }
            }
        });

        Main.panel.addToStatusArea(this.uuid, this._indicator, 1, 'left');

        const initialItems = this._getListeningSockets(false);
        this._syncKnownSockets(initialItems, false);
        this._refreshResults();
        this._startKernelSocketMonitor();
    }

    _buildMenuSkeleton() {
        this._indicator.menu.removeAll();

        const menuItem = new PopupMenu.PopupBaseMenuItem({reactive: false});
        this._body = new St.BoxLayout({
            vertical: true,
            style_class: 'pk-menu-body',
        });

        // Header bar
        const headerBar = new St.BoxLayout({
            style_class: 'pk-header-bar',
            x_expand: true,
        });
        this._titleLabel = new St.Label({
            text: 'Active Sockets (0/0)',
            style_class: 'pk-header-title',
            y_align: Clutter.ActorAlign.CENTER,
            x_expand: true,
        });
        headerBar.add_child(this._titleLabel);

        this._notifyBtn = new St.Button({
            label: 'Notify: OFF',
            style_class: 'button pk-pill-btn',
            can_focus: true,
        });
        this._notifyBtn.connect('clicked', () => {
            this._notifyEnabled = !this._notifyEnabled;
            this._updateNotifyButtonStyle();
            if (this._notifyEnabled) {
                const current = this._getListeningSockets(false);
                this._syncKnownSockets(current, false);
            }
        });
        headerBar.add_child(this._notifyBtn);

        this._headerActionsBox = new St.BoxLayout({
            style_class: 'pk-tile-actions',
        });
        headerBar.add_child(this._headerActionsBox);
        this._body.add_child(headerBar);

        // Search + Proto filter row (persistent - never destroyed on typing)
        const filterRow = new St.BoxLayout({
            style_class: 'pk-filter-row',
            x_expand: true,
        });
        this._searchEntry = new St.Entry({
            hint_text: 'Search port, process, PID, command...',
            text: this._searchQuery,
            style_class: 'pk-search-entry',
            x_expand: true,
            can_focus: true,
        });
        this._searchEntry.clutter_text.connect('text-changed', () => {
            this._searchQuery = this._searchEntry.get_text();
            this._refreshResults();
        });
        filterRow.add_child(this._searchEntry);

        for (const proto of PROTOS) {
            const pBtn = new St.Button({
                label: proto,
                style_class: 'button pk-proto-btn',
                can_focus: true,
            });
            pBtn.connect('clicked', () => {
                this._protoFilter = proto;
                this._updateProtoButtonStyles();
                this._refreshResults();
            });
            this._protoButtons[proto] = pBtn;
            filterRow.add_child(pBtn);
        }
        this._updateProtoButtonStyles();
        this._body.add_child(filterRow);

        // Scrollable results area (only this container is rebuilt)
        this._scrollView = new St.ScrollView({
            style: 'max-height: 460px;',
            hscrollbar_policy: St.PolicyType.NEVER,
            vscrollbar_policy: St.PolicyType.AUTOMATIC,
        });
        this._listBox = new St.BoxLayout({
            vertical: true,
            x_expand: true,
        });
        this._scrollView.add_child(this._listBox);
        this._body.add_child(this._scrollView);

        menuItem.add_child(this._body);
        this._indicator.menu.addMenuItem(menuItem);
    }

    _updateNotifyButtonStyle() {
        if (!this._notifyBtn) return;
        this._notifyBtn.set_label(this._notifyEnabled ? 'Notify: ON' : 'Notify: OFF');
        this._notifyBtn.style_class = this._notifyEnabled
            ? 'button pk-pill-btn pk-pill-accent'
            : 'button pk-pill-btn';
    }

    _updateProtoButtonStyles() {
        for (const proto of PROTOS) {
            const btn = this._protoButtons[proto];
            if (!btn) continue;
            btn.style_class = this._protoFilter === proto
                ? 'button pk-proto-btn pk-pill-accent'
                : 'button pk-proto-btn';
        }
    }

    _getDockerContainerMap() {
        const now = GLib.get_monotonic_time();
        if (now - this._dockerCacheTime < 3000000) {
            return this._dockerPortMap;
        }
        this._dockerCacheTime = now;

        const map = new Map();
        try {
            const [ok, stdout] = GLib.spawn_command_line_sync('docker ps --format "{{.Names}}|{{.Ports}}"');
            if (!ok || !stdout) {
                this._dockerPortMap = map;
                return map;
            }
            const output = new TextDecoder().decode(stdout);
            for (const line of output.split('\n')) {
                if (!line.includes('|')) continue;
                const [name, portsStr] = line.split('|');
                const matches = portsStr.matchAll(/:(\d+)->/g);
                for (const m of matches) {
                    map.set(m[1], name.trim());
                }
            }
        } catch (_e) {}

        this._dockerPortMap = map;
        return map;
    }

    _syncKnownSockets(items, emitNotifications = true) {
        const nextMap = new Map();
        for (const item of items) {
            if (item.proto === 'UNIX') continue;
            const key = `${item.proto}:${item.portNum}:${item.pid}`;
            nextMap.set(key, item);
        }

        if (emitNotifications && this._notifyEnabled) {
            for (const [key, item] of nextMap.entries()) {
                if (!this._knownSockets.has(key)) {
                    Main.notify(
                        'Port Opened',
                        `${item.proto} ${item.endpoint} opened by ${item.displayName} (PID ${item.pid})`
                    );
                }
            }
            for (const [key, item] of this._knownSockets.entries()) {
                if (!nextMap.has(key)) {
                    Main.notify(
                        'Port Closed',
                        `${item.proto} ${item.endpoint} closed (${item.displayName})`
                    );
                }
            }
        }

        this._knownSockets = nextMap;
    }

    _startKernelSocketMonitor() {
        try {
            this._monitorProc = new Gio.Subprocess({
                argv: ['ss', '-tE'],
                flags: Gio.SubprocessFlags.STDOUT_PIPE | Gio.SubprocessFlags.STDERR_SILENCE,
            });
            this._monitorProc.init(this._cancellable);

            const stdout = this._monitorProc.get_stdout_pipe();
            this._dataStream = new Gio.DataInputStream({base_stream: stdout});
            this._readNextSocketEvent();
        } catch (e) {
            logError(e, 'PortKiller monitor error');
        }
    }

    _readNextSocketEvent() {
        if (!this._dataStream || this._cancellable.is_cancelled()) return;

        this._dataStream.read_line_async(GLib.PRIORITY_DEFAULT, this._cancellable, (stream, res) => {
            try {
                const [line] = stream.read_line_finish_utf8(res);
                if (line !== null) {
                    if (!this._debounceId) {
                        this._debounceId = GLib.timeout_add(GLib.PRIORITY_DEFAULT, 120, () => {
                            this._debounceId = null;
                            this._updateBadge();
                            return GLib.SOURCE_REMOVE;
                        });
                    }
                    this._readNextSocketEvent();
                }
            } catch (e) {
                if (!e.matches(Gio.IOErrorEnum, Gio.IOErrorEnum.CANCELLED)) {
                    logError(e, 'PortKiller read error');
                }
            }
        });
    }

    _getProcDetails(pid) {
        let cmd = '';
        let ppid = '?';
        let rssMb = '?';
        let uptime = '';

        try {
            const [okCmd, cmdBytes] = GLib.file_get_contents(`/proc/${pid}/cmdline`);
            if (okCmd && cmdBytes) {
                const raw = new TextDecoder().decode(cmdBytes).replace(/\0+/g, ' ').trim();
                cmd = raw.length > 78 ? `${raw.slice(0, 75)}...` : raw;
            }
        } catch (_e) {}

        try {
            const [okStat, statBytes] = GLib.file_get_contents(`/proc/${pid}/status`);
            if (okStat && statBytes) {
                const text = new TextDecoder().decode(statBytes);
                const ppidMatch = text.match(/PPid:\s+(\d+)/);
                if (ppidMatch) ppid = ppidMatch[1];

                const rssMatch = text.match(/VmRSS:\s+(\d+)\s+kB/);
                if (rssMatch) {
                    rssMb = `${Math.round(parseInt(rssMatch[1], 10) / 1024)} MB`;
                }
            }
        } catch (_e) {}

        try {
            const info = Gio.File.new_for_path(`/proc/${pid}`).query_info(
                'time::modified',
                Gio.FileQueryInfoFlags.NONE,
                null
            );
            const startSec = info.get_modification_date_time().to_unix();
            const diff = Math.max(0, Math.floor(GLib.get_real_time() / 1000000) - startSec);
            if (diff < 60) uptime = `${diff}s`;
            else if (diff < 3600) uptime = `${Math.floor(diff / 60)}m`;
            else uptime = `${Math.floor(diff / 3600)}h ${Math.floor((diff % 3600) / 60)}m`;
        } catch (_e) {}

        return {cmd, ppid, rssMb, uptime};
    }

    _parseInetOutput(cmdStr, proto, dockerMap) {
        const results = [];
        try {
            const [ok, stdout] = GLib.spawn_command_line_sync(cmdStr);
            if (!ok || !stdout) return results;

            const output = new TextDecoder().decode(stdout);
            const lines = output.split('\n');

            for (const line of lines) {
                if (proto === 'TCP' && !line.includes('LISTEN')) continue;

                const addrPortMatch = line.match(/(\S+):(\d+)\s+/);
                const procMatch = line.match(/users:\(\("([^"]+)",pid=(\d+)/);

                if (addrPortMatch && procMatch) {
                    const bindAddr = addrPortMatch[1].replace(/^\[|\]$/g, '');
                    const port = addrPortMatch[2];
                    const portNum = parseInt(port, 10);
                    if (portNum >= 32768) continue;

                    const name = procMatch[1];
                    const pid = procMatch[2];
                    const containerName = (name === 'docker-proxy' || name === 'rootlessport')
                        ? dockerMap.get(port)
                        : null;
                    const displayName = containerName ? `docker (${containerName})` : name;

                    results.push({
                        proto,
                        endpoint: `:${port}`,
                        portNum,
                        url: `http://localhost:${port}`,
                        bindAddr,
                        name,
                        displayName,
                        containerName: containerName || '',
                        pid,
                    });
                }
            }
        } catch (_e) {}
        return results;
    }

    _parseUnixOutput() {
        const results = [];
        try {
            const [ok, stdout] = GLib.spawn_command_line_sync('ss -xlpnH');
            if (!ok || !stdout) return results;

            const output = new TextDecoder().decode(stdout);
            const lines = output.split('\n');

            for (const line of lines) {
                const procMatch = line.match(/users:\(\("([^"]+)",pid=(\d+)/);
                const pathMatch = line.match(/\s+(\/[^\s]+|@[^\s]+)\s+\d+\s+users:/);
                if (procMatch && pathMatch) {
                    const sockPath = pathMatch[1];
                    const shortSock = sockPath.length > 22 ? `...${sockPath.slice(-19)}` : sockPath;
                    const name = procMatch[1];
                    results.push({
                        proto: 'UNIX',
                        endpoint: shortSock,
                        portNum: 999999,
                        url: null,
                        bindAddr: sockPath,
                        name,
                        displayName: name,
                        containerName: '',
                        pid: procMatch[2],
                    });
                }
            }
        } catch (_e) {}
        return results;
    }

    _getListeningSockets(applyFilters = false) {
        try {
            const dockerMap = this._getDockerContainerMap();
            const rawList = [
                ...this._parseInetOutput('ss -tlpnH', 'TCP', dockerMap),
                ...this._parseInetOutput('ss -ulpnH', 'UDP', dockerMap),
                ...this._parseUnixOutput(),
            ];

            const seen = new Set();
            const items = [];

            for (const item of rawList) {
                const key = `${item.proto}-${item.bindAddr}-${item.endpoint}-${item.pid}`;
                if (seen.has(key)) continue;
                seen.add(key);

                const details = this._getProcDetails(item.pid);
                const fullEntry = {...item, ...details};

                if (applyFilters) {
                    if (this._protoFilter !== 'ALL' && fullEntry.proto !== this._protoFilter) {
                        continue;
                    }
                    if (this._searchQuery.trim() !== '') {
                        const q = this._searchQuery.trim().toLowerCase();
                        const hay = `${fullEntry.endpoint} ${fullEntry.displayName} ${fullEntry.name} ${fullEntry.containerName} ${fullEntry.pid} ${fullEntry.cmd} ${fullEntry.bindAddr}`.toLowerCase();
                        if (!hay.includes(q)) continue;
                    }
                }

                items.push(fullEntry);
            }

            return items.sort((a, b) => a.portNum - b.portNum);
        } catch (e) {
            logError(e, 'PortKiller error');
            return [];
        }
    }

    _updateBadge() {
        const allItems = this._getListeningSockets(false);
        const count = allItems.length;

        this._syncKnownSockets(allItems, true);

        if (count !== this._lastCount) {
            this._lastCount = count;
            this._label.set_text(`${count}`);

            if (this._indicator.menu.isOpen) {
                this._refreshResults();
            }
        }
    }

    _killProc(pid, force = false) {
        const sig = force ? '-9' : '-15';
        GLib.spawn_command_line_sync(`kill ${sig} ${pid}`);
        GLib.timeout_add(GLib.PRIORITY_DEFAULT, 220, () => {
            this._refreshResults();
            return GLib.SOURCE_REMOVE;
        });
    }

    _buildSocketTile(item) {
        const tile = new St.BoxLayout({
            vertical: true,
            style_class: 'pk-tile',
            x_expand: true,
        });

        // Row 1: Port pill + Proto tag + Process name
        const topRow = new St.BoxLayout({style_class: 'pk-tile-top', x_expand: true});

        const endpointPill = new St.Label({
            text: item.endpoint,
            style_class: 'pk-port-pill',
            y_align: Clutter.ActorAlign.CENTER,
        });

        const protoTag = new St.Label({
            text: item.proto,
            style_class: 'pk-proto-tag',
            y_align: Clutter.ActorAlign.CENTER,
        });

        const procTitle = new St.Label({
            text: `${item.displayName}`,
            style_class: 'pk-proc-title',
            y_align: Clutter.ActorAlign.CENTER,
            x_expand: true,
        });

        topRow.add_child(endpointPill);
        topRow.add_child(protoTag);
        topRow.add_child(procTitle);
        tile.add_child(topRow);

        // Row 2: Metadata line (PID, PPID, RSS, Uptime, Bind address)
        const procSub = new St.Label({
            text: `PID ${item.pid} • PPID ${item.ppid} • ${item.rssMb} • ${item.uptime} • ${item.bindAddr}`,
            style_class: 'pk-proc-sub',
            x_expand: true,
        });
        tile.add_child(procSub);

        // Row 3: Command line
        if (item.cmd) {
            const cmdLabel = new St.Label({
                text: item.cmd,
                style_class: 'pk-cmd-box',
                x_expand: true,
            });
            tile.add_child(cmdLabel);
        }

        // Row 4: Action buttons
        const actionsRow = new St.BoxLayout({
            style_class: 'pk-tile-actions',
            x_expand: true,
        });

        if (item.url) {
            const openBtn = new St.Button({
                label: 'Open',
                style_class: 'button pk-mini-btn',
                can_focus: true,
            });
            openBtn.connect('clicked', () => {
                GLib.spawn_command_line_async(`gio open ${item.url}`);
            });
            actionsRow.add_child(openBtn);

            const copyUrlBtn = new St.Button({
                label: 'Copy URL',
                style_class: 'button pk-mini-btn',
                can_focus: true,
            });
            copyUrlBtn.connect('clicked', () => {
                St.Clipboard.get_default().set_text(St.ClipboardType.CLIPBOARD, item.url);
                copyUrlBtn.set_label('Copied');
                GLib.timeout_add(GLib.PRIORITY_DEFAULT, 900, () => {
                    copyUrlBtn.set_label('Copy URL');
                    return GLib.SOURCE_REMOVE;
                });
            });
            actionsRow.add_child(copyUrlBtn);
        }

        const copyPidBtn = new St.Button({
            label: 'Copy PID',
            style_class: 'button pk-mini-btn',
            can_focus: true,
        });
        copyPidBtn.connect('clicked', () => {
            St.Clipboard.get_default().set_text(St.ClipboardType.CLIPBOARD, `${item.pid}`);
            copyPidBtn.set_label('Copied');
            GLib.timeout_add(GLib.PRIORITY_DEFAULT, 900, () => {
                copyPidBtn.set_label('Copy PID');
                return GLib.SOURCE_REMOVE;
            });
        });
        actionsRow.add_child(copyPidBtn);

        const spacer = new St.Widget({x_expand: true});
        actionsRow.add_child(spacer);

        const killBtn = new St.Button({
            label: 'Kill',
            style_class: 'button pk-mini-btn pk-mini-kill',
            can_focus: true,
        });
        killBtn.connect('clicked', () => this._killProc(item.pid, false));

        const forceKillBtn = new St.Button({
            label: 'Force Kill',
            style_class: 'button pk-mini-btn pk-mini-force',
            can_focus: true,
        });
        forceKillBtn.connect('clicked', () => this._killProc(item.pid, true));

        actionsRow.add_child(killBtn);
        actionsRow.add_child(forceKillBtn);
        tile.add_child(actionsRow);

        return tile;
    }

    _refreshResults() {
        try {
            const allItems = this._getListeningSockets(false);
            const filteredItems = this._getListeningSockets(true);

            this._lastCount = allItems.length;
            this._label.set_text(`${allItems.length}`);

            if (this._titleLabel) {
                this._titleLabel.set_text(`Active Sockets (${filteredItems.length}/${allItems.length})`);
            }

            if (this._headerActionsBox) {
                this._headerActionsBox.destroy_all_children();
                if (filteredItems.length > 0) {
                    const killAllBtn = new St.Button({
                        label: 'Kill All',
                        style_class: 'button pk-pill-btn pk-mini-kill',
                        can_focus: true,
                    });
                    killAllBtn.connect('clicked', () => {
                        const pids = [...new Set(filteredItems.map(p => p.pid))].join(' ');
                        GLib.spawn_command_line_sync(`kill -15 ${pids}`);
                        GLib.timeout_add(GLib.PRIORITY_DEFAULT, 220, () => {
                            this._refreshResults();
                            return GLib.SOURCE_REMOVE;
                        });
                    });

                    const forceAllBtn = new St.Button({
                        label: 'Force All',
                        style_class: 'button pk-pill-btn pk-pill-accent',
                        can_focus: true,
                    });
                    forceAllBtn.connect('clicked', () => {
                        const pids = [...new Set(filteredItems.map(p => p.pid))].join(' ');
                        GLib.spawn_command_line_sync(`kill -9 ${pids}`);
                        GLib.timeout_add(GLib.PRIORITY_DEFAULT, 220, () => {
                            this._refreshResults();
                            return GLib.SOURCE_REMOVE;
                        });
                    });

                    this._headerActionsBox.add_child(killAllBtn);
                    this._headerActionsBox.add_child(forceAllBtn);
                }
            }

            if (!this._listBox) return;
            this._listBox.destroy_all_children();

            if (filteredItems.length === 0) {
                const emptyLabel = new St.Label({
                    text: 'No active sockets match the current filter.',
                    style_class: 'pk-empty-tile',
                    x_expand: true,
                });
                this._listBox.add_child(emptyLabel);
            } else {
                for (const item of filteredItems) {
                    this._listBox.add_child(this._buildSocketTile(item));
                }
            }
        } catch (e) {
            logError(e, 'PortKiller refreshResults error');
        }
    }

    disable() {
        if (this._debounceId) {
            GLib.Source.remove(this._debounceId);
            this._debounceId = null;
        }
        if (this._cancellable) {
            this._cancellable.cancel();
            this._cancellable = null;
        }
        if (this._dataStream) {
            try {
                this._dataStream.close(null);
            } catch (_e) {}
            this._dataStream = null;
        }
        if (this._monitorProc) {
            try {
                this._monitorProc.send_signal(15);
                this._monitorProc.force_exit();
            } catch (_e) {}
            this._monitorProc = null;
        }
        if (this._indicator) {
            this._indicator.destroy();
            this._indicator = null;
        }
        this._searchEntry = null;
        this._listBox = null;
        this._titleLabel = null;
        this._headerActionsBox = null;
        this._notifyBtn = null;
        this._knownSockets.clear();
        this._dockerPortMap.clear();
    }
}
