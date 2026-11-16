import St from 'gi://St';
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import Clutter from 'gi://Clutter';
import Pango from 'gi://Pango';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import * as PanelMenu from 'resource:///org/gnome/shell/ui/panelMenu.js';
import * as ModalDialog from 'resource:///org/gnome/shell/ui/modalDialog.js';
import * as Dialog from 'resource:///org/gnome/shell/ui/dialog.js';
import {parseDockerBindings, parseInet, parseUnix, uniqueSockets, filterSockets, isDockerProxy, developerKey, isDeveloperSocket, viewSockets} from './sockets.js';
import {createSession, cancelSession, after, runCommand, readFile, watchCommand} from './runtime.js';
import * as PopupMenu from 'resource:///org/gnome/shell/ui/popupMenu.js';
import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';

const PROTOS = ['ALL', 'TCP', 'UDP', 'UNIX'];

export default class PortKillerExtension extends Extension {
    enable() {
        this._settings = this.getSettings();
        this._session = createSession();
        this._items = [];
        this._initialized = false;
        this._snapshotError = '';
        this._eventError = 'Connecting to event helper…';
        this._eventMonitor = null;
        this._dockerMonitor = null;
        this._dockerError = '';
        this._dockerEventError = '';
        this._renderSignature = null;
        this._busyContainers = new Set();
        this._indicator = new PanelMenu.Button(0.0, this.metadata.name, false);
        this._lastCount = -1;
        this._searchQuery = '';
        this._viewMode = this._settings.get_string('view-mode');
        this._devIncludes = this._settings.get_strv('dev-includes');
        this._devExcludes = this._settings.get_strv('dev-excludes');
        this._protoFilter = this._settings.get_string('protocol-filter');
        this._notifyEnabled = this._settings.get_boolean('notify-enabled');
        if (this._viewMode === 'DEV' && !['ALL', 'TCP'].includes(this._protoFilter))
            this._protoFilter = 'ALL';
        this._knownSockets = new Map();
        this._dockerBindings = [];
        this._dockerCacheTime = 0;
        this._protoButtons = {};
        this._currentModeClass = null;

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
        this._syncThemeMode();

        try {
            this._interfaceSettings = new Gio.Settings({schema_id: 'org.gnome.desktop.interface'});
            this._themeChangedId = this._interfaceSettings.connect('changed', (_s, key) => {
                if (key === 'color-scheme' || key === 'gtk-theme') {
                    this._syncThemeMode();
                }
            });
        } catch (_e) {}

        this._indicator.menu.connect('open-state-changed', (_menu, isOpen) => {
            if (isOpen) {
                this._syncThemeMode();
                this._refreshResults();
                this._scanSockets(this._session);
                if (this._searchEntry) {
                    after(this._session, 0, () => {
                        if (this._searchEntry && this._indicator?.menu?.isOpen) {
                            global.stage.set_key_focus(this._searchEntry.clutter_text);
                        }
                        return GLib.SOURCE_REMOVE;
                    });
                }
            }
        });

        Main.panel.addToStatusArea(this.uuid, this._indicator, 1, 'left');

        this._settingsChangedId = this._settings.connect('changed', (_settings, key) => {
            this._notifyEnabled = this._settings.get_boolean('notify-enabled');
            this._viewMode = this._settings.get_string('view-mode');
            this._devIncludes = this._settings.get_strv('dev-includes');
            this._devExcludes = this._settings.get_strv('dev-excludes');
            this._protoFilter = this._settings.get_string('protocol-filter');
            if (this._viewMode === 'DEV' && !['ALL', 'TCP'].includes(this._protoFilter)) this._protoFilter = 'ALL';
            this._updateNotifySwitchUI();
            this._updateProtoButtonStyles();
            if (key === 'view-mode') {
                this._initialized = false;
                this._scanSockets(this._session);
            }
            this._updateBadge();
            this._syncKnownSockets(this._items, false);
            this._refreshResults();
        });
        this._refreshResults();
        this._startEventMonitors(this._session);
        this._scanSockets(this._session);
    }

