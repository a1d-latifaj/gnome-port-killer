import Gio from 'gi://Gio';
import GLib from 'gi://GLib';

// Each enable owns a separate lifetime. Old callbacks never touch a new session.
export function createSession() {
    return {active: true, cancellable: new Gio.Cancellable(), processes: new Set(),
        sources: new Set(), refreshing: false};
}

export function after(session, milliseconds, callback) {
    const id = GLib.timeout_add(GLib.PRIORITY_DEFAULT, milliseconds, () => {
        session.sources.delete(id);
        if (session.active) callback();
        return GLib.SOURCE_REMOVE;
    });
    session.sources.add(id);
    return id;
}

export function cancelSession(session) {
    session.active = false;
    session.cancellable.cancel();
    for (const id of session.sources) GLib.Source.remove(id);
    session.sources.clear();
    for (const proc of session.processes) proc.force_exit();
    session.processes.clear();
}

export async function runCommand(session, argv, timeoutMs = 4000) {
    if (!session.active) throw new Error('Extension disabled');
    const proc = Gio.Subprocess.new(argv,
        Gio.SubprocessFlags.STDOUT_PIPE | Gio.SubprocessFlags.STDERR_PIPE);
    session.processes.add(proc);
    const cancellable = new Gio.Cancellable();
    const cancelId = session.cancellable.connect(() => cancellable.cancel());
    let timedOut = false;
    const timeoutId = after(session, timeoutMs, () => {
        timedOut = true;
        proc.force_exit();
        cancellable.cancel();
    });
    try {
        return await new Promise((resolve, reject) => {
            proc.communicate_utf8_async(null, cancellable, (source, result) => {
                try {
                    const [, stdout, stderr] = source.communicate_utf8_finish(result);
                    if (!source.get_successful())
                        throw new Error(stderr.trim().slice(0, 240) || `${argv[0]} exited unsuccessfully`);
                    resolve(stdout || '');
                } catch (error) {
                    reject(timedOut ? new Error(`${argv[0]} timed out`) : error);
                }
            });
        });
    } finally {
        session.cancellable.disconnect(cancelId);
        if (session.sources.delete(timeoutId)) GLib.Source.remove(timeoutId);
        session.processes.delete(proc);
        // Cancellation can finish the read before the child exits.
        if (cancellable.is_cancelled()) proc.force_exit();
    }
}

export function readFile(session, path) {
    return new Promise((resolve, reject) => {
        Gio.File.new_for_path(path).load_contents_async(session.cancellable, (file, result) => {
            try {
                const [, contents] = file.load_contents_finish(result);
                resolve(new TextDecoder().decode(contents));
            } catch (error) { reject(error); }
        });
    });
}

// Long-lived event streams block in the main loop's async read, not in a timer.
// Children are owned by the enable session and reaped even after cancellation.
export function watchCommand(session, argv, onLine, onExit) {
    const proc = Gio.Subprocess.new(argv, Gio.SubprocessFlags.STDOUT_PIPE);
    session.processes.add(proc);
    const stream = new Gio.DataInputStream({base_stream: proc.get_stdout_pipe()});
    let done = false;
    const read = () => {
        if (done || !session.active) return;
        stream.read_line_async(GLib.PRIORITY_DEFAULT, session.cancellable, (source, result) => {
            try {
                const [line] = source.read_line_finish_utf8(result);
                if (line === null || done || !session.active) return;
                onLine(line);
                read();
            } catch (error) {
                if (!done && session.active) proc.force_exit();
            }
        });
    };
    // Do not cancel wait(): it must reap the child after disable force-exits it.
    proc.wait_async(null, (source, result) => {
        let error = null;
        try {
            source.wait_finish(result);
            if (!source.get_successful()) error = new Error(`${argv[0]} exited unsuccessfully`);
        } catch (failure) { error = failure; }
        done = true;
        session.processes.delete(proc);
        try { stream.close(null); } catch (_error) { /* read cancellation may have closed it */ }
        if (session.active) onExit(error || new Error(`${argv[0]} event stream disconnected`));
    });
    read();
    return proc;
}
