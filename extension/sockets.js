// Pure parsing and filtering, shared by the extension and regression tests.
export const isDockerProxy = name => ['docker-proxy', 'rootlessport'].includes(name);
const normalizeAddress = address => address.replace(/^\[|\]$/g, '');

export function parseDockerBindings(output) {
    const bindings = [];
    for (const line of output.split('\n').filter(line => line.trim())) {
        const container = JSON.parse(line);
        if (!/^[a-f0-9]{64}$/.test(container.ID) || !container.Names) continue;
        for (const port of (container.Ports || '').split(/,\s*/)) {
            const match = port.match(/^(.*):(\d+)->\d+\/(tcp|udp)$/);
            if (!match) continue; // Exposed ports without a host binding are not listeners.
            bindings.push({id: container.ID, name: container.Names,
                address: normalizeAddress(match[1]), port: Number(match[2]),
                proto: match[3].toUpperCase()});
        }
    }
    return bindings;
}

export function resolveContainer(bindings, proto, port, address) {
    const candidates = bindings.filter(binding => binding.proto === proto && binding.port === port);
    const exact = candidates.filter(binding => binding.address === address);
    const matches = exact.length ? exact : candidates.filter(binding =>
        (binding.address === '0.0.0.0' && !address.includes(':')) ||
        (binding.address === '::' && address.includes(':')));
    const ids = new Set(matches.map(binding => binding.id));
    return ids.size === 1 ? matches[0] : null;
}

export function parseInet(output, proto, bindings = []) {
    const results = [];
    for (const line of output.split('\n')) {
        const cols = line.trim().split(/\s+/);
        if (cols.length < 5 || (proto === 'TCP' && cols[0] !== 'LISTEN')) continue;
        const endpoint = cols[3].match(/^(.*):(\d+)$/);
        if (!endpoint) continue;
        const portNum = Number(endpoint[2]);
        if (portNum >= 32768) continue; // Preserve the existing low-port view.
        const bindAddr = normalizeAddress(endpoint[1]);
        for (const proc of line.matchAll(/\("([^"]+)",pid=(\d+)/g)) {
            const [, name, pid] = proc;
            const container = isDockerProxy(name)
                ? resolveContainer(bindings, proto, portNum, bindAddr) : null;
            results.push({proto, endpoint: `:${portNum}`, portNum,
                url: proto === 'TCP' ? `http://localhost:${portNum}` : null,
                bindAddr, name, displayName: container ? `docker (${container.name})` : name,
                containerName: container?.name || '', containerId: container?.id || '', pid});
        }
    }
    return results;
}

export function parseUnix(output) {
    const results = [];
    for (const line of output.split('\n')) {
        // ss -xlpnH: Netid State Recv-Q Send-Q Local-Path Inode Peer-Path Inode Process
        const cols = line.trim().split(/\s+/);
        const path = cols[4];
        if (!path || !/^[\/@]/.test(path)) continue;
        for (const proc of line.matchAll(/\("([^"]+)",pid=(\d+)/g)) {
            results.push({proto: 'UNIX', endpoint: path.length > 22 ? `...${path.slice(-19)}` : path,
                portNum: 999999, url: null, bindAddr: path, name: proc[1], displayName: proc[1],
                containerName: '', containerId: '', pid: proc[2]});
        }
    }
    return results;
}

export function uniqueSockets(items) {
    const seen = new Set();
    return items.filter(item => {
        const key = `${item.proto}:${item.bindAddr}:${item.endpoint}:${item.pid}`;
        if (seen.has(key)) return false;
        seen.add(key);
        return true;
    }).sort((a, b) => a.portNum - b.portNum);
}

export function filterSockets(items, proto, query) {
    const q = query.trim().toLowerCase();
    return items.filter(item => (proto === 'ALL' || item.proto === proto) &&
        (!q || [item.endpoint, item.displayName, item.name, item.containerName,
            item.pid, item.cmd, item.bindAddr].join(' ').toLowerCase().includes(q)));
}

// A conservative convenience view, with persisted per-service overrides.
export function developerKey(item) {
    return `${item.proto}:${item.portNum}:${item.name}`;
}
const DEV_NAMES = /^(node(js)?|python([23](\.\d+)?)?|uvicorn|gunicorn|hypercorn|deno|bun|java|ruby|puma|php([0-9.]+)?|php-fpm.*|dotnet|beam(\.smp)?|postgres.*|postmaster|mysqld|mariadbd|redis-server|mongod|memcached|nginx|httpd|apache2|caddy|vite|webpack.*|next-server.*)$/i;
const DESKTOP_NAMES = /^(gnome.*|systemd.*|pipewire.*|pulseaudio|wireplumber|cups.*|avahi.*|dbus.*|sshd|NetworkManager|chrome.*|chromium.*|firefox.*)$/i;
const DEV_PORTS = new Set([3000, 3001, 3002, 4000, 4173, 4200, 5000, 5001, 5173, 5174,
    5432, 5672, 6379, 8000, 8001, 8002, 8080, 8081, 8082, 8443, 9000, 9090, 9200, 11211, 27017]);
export function isDeveloperSocket(item, includes = [], excludes = []) {
    if (item.proto !== 'TCP') return false;
    const key = developerKey(item);
    if (excludes.includes(key)) return false;
    if (includes.includes(key)) return true;
    if (DESKTOP_NAMES.test(item.name)) return false;
    return Boolean(item.containerId) || isDockerProxy(item.name) || DEV_NAMES.test(item.name) || DEV_PORTS.has(item.portNum);
}
export function viewSockets(items, mode, includes = [], excludes = []) {
    return mode === 'DEV' ? items.filter(item => isDeveloperSocket(item, includes, excludes)) : items;
}