    _isSystemDarkMode() {
        try {
            const settings = this._interfaceSettings || new Gio.Settings({schema_id: 'org.gnome.desktop.interface'});
            // GNOME's default preference is light; only an explicit dark preference opts in.
            return settings.get_string('color-scheme') === 'prefer-dark';
        } catch (_e) {
            return false;
        }
    }

    _syncThemeMode() {
        const isDark = this._isSystemDarkMode();
        const nextClass = isDark ? 'pk-dark' : 'pk-light';
        const prevClass = this._currentModeClass;
        this._currentModeClass = nextClass;

        if (this._body) {
            if (prevClass) this._body.remove_style_class_name(prevClass);
            this._body.add_style_class_name('pk-menu-body');
            this._body.add_style_class_name(nextClass);
        }

        const menuBox = this._indicator?.menu.box;
        if (menuBox) {
            if (prevClass) menuBox.remove_style_class_name(prevClass);
            menuBox.add_style_class_name('pk-menu-surface');
            menuBox.add_style_class_name(nextClass);
        }

        this._updateNotifySwitchUI();
    }

    _buildNotifySwitch() {
        const btn = new St.Button({
            style_class: 'pk-switch-btn',
            can_focus: true,
            y_align: Clutter.ActorAlign.CENTER,
        });

        btn.accessible_name = 'Toggle port notifications';
        this._notifySwitchText = new St.Label({
            text: '',
            style_class: 'pk-switch-label',
            y_align: Clutter.ActorAlign.CENTER,
        });
        btn.set_child(this._notifySwitchText);

        btn.connect('clicked', () => {
            this._notifyEnabled = !this._notifyEnabled;
            this._settings.set_boolean('notify-enabled', this._notifyEnabled);
            this._updateNotifySwitchUI();
            if (this._notifyEnabled) {
                const current = this._items;
                this._syncKnownSockets(current, false);
            }
        });

        this._notifySwitchBtn = btn;
        this._updateNotifySwitchUI();
        return btn;
    }

    _updateNotifySwitchUI() {
        if (!this._notifySwitchText) return;
        this._notifySwitchText.set_text(this._notifyEnabled ? 'Notify: ON' : 'Notify: OFF');
        this._notifySwitchBtn?.set_checked(this._notifyEnabled);
    }

