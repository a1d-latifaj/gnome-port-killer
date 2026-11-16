import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import * as sockets from '../extension/sockets.js';

function harness(command, watch = () => ({})) {
    const timers = new Map();
    let nextTimer = 0;
    const source = fs.readFileSync(new URL('../extension/extension.js', import.meta.url), 'utf8')
        .replace(/^import .*;\n/gm, '').replace('export default class', 'return class');
    const bindings = {Extension: class {}, ...sockets,
        GLib: {get_monotonic_time: () => 10000000, Source: {remove: id => timers.delete(id)}},
        watchCommand: watch, runCommand: command, readFile: async () => '1000 100',
        after: (session, _ms, callback) => {
            const id = ++nextTimer;
            timers.set(id, callback); session.sources.add(id); return id;
        }, Main: {notify() {}},
    };
    const Class = new Function(...Object.keys(bindings), source)(...Object.values(bindings));
    const extension = new Class();
    Object.assign(extension, {_session: {active: true, sources: new Set()}, _dockerCacheTime: 0,
        _dockerBindings: [], _items: [], _knownSockets: new Map(), _notifyEnabled: false,
        _indicator: {menu: {isOpen: true}}, _label: {set_text() {}}, _busyContainers: new Set(),
        _getProcDetails: async () => ({cmd: 'test', startTicks: '100'}),
        _refreshResults() { this.renders = (this.renders || 0) + 1; }});
    return {extension, timers};
}
const tcp = pid => `LISTEN 0 10 127.0.0.1:3000 0.0.0.0:* users:(("node",pid=${pid},fd=1))`;

test('same-count replacements refresh and failed scans retain last successful snapshot', async () => {
    let pid = '1', fail = false;
    const {extension: e, timers} = harness(async (_session, argv) => {
        if (argv[0] === 'docker') return '';
        if (fail) throw new Error('ss failed');
        return argv[1] === '-tlpnH' ? tcp(pid) : '';
    });
    await e._scanSockets(e._session);
    assert.equal(e._items[0].pid, '1');
    pid = '2';
    await e._scanSockets(e._session);
    assert.equal(e._items[0].pid, '2');
    assert.equal(e.renders, 2);
    assert.equal(timers.size, 0, 'no timer is scheduled after a successful scan');
    fail = true;
    await e._scanSockets(e._session);
    assert.equal(e._items[0].pid, '2');
    assert.match(e._snapshotError, /ss failed/);
    assert.equal(e._knownSockets.size, 1);
});

test('overlapping requests coalesce and disabled sessions cannot publish results', async () => {
    let release;
    const waiting = new Promise(resolve => { release = resolve; });
    const {extension: e, timers} = harness(async () => { await waiting; return ''; });
    const first = e._scanSockets(e._session);
    await e._scanSockets(e._session);
    assert.equal(timers.size, 0);
    assert.equal(e._session.refreshAgain, true, 'event during scan queues a follow-up');
    e._session.active = false;
    release(); await first;
    assert.equal(e.renders, undefined);
    assert.equal(timers.size, 0);
});

test('Docker stop requires confirmation and sends the full ID as a separate argument', async () => {
    const commands = [];
    const {extension: e} = harness(async (_session, argv) => { commands.push(argv); return ''; });
    e._scanSockets = () => {};
    let confirm;
    e._confirm = (...args) => { confirm = args; };
    const item = {containerId: 'a'.repeat(64), containerName: 'backend'};
    e._stopContainer(item);
    assert.equal(commands.length, 0, 'no mutation before confirmation');
    assert.match(confirm[1], /entire Docker container/);
    await confirm[3]();
    assert.deepEqual(commands, [['docker', 'stop', '--time', '10', item.containerId]]);
    assert.equal(e._busyContainers.size, 0);
    confirm = null;
    e._stopContainer({...item, containerId: 'backend'});
    assert.equal(confirm, null, 'mutable names are never accepted as stop targets');
});


test('event bursts coalesce into one scan with no recurring timer', async () => {
    const watches = [];
    const {extension: e, timers} = harness(async () => '', (_session, argv, onLine, onExit) => {
        watches.push({argv, onLine, onExit}); return {};
    });
    e._startEventMonitors(e._session);
    assert.equal(watches.length, 2);
    assert.equal(timers.size, 0, 'subscribing does not start polling');
    watches[0].onLine('ready');
    const rendersBeforeEvents = e.renders;
    for (let i = 0; i < 20; i++) watches[0].onLine('change');
    assert.equal(e.renders, rendersBeforeEvents, 'raw events must not rerender stale cards');
    assert.equal(timers.size, 1);
    const [id, callback] = [...timers][0];
    timers.delete(id); e._session.sources.delete(id); callback();
    await new Promise(resolve => setImmediate(resolve));
    assert.equal(timers.size, 0, 'completed event scan leaves no refresh timer');
    watches[0].onExit(new Error('disconnected'));
    assert.match(e._eventError, /unavailable/);
    assert.equal(timers.size, 0, 'disconnect does not activate hidden polling');
});

test('an event received during a scan schedules one follow-up', async () => {
    let release;
    const gate = new Promise(resolve => { release = resolve; });
    const {extension: e, timers} = harness(async () => { await gate; return ''; });
    const scan = e._scanSockets(e._session);
    await e._scanSockets(e._session);
    release(); await scan;
    assert.equal(timers.size, 1);
    assert.equal(e._session.refreshAgain, false);
});

test('protocol-specific events scan only that protocol and preserve other snapshots', async () => {
    const commands = [];
    const {extension: e} = harness(async (_session, argv) => {
        commands.push(argv);
        return argv[1] === '-ulpnH' ? 'UNCONN 0 0 127.0.0.1:5353 0.0.0.0:* users:(("dns",pid=8,fd=1))' : '';
    });
    e._items = [{proto: 'TCP', portNum: 3000, pid: '1', name: 'node', endpoint: ':3000', bindAddr: '127.0.0.1'}];
    await e._scanSockets(e._session, ['UDP']);
    assert.deepEqual(commands.filter(argv => argv[0] === 'ss').map(argv => argv[1]), ['-ulpnH']);
    assert.equal(e._items.some(item => item.proto === 'TCP'), true);
    assert.equal(e._items.some(item => item.proto === 'UDP'), true);
});

test('DEV ignores background UDP/UNIX events and closed menu skips proc details', async () => {
    const commands = [];
    const {extension: e, timers} = harness(async (_session, argv) => {
        commands.push(argv); return argv[1] === '-tlpnH' ? tcp('1') : '';
    });
    e._viewMode = 'DEV';
    e._indicator.menu.isOpen = false;
    let detailReads = 0;
    e._getProcDetails = async () => { detailReads++; return {}; };
    e._scheduleRefresh(e._session, ['UNIX']);
    e._scheduleRefresh(e._session, ['UDP']);
    assert.equal(timers.size, 0);
    await e._scanSockets(e._session);
    assert.equal(detailReads, 0);
    assert.deepEqual(commands.filter(argv => argv[0] === 'ss').map(argv => argv[1]), ['-tlpnH']);
});
