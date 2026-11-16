import GLib from 'gi://GLib';
import {createSession, cancelSession, after, runCommand, readFile, watchCommand} from '../extension/runtime.js';
const assert = (condition, message) => { if (!condition) throw new Error(message); };
const session = createSession();
const loop = new GLib.MainLoop(null, false);
let failed = false;
(async () => {
    try {
        assert(await runCommand(session, ['printf', '%s', 'hello']) === 'hello', 'stdout');
        try { await runCommand(session, ['sh', '-c', 'echo denied >&2; exit 3']); throw new Error('missing error'); }
        catch (error) { assert(error.message.includes('denied'), 'stderr on nonzero exit'); }
        const lines = [];
        await new Promise(resolve => {
            watchCommand(session, ['sh', '-c', 'printf "ready\\nchange\\n"; sleep 0.1'],
                line => lines.push(line), () => resolve());
        });
        assert(lines.join(',') === 'ready,change', 'stream delivers event lines');
        let heartbeat = false;
        after(session, 10, () => { heartbeat = true; });
        try { await runCommand(session, ['sleep', '5'], 50); throw new Error('missing timeout'); }
        catch (error) { assert(error.message.includes('timed out'), 'timeout'); }
        assert(heartbeat, 'main loop remained responsive');
        assert((await readFile(session, '/proc/uptime')).length > 0, 'async proc read');
        const pending = runCommand(session, ['sleep', '5']);
        after(session, 10, () => cancelSession(session));
        try { await pending; throw new Error('missing cancellation'); }
        catch (error) { assert(!session.active, 'disable cancels command'); }
        assert(session.sources.size === 0 && session.processes.size === 0, 'cleanup');
        print('PASS: asynchronous commands, errors, timeout, responsiveness, reads and disable cleanup');
    } catch (error) { printerr(error.stack); failed = true; }
    finally { if (session.active) cancelSession(session); loop.quit(); }
})();
loop.run();
if (failed) throw new Error('Runtime tests failed');