    _buildMenuSkeleton() {
        this._indicator.menu.removeAll();

        const menuItem = new PopupMenu.PopupBaseMenuItem({reactive: false, can_focus: false});
        menuItem.add_style_class_name('pk-menu-item');
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
            text: 'Active Sockets',
            style_class: 'pk-header-title',
            y_align: Clutter.ActorAlign.CENTER,
            x_expand: true,
        });
        const titleBox = new St.BoxLayout({style_class: 'pk-title-box', x_expand: true});
        this._titleLabel.x_expand = false;
        this._countLabel = new St.Label({
            text: '(0/0)',
            style_class: 'pk-count-label',
            y_align: Clutter.ActorAlign.CENTER,
        });
        titleBox.add_child(this._titleLabel);
        titleBox.add_child(this._countLabel);
        headerBar.add_child(titleBox);
        headerBar.add_child(this._buildNotifySwitch());

        this._headerActionsBox = new St.BoxLayout({
            style_class: 'pk-header-actions',
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
        this._searchEntry.set_primary_icon(new St.Icon({icon_name: 'edit-find-symbolic', style_class: 'pk-search-icon'}));
        this._searchEntry.clutter_text.connect('text-changed', () => {
            this._searchQuery = this._searchEntry.get_text();
            this._refreshResults();
        });
        filterRow.add_child(this._searchEntry);

        this._viewButton = new St.Button({label: this._viewMode, style_class: 'pk-proto-btn pk-pill-accent',
            can_focus: true, accessible_name: 'Toggle developer and all sockets view'});
        this._viewButton.connect('clicked', () => {
            this._settings.set_string('view-mode', this._viewMode === 'DEV' ? 'ALL' : 'DEV');
        });
        filterRow.add_child(this._viewButton);
        for (const proto of PROTOS) {
            const pBtn = new St.Button({
                label: proto,
                style_class: 'pk-proto-btn',
                can_focus: true,
            });
            pBtn.connect('clicked', () => {
                this._protoFilter = proto;
                this._settings.set_string('protocol-filter', proto);
                this._updateProtoButtonStyles();
                this._refreshResults();
            });
            this._protoButtons[proto] = pBtn;
            filterRow.add_child(pBtn);
        }
        const refreshBtn = new St.Button({label: '↻', style_class: 'pk-proto-btn', can_focus: true,
            accessible_name: 'Refresh sockets and reconnect event monitors'});
        refreshBtn.connect('clicked', () => {
            this._dockerCacheTime = 0;
            this._startEventMonitors(this._session);
            this._scanSockets(this._session);
        });
        filterRow.add_child(refreshBtn);
        this._updateProtoButtonStyles();
        this._body.add_child(filterRow);

        this._statusLabel = new St.Label({style_class: 'pk-status-label', visible: false});
        this._statusLabel.clutter_text.line_wrap = true;
        this._body.add_child(this._statusLabel);

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

    _updateProtoButtonStyles() {
        this._viewButton?.set_label(this._viewMode);
        for (const proto of PROTOS) {
            const btn = this._protoButtons[proto];
            if (!btn) continue;
            btn.visible = this._viewMode !== 'DEV' || ['ALL', 'TCP'].includes(proto);
            btn.style_class = this._protoFilter === proto
                ? 'pk-proto-btn pk-pill-accent'
                : 'pk-proto-btn';
        }
    }

    async _loadDockerBindings(session) {
        const now = GLib.get_monotonic_time();
        if (now - this._dockerCacheTime < 5000000) return;
        this._dockerCacheTime = now;
        try {
            const output = await runCommand(session, ['docker', 'ps', '--no-trunc', '--format', '{{json .}}']);
            if (!session.active) return;
            this._dockerBindings = parseDockerBindings(output);
            this._dockerError = '';
        } catch (error) {
            if (!session.active) return;
            this._dockerBindings = [];
            this._dockerError = `Docker unavailable: ${error.message}`;
        }
    }

    _scheduleRefresh(session, protocols = this._viewMode === 'DEV' ? ['TCP'] : ['TCP', 'UDP', 'UNIX']) {
        if (!session.active) return;
        session.pendingProtocols ??= new Set();
        for (const proto of protocols) {
            if (this._viewMode !== 'DEV' || proto === 'TCP') session.pendingProtocols.add(proto);
        }
        if (!session.pendingProtocols.size) return;
        if (session.refreshTimer) return;
        // One-shot debounce, scheduled only by an actual event. No idle scanning.
        session.refreshTimer = after(session, 180, () => {
            session.refreshTimer = null;
            this._scanSockets(session, []);
        });
    }

    _startEventMonitors(session) {
        if (!this._eventMonitor) {
            try {
                this._eventMonitor = watchCommand(session,
                    ['/usr/local/libexec/port-killer-events', '--watch'], line => {
                        const previousError = this._eventError;
                        if (line === 'ready') {
                            this._eventError = '';
                            this._scheduleRefresh(session);
                        } else if (/^change (TCP|UDP|UNIX)$/.test(line)) {
                            this._scheduleRefresh(session, [line.slice(7)]);
                        } else if (line === 'change' || line === 'resync') {
                            this._scheduleRefresh(session);
                        } else if (line === 'waiting') {
                            this._eventError = 'Waiting for kernel event probes…';
                        }
                        if (this._eventError !== previousError && this._indicator.menu.isOpen)
                            this._refreshResults();
                    }, () => {
                        this._eventMonitor = null;
                        this._eventError = 'Live events unavailable. Start port-killer-events.service and press ↻ to reconnect.';
                        if (this._indicator.menu.isOpen) this._refreshResults();
                    });
            } catch (_error) {
                this._eventError = 'Live events unavailable. Install the Rust helper; ↻ refreshes manually.';
            }
        }
        if (!this._dockerMonitor) {
            try {
                this._dockerMonitor = watchCommand(session,
                    ['docker', 'events', '--filter', 'type=container', '--format', '{{json .}}'], () => {
                        this._dockerCacheTime = 0;
                        this._scheduleRefresh(session);
                    }, () => {
                        this._dockerMonitor = null;
                        this._dockerEventError = 'Docker events disconnected. Press ↻ to reconnect.';
                        if (this._indicator.menu.isOpen) this._refreshResults();
                    });
                this._dockerEventError = '';
                // Reconnects always resync, including containers changed while disconnected.
                this._dockerCacheTime = 0;
            } catch (_error) { /* Docker support is optional. */ }
        }
    }

    async _scanSockets(session, protocols = this._viewMode === 'DEV' ? ['TCP'] : ['TCP', 'UDP', 'UNIX']) {
        if (!session.active) return;
        session.pendingProtocols ??= new Set();
        for (const proto of protocols) session.pendingProtocols.add(proto);
        if (session.refreshing) { session.refreshAgain = true; return; }
        if (session.refreshTimer && session.sources.delete(session.refreshTimer))
            GLib.Source.remove(session.refreshTimer);
        session.refreshTimer = null;
        session.refreshing = true;
        try {
            const requested = new Set(session.pendingProtocols);
            session.pendingProtocols.clear();
            const flags = {TCP: '-tlpnH', UDP: '-ulpnH', UNIX: '-xlpnH'};
            const dockerUpdate = [...requested].some(proto => proto !== 'UNIX')
                ? this._loadDockerBindings(session) : Promise.resolve();
            const scans = await Promise.allSettled([...requested].map(async proto =>
                [proto, await runCommand(session, ['ss', flags[proto]])]));
            await dockerUpdate;
            if (!session.active) return;
            const failed = scans.find(result => result.status === 'rejected');
            if (failed) throw failed.reason;
            const updated = scans.flatMap(result => {
                const [proto, output] = result.value;
                return proto === 'UNIX' ? parseUnix(output) : parseInet(output, proto, this._dockerBindings);
            });
            let items = uniqueSockets([...this._items.filter(item => !requested.has(item.proto)), ...updated]);
            const details = new Map();
            if (this._indicator.menu.isOpen) {
                const uptime = Number((await readFile(session, '/proc/uptime')).split(' ')[0]);
                await Promise.all([...new Set(this._viewItems(items).map(item => item.pid))].map(async pid => {
                    details.set(pid, await this._getProcDetails(session, pid, uptime));
                }));
            }
            if (!session.active) return;
            items = items.map(item => ({cmd: '', ppid: '?', rssMb: '?', uptime: '?', ...item, ...details.get(item.pid)}));
            this._syncKnownSockets(items, this._initialized);
            this._initialized = true;
            this._items = items;
            this._snapshotError = '';
            this._updateBadge();
            if (this._indicator.menu.isOpen) this._refreshResults();
        } catch (error) {
            if (!session.active) return;
            // Keep the last good snapshot; a failed scan is not a socket-close event.
            this._snapshotError = `Refresh failed; showing last results: ${error.message}`;
            if (this._indicator.menu.isOpen) this._refreshResults();
        } finally {
            session.refreshing = false;
            if (session.active && session.refreshAgain) {
                session.refreshAgain = false;
                this._scheduleRefresh(session, []);
            }
        }
    }

    _syncKnownSockets(items, emitNotifications = true) {
        const nextMap = new Map();
        for (const item of items) {
            if (item.proto === 'UNIX' || !this._viewItems([item]).length) continue;
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

    async _getProcDetails(session, pid, uptimeSeconds) {
        const [cmd, status, stat] = await Promise.all([
            readFile(session, `/proc/${pid}/cmdline`).catch(() => ''),
            readFile(session, `/proc/${pid}/status`).catch(() => ''),
            readFile(session, `/proc/${pid}/stat`).catch(() => ''),
        ]);
        const command = cmd.replace(/\0+/g, ' ').trim();
        const rss = status.match(/VmRSS:\s+(\d+)/);
        // /proc stat field 22 is starttime; the comm field can contain spaces or parentheses.
        const fields = stat.slice(stat.lastIndexOf(')') + 2).split(' ');
        const startTicks = fields[19] || '';
        // Linux exposes proc clock ticks at USER_HZ (100 on supported GNOME Linux targets).
        const seconds = startTicks ? Math.max(0, Math.floor(uptimeSeconds - Number(startTicks) / 100)) : null;
        const uptime = seconds === null ? '?' : seconds < 60 ? `${seconds}s`
            : seconds < 3600 ? `${Math.floor(seconds / 60)}m`
                : `${Math.floor(seconds / 3600)}h ${Math.floor(seconds % 3600 / 60)}m`;
        return {cmd: command.length > 100 ? `${command.slice(0, 97)}...` : command,
            ppid: status.match(/PPid:\s+(\d+)/)?.[1] || '?',
            rssMb: rss ? `${Math.round(Number(rss[1]) / 1024)} MB` : '?', uptime, startTicks};
    }

    _confirm(title, description, actionLabel, action) {
        if (this._confirmDialog) return;
        this._indicator.menu.close();
        const dialog = new ModalDialog.ModalDialog({destroyOnClose: true});
        this._confirmDialog = dialog;
        dialog.contentLayout.add_child(new Dialog.MessageDialogContent({title, description}));
        dialog.setButtons([
            {label: 'Cancel', key: Clutter.KEY_Escape, default: true, action: () => dialog.close()},
            {label: actionLabel, action: () => { dialog.close(); action(); }},
        ]);
        dialog.connect('destroy', () => {
            if (this._confirmDialog === dialog) this._confirmDialog = null;
        });
        if (!dialog.open()) { dialog.destroy(); }
    }

    _stopContainer(item) {
        if (!/^[a-f0-9]{64}$/.test(item.containerId) || this._busyContainers.has(item.containerId)) return;
        this._confirm(`Stop ${item.containerName}?`,
            'This stops the entire Docker container and all services running inside it.',
            'Stop Container', () => this._executeStop(item));
    }

    async _executeStop(item) {
        const session = this._session;
        this._busyContainers.add(item.containerId);
        this._refreshResults();
        try {
            await runCommand(session, ['docker', 'stop', '--time', '10', item.containerId], 15000);
            if (session.active) Main.notify('Container stopped', item.containerName);
        } catch (error) {
            if (session.active) Main.notify('Could not confirm container stop',
                `${item.containerName}: ${error.message}. Check Docker before trying again.`);
        } finally {
            if (session.active) {
                this._busyContainers.delete(item.containerId);
                this._dockerCacheTime = 0;
                this._refreshResults();
                this._scanSockets(session);
            }
        }
    }

    async _killItems(items, force = false) {
        const session = this._session;
        try {
            const unique = [...new Map(items.filter(item => !isDockerProxy(item.name))
                .map(item => [item.pid, item])).values()];
            for (const item of unique) {
                // Do not send a signal to a PID that has been reused since the snapshot.
                const stat = await readFile(session, `/proc/${item.pid}/stat`);
                const startTicks = stat.slice(stat.lastIndexOf(')') + 2).split(' ')[19];
                if (!item.startTicks || startTicks !== item.startTicks)
                    throw new Error(`Process ${item.pid} changed; refresh before trying again`);
                await runCommand(session, ['kill', force ? '-9' : '-15', item.pid]);
            }
        } catch (error) {
            if (session.active) Main.notify('Could not stop process', error.message);
        } finally {
            if (session.active) this._scanSockets(session);
        }
    }

    _bulkKill(items, force) {
        this._confirm(force ? 'Force kill filtered processes?' : 'Stop filtered processes?',
            'All ports belonging to these processes will close. Docker proxy processes are excluded.',
            force ? 'Force All' : 'Kill All', () => this._killItems(items, force));
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

        procTitle.clutter_text.ellipsize = Pango.EllipsizeMode.END;
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
        procSub.clutter_text.ellipsize = Pango.EllipsizeMode.END;
        tile.add_child(procSub);

        // Row 3: Command line
        if (item.cmd) {
            const cmdLabel = new St.Label({
                text: item.cmd,
                style_class: 'pk-cmd-box',
                x_expand: true,
            });
            cmdLabel.clutter_text.ellipsize = Pango.EllipsizeMode.END;
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
                style_class: 'pk-mini-btn',
                can_focus: true,
            });
            openBtn.connect('clicked', () => {
                const session = this._session;
                runCommand(session, ['gio', 'open', item.url]).catch(error => {
                    if (session.active) Main.notify('Could not open URL', error.message);
                });
            });
            actionsRow.add_child(openBtn);

            const copyUrlBtn = new St.Button({
                label: 'Copy URL',
                style_class: 'pk-mini-btn',
                can_focus: true,
            });
            copyUrlBtn.connect('clicked', () => {
                St.Clipboard.get_default().set_text(St.ClipboardType.CLIPBOARD, item.url);
                copyUrlBtn.set_label('Copied');
                let alive = true;
                copyUrlBtn.connect('destroy', () => { alive = false; });
                after(this._session, 900, () => {
                    if (alive) copyUrlBtn.set_label('Copy URL');
                    return GLib.SOURCE_REMOVE;
                });
            });
            actionsRow.add_child(copyUrlBtn);
        }

        const copyPidBtn = new St.Button({
            label: 'Copy PID',
            style_class: 'pk-mini-btn',
            can_focus: true,
        });
        copyPidBtn.connect('clicked', () => {
            St.Clipboard.get_default().set_text(St.ClipboardType.CLIPBOARD, `${item.pid}`);
            copyPidBtn.set_label('Copied');
            let alive = true;
            copyPidBtn.connect('destroy', () => { alive = false; });
            after(this._session, 900, () => {
                if (alive) copyPidBtn.set_label('Copy PID');
                return GLib.SOURCE_REMOVE;
            });
        });
        actionsRow.add_child(copyPidBtn);

        if (item.proto === 'TCP') {
            const devVisible = isDeveloperSocket(item, this._devIncludes, this._devExcludes);
            const devBtn = new St.Button({label: devVisible ? 'Hide from DEV' : 'Show in DEV',
                style_class: 'pk-mini-btn', can_focus: true});
            devBtn.connect('clicked', () => this._toggleDeveloper(item));
            actionsRow.add_child(devBtn);
        }

        const spacer = new St.Widget({x_expand: true});
        actionsRow.add_child(spacer);

        if (isDockerProxy(item.name)) {
            const stopBtn = new St.Button({
                label: this._busyContainers.has(item.containerId) ? 'Stopping…' : 'Stop Container',
                style_class: 'pk-mini-btn pk-mini-kill', can_focus: true,
                reactive: Boolean(item.containerId) && !this._busyContainers.has(item.containerId),
            });
            stopBtn.accessible_name = item.containerId ? `Stop container ${item.containerName}`
                : 'Docker container could not be identified';
            stopBtn.connect('clicked', () => this._stopContainer(item));
            actionsRow.add_child(stopBtn);
        } else {
            for (const force of [false, true]) {
                const btn = new St.Button({label: force ? 'Force Kill' : 'Kill',
                    style_class: `pk-mini-btn ${force ? 'pk-mini-force' : 'pk-mini-kill'}`, can_focus: true});
                btn.connect('clicked', () => this._killItems([item], force));
                actionsRow.add_child(btn);
            }
        }
        tile.add_child(actionsRow);

        return tile;
    }

    _viewItems(items = this._items) {
        return viewSockets(items, this._viewMode, this._devIncludes, this._devExcludes);
    }

    _updateBadge() {
        this._label.set_text(`${this._viewItems().length}`);
    }

    _toggleDeveloper(item) {
        const key = developerKey(item);
        const visible = isDeveloperSocket(item, this._devIncludes, this._devExcludes);
        const includes = this._devIncludes.filter(value => value !== key);
        const excludes = this._devExcludes.filter(value => value !== key);
        (visible ? excludes : includes).push(key);
        this._settings.set_strv('dev-includes', includes);
        this._settings.set_strv('dev-excludes', excludes);
    }

    _refreshResults() {
        try {
            const allItems = this._viewItems();
            const filteredItems = filterSockets(allItems, this._protoFilter, this._searchQuery);
            const status = this._snapshotError || this._eventError || (allItems.some(item => isDockerProxy(item.name)) ? this._dockerError || this._dockerEventError : '');
            this._statusLabel.set_text(status);
            this._statusLabel.visible = Boolean(status);
            const signature = JSON.stringify([filteredItems, allItems.length, this._viewMode, this._devIncludes, this._devExcludes, status, this._initialized, [...this._busyContainers]]);
            if (signature === this._renderSignature) return;
            this._renderSignature = signature;

            this._lastCount = allItems.length;
            this._label.set_text(`${allItems.length}`);

            if (this._titleLabel) {
                this._titleLabel.set_text(this._viewMode === 'DEV' ? 'Dev Sockets' : 'Active Sockets');
                this._countLabel.set_text(`(${filteredItems.length}/${allItems.length})`);
            }

            if (this._headerActionsBox) {
                this._headerActionsBox.destroy_all_children();
                if (filteredItems.some(item => !isDockerProxy(item.name))) {
                    const killAllBtn = new St.Button({
                        label: 'Kill All',
                        style_class: 'pk-pill-btn pk-mini-kill',
                        can_focus: true,
                    });
                    killAllBtn.connect('clicked', () => this._bulkKill(filteredItems, false));

                    const forceAllBtn = new St.Button({
                        label: 'Force All',
                        style_class: 'pk-pill-btn pk-mini-force',
                        can_focus: true,
                    });
                    forceAllBtn.connect('clicked', () => this._bulkKill(filteredItems, true));

                    this._headerActionsBox.add_child(killAllBtn);
                    this._headerActionsBox.add_child(forceAllBtn);
                }
            }

            if (!this._listBox) return;
            this._listBox.destroy_all_children();

            if (filteredItems.length === 0) {
                const emptyLabel = new St.Label({
                    text: this._initialized ? 'No active sockets match the current filter.' : 'Loading sockets…',
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
        if (this._themeChangedId && this._interfaceSettings) {
            try {
                this._interfaceSettings.disconnect(this._themeChangedId);
            } catch (_e) {}
            this._themeChangedId = null;
        }
        this._interfaceSettings = null;

        cancelSession(this._session);
        this._eventMonitor = null;
        this._dockerMonitor = null;
        this._confirmDialog?.destroy();
        this._confirmDialog = null;
        if (this._settingsChangedId) this._settings.disconnect(this._settingsChangedId);
        this._settingsChangedId = null;
        this._settings = null;
        if (this._indicator) {
            this._indicator.destroy();
            this._indicator = null;
        }
        this._searchEntry = null;
        this._listBox = null;
        this._titleLabel = null;
        this._countLabel = null;
        this._headerActionsBox = null;
        this._notifySwitchBtn = null;
        this._notifySwitchText = null;
        this._knownSockets.clear();
        this._dockerBindings = [];
        this._items = [];
        this._statusLabel = null;
    }
}
