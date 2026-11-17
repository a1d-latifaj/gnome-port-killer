import test from 'node:test';
import assert from 'node:assert/strict';
import {parseDockerBindings, resolveContainer, parseInet, parseUnix, uniqueSockets, filterSockets} from '../extension/sockets.js';
const a = 'a'.repeat(64), b = 'b'.repeat(64);
const docker = (ID, Names, Ports) => JSON.stringify({ID, Names, Ports});

test('Docker identity includes protocol, address and full immutable ID', () => {
    const bindings = parseDockerBindings([
        docker(a, 'web', '0.0.0.0:8080->80/tcp, [::]:8080->80/tcp, 80/tcp'),
        docker(b, 'dns', '127.0.0.1:8080->53/udp, 127.0.0.2:8080->80/tcp'),
    ].join('\n'));
    assert.equal(resolveContainer(bindings, 'TCP', 8080, '0.0.0.0').id, a);
    assert.equal(resolveContainer(bindings, 'TCP', 8080, '::').id, a);
    assert.equal(resolveContainer(bindings, 'UDP', 8080, '127.0.0.1').id, b);
    assert.equal(resolveContainer(bindings, 'TCP', 8080, '127.0.0.2').id, b);
    assert.equal(resolveContainer(bindings, 'UDP', 8080, '::'), null);
    assert.equal(resolveContainer([...bindings, {...bindings[0], id: b}], 'TCP', 8080, '0.0.0.0'), null);
});

test('TCP handles IPv6, multiple owners and Docker mapping without mapping ordinary processes', () => {
    const bindings = parseDockerBindings(docker(a, 'backend', '[::]:8000->80/tcp'));
    const rows = parseInet('LISTEN 0 4096 [::]:8000 [::]:* users:(("docker-proxy",pid=42,fd=1),("node",pid=43,fd=1))', 'TCP', bindings);
    assert.equal(rows.length, 2);
    assert.equal(rows[0].containerId, a);
    assert.equal(rows[1].containerId, '');
    assert.equal(rows[0].bindAddr, '::');
    assert.equal(parseInet('LISTEN 0 10 0.0.0.0:8000 0.0.0.0:*', 'TCP').length, 0);
});

test('UDP has no HTTP action and UNIX uses the local path rather than peer path', () => {
    const [udp] = parseInet('UNCONN 0 0 127.0.0.1:5353 0.0.0.0:* users:(("dns",pid=5,fd=1))', 'UDP');
    assert.equal(udp.url, null);
    const [unix] = parseUnix('u_str LISTEN 0 128 /tmp/local.sock 123 * 0 users:(("app",pid=8,fd=2))');
    assert.equal(unix.bindAddr, '/tmp/local.sock');
    assert.equal(parseUnix('u_str LISTEN 0 128 @abstract 123 * 0 users:(("app",pid=8,fd=2))')[0].bindAddr, '@abstract');
});

test('deduplication retains distinct binds and filters search cached metadata', () => {
    const [item] = parseInet('LISTEN 0 10 127.0.0.1:3000 0.0.0.0:* users:(("node",pid=7,fd=1))', 'TCP');
    assert.equal(uniqueSockets([item, item, {...item, bindAddr: '::1'}]).length, 2);
    assert.equal(filterSockets([{...item, cmd: 'npm run development'}], 'TCP', 'DEVELOPMENT').length, 1);
    assert.equal(filterSockets([item], 'UDP', '').length, 0);
});

test('developer view hides desktop and non-TCP sockets, retains runtimes and containers', async () => {
    const {isDeveloperSocket, developerKey, viewSockets} = await import('../extension/sockets.js');
    const base = {proto: 'TCP', portNum: 8000, name: 'my-rust-api'};
    assert.equal(isDeveloperSocket(base), true);
    assert.equal(isDeveloperSocket({...base, name: 'gnome-shell'}), false);
    assert.equal(isDeveloperSocket({...base, proto: 'UNIX'}), false);
    assert.equal(isDeveloperSocket({...base, proto: 'UDP'}), false);
    assert.equal(isDeveloperSocket({...base, portNum: 18000, name: 'uvicorn'}), true);
    const custom = {...base, name: 'custom-api', portNum: 18000};
    assert.equal(isDeveloperSocket(custom), false);
    assert.equal(isDeveloperSocket(custom, [developerKey(custom)]), true);
    assert.equal(isDeveloperSocket(base, [], [developerKey(base)]), false);
    assert.equal(viewSockets([base, {...base, proto: 'UNIX'}], 'ALL').length, 2);
});
