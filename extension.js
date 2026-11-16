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
        this._cancellable = new Gio.Cancellable();

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

        this._indicator.menu.connect('open-state-changed', (_menu, isOpen) => {
            if (isOpen) {
                this._renderMenu();
            }
        });

        Main.panel.addToStatusArea(this.uuid, this._indicator, 1, 'left');

        this._renderMenu();
        this._startKernelSocketMonitor();
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
                        this._debounceId = GLib.timeout_add(GLib.PRIORITY_DEFAULT, 80, () => {
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

    _parseInetOutput(cmdStr, proto) {
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
                    results.push({
                        proto,
                        endpoint: `:${port}`,
                        portNum,
                        url: `http://localhost:${port}`,
                        bindAddr,
                        name,
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
                    results.push({
                        proto: 'UNIX',
                        endpoint: shortSock,
                        portNum: 999999,
                        url: null,
                        bindAddr: sockPath,
                        name: procMatch[1],
                        pid: procMatch[2],
                    });
                }
            }
        } catch (_e) {}
        return results;
    }

    _getListeningSockets(applyFilters = false) {
        try {
            const rawList = [
                ...this._parseInetOutput('ss -tlpnH', 'TCP'),
                ...this._parseInetOutput('ss -ulpnH', 'UDP'),
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
                        const hay = `${fullEntry.endpoint} ${fullEntry.name} ${fullEntry.pid} ${fullEntry.cmd} ${fullEntry.bindAddr}`.toLowerCase();
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

        if (count !== this._lastCount) {
            this._lastCount = count;
            this._label.set_text(`${count}`);

            if (this._indicator.menu.isOpen) {
                this._renderMenu();
            }
        }
    }

    _killProc(pid, force = false) {
        const sig = force ? '-9' : '-15';
        GLib.spawn_command_line_sync(`kill ${sig} ${pid}`);
        GLib.timeout_add(GLib.PRIORITY_DEFAULT, 220, () => {
            this._renderMenu();
            return GLib.SOURCE_REMOVE;
        });
    }

    _buildSocketTile(item) {
        const tile = new St.BoxLayout({
            vertical: true,
            style_class: 'pk-tile',
            x_expand: true,
        });

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
            text: `${item.name}`,
            style_class: 'pk-proc-title',
            y_align: Clutter.ActorAlign.CENTER,
            x_expand: true,
        });

        const procSub = new St.Label({
            text: `PID ${item.pid} • PPID ${item.ppid} • ${item.rssMb} • ${item.uptime} • ${item.bindAddr}`,
            style_class: 'pk-proc-sub',
            y_align: Clutter.ActorAlign.CENTER,
        });

        topRow.add_child(endpointPill);
        topRow.add_child(protoTag);
        topRow.add_child(procTitle);
        topRow.add_child(procSub);
        tile.add_child(topRow);

        if (item.cmd) {
            const cmdLabel = new St.Label({
                text: item.cmd,
                style_class: 'pk-cmd-box',
                x_expand: true,
            });
            tile.add_child(cmdLabel);
        }

        const actionsRow = new St.BoxLayout({
            style_class: 'pk-tile-actions',
            x_expand: true,
        });

        const spacer = new St.Widget({x_expand: true});
        actionsRow.add_child(spacer);

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

        actionsRow.add_child(copyPidBtn);
        actionsRow.add_child(killBtn);
        actionsRow.add_child(forceKillBtn);
        tile.add_child(actionsRow);

        return tile;
    }

    _renderMenu() {
        try {
            const allItems = this._getListeningSockets(false);
            const filteredItems = this._getListeningSockets(true);

            this._lastCount = allItems.length;
            this._label.set_text(`${allItems.length}`);

            this._indicator.menu.removeAll();

            const menuItem = new PopupMenu.PopupBaseMenuItem({reactive: false});
            const body = new St.BoxLayout({
                vertical: true,
                style_class: 'pk-menu-body',
            });

            // Header bar
            const headerBar = new St.BoxLayout({
                style_class: 'pk-header-bar',
                x_expand: true,
            });
            const title = new St.Label({
                text: `Active Sockets (${filteredItems.length}/${allItems.length})`,
                style_class: 'pk-header-title',
                y_align: Clutter.ActorAlign.CENTER,
                x_expand: true,
            });
            headerBar.add_child(title);

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
                        this._renderMenu();
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
                        this._renderMenu();
                        return GLib.SOURCE_REMOVE;
                    });
                });

                headerBar.add_child(killAllBtn);
                headerBar.add_child(forceAllBtn);
            }
            body.add_child(headerBar);

            // Search + Proto filter row
            const filterRow = new St.BoxLayout({
                style_class: 'pk-filter-row',
                x_expand: true,
            });
            const searchEntry = new St.Entry({
                hint_text: 'Kerko porte, proces, PID, komande...',
                text: this._searchQuery,
                style_class: 'pk-search-entry',
                x_expand: true,
                can_focus: true,
            });
            searchEntry.clutter_text.connect('text-changed', () => {
                this._searchQuery = searchEntry.get_text();
                this._renderMenu();
            });
            filterRow.add_child(searchEntry);

            for (const proto of PROTOS) {
                const pBtn = new St.Button({
                    label: proto,
                    style_class: this._protoFilter === proto
                        ? 'button pk-proto-btn pk-pill-accent'
                        : 'button pk-proto-btn',
                    can_focus: true,
                });
                pBtn.connect('clicked', () => {
                    this._protoFilter = proto;
                    this._renderMenu();
                });
                filterRow.add_child(pBtn);
            }
            body.add_child(filterRow);

            if (filteredItems.length === 0) {
                const emptyLabel = new St.Label({
                    text: 'Asnje socket aktiv nuk perputhet me filtrin.',
                    style_class: 'pk-empty-tile',
                    x_expand: true,
                });
                body.add_child(emptyLabel);
            } else {
                const scrollView = new St.ScrollView({
                    style: 'max-height: 460px;',
                    hscrollbar_policy: St.PolicyType.NEVER,
                    vscrollbar_policy: St.PolicyType.AUTOMATIC,
                });
                const listBox = new St.BoxLayout({
                    vertical: true,
                    x_expand: true,
                });

                for (const item of filteredItems) {
                    listBox.add_child(this._buildSocketTile(item));
                }

                scrollView.add_child(listBox);
                body.add_child(scrollView);
            }

            menuItem.add_child(body);
            this._indicator.menu.addMenuItem(menuItem);
        } catch (e) {
            logError(e, 'PortKiller renderMenu error');
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
        if (this._monitorProc) {
            this._monitorProc.force_exit();
            this._monitorProc = null;
        }
        if (this._indicator) {
            this._indicator.destroy();
            this._indicator = null;
        }
    }
}
