// A read-only bridge: a privileged daemon owns the fixed BPF program; an
// unprivileged --watch client streams invalidations to GNOME. No RPC commands.
mod event;
mod event_batch;
mod native;
mod reconcile;
mod recovery_gate;
mod snapshot;
mod state;
use std::env;
use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::process;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

const SOCKET: &str = "/run/port-killer/events.sock";
const MAX_CLIENTS: usize = 32;

#[derive(Default)]
struct Broker {
    ready: bool,
    clients: Vec<UnixStream>,
}
impl Broker {
    fn subscribe(&mut self, mut client: UnixStream) -> io::Result<()> {
        client.set_write_timeout(Some(Duration::from_millis(50)))?;
        // A disconnected client must not occupy a slot forever while idle.
        // Reading is nonblocking and happens only on a new subscription.
        self.clients.retain_mut(|existing| {
            if existing.set_nonblocking(true).is_err() {
                return false;
            }
            let result = existing.read(&mut [0u8; 1]);
            let alive =
                matches!(result, Err(ref error) if error.kind() == io::ErrorKind::WouldBlock);
            existing.set_nonblocking(false).is_ok() && alive
        });
        if self.clients.len() >= MAX_CLIENTS {
            client.write_all(b"busy\n")?;
            return Ok(());
        }
        client.write_all(if self.ready { b"ready\n" } else { b"waiting\n" })?;
        self.clients.push(client);
        Ok(())
    }
    fn publish(&mut self, event: &str) {
        if !matches!(
            event,
            "ready" | "change" | "change TCP" | "change UDP" | "change UNIX" | "resync"
        ) {
            return;
        }
        if event == "ready" {
            self.ready = true;
        }
        let line = format!("{event}\n");
        self.clients
            .retain_mut(|client| client.write_all(line.as_bytes()).is_ok());
    }
}

fn prepare_listener(path: &Path) -> io::Result<UnixListener> {
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if !metadata.file_type().is_socket() {
            return Err(io::Error::other("Refusing to replace a non-socket path"));
        }
        if UnixStream::connect(path).is_ok() {
            return Err(io::Error::new(
                io::ErrorKind::AddrInUse,
                "Event daemon already running",
            ));
        }
        fs::remove_file(path)?;
    }
    let listener = UnixListener::bind(path)?;
    // Clients can only read generic invalidations. There is no command handler.
    fs::set_permissions(path, fs::Permissions::from_mode(0o666))?;
    Ok(listener)
}

fn serve() -> io::Result<()> {
    let listener = prepare_listener(Path::new(SOCKET))?;
    let broker = Arc::new(Mutex::new(Broker::default()));
    let accept_broker = broker.clone();
    thread::spawn(move || {
        for connection in listener.incoming() {
            match connection {
                Ok(client) => {
                    if let Err(error) = accept_broker.lock().unwrap().subscribe(client) {
                        eprintln!("Event subscription failed: {error}");
                    }
                }
                Err(error) => eprintln!("Event connection failed: {error}"),
            }
        }
    });
    native::run(false, |event| broker.lock().unwrap().publish(event))
}

fn watch(path: &Path, output: &mut impl Write) -> io::Result<()> {
    let stream = UnixStream::connect(path)?;
    for line in BufReader::new(stream).lines() {
        let line = line?;
        match line.as_str() {
            "ready" | "waiting" | "change" | "change TCP" | "change UDP" | "change UNIX"
            | "resync" => {
                writeln!(output, "{line}")?;
                output.flush()?;
            }
            "busy" => return Err(io::Error::other("Event service has too many clients")),
            _ => return Err(io::Error::other("Unsupported event service protocol")),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::UnexpectedEof,
        "Event service disconnected",
    ))
}

fn main() {
    let args: Vec<_> = env::args().skip(1).collect();
    let result = match args.as_slice() {
        [arg] if arg == "--serve" => serve(),
        [arg] if arg == "--watch" => watch(Path::new(SOCKET), &mut io::stdout().lock()),
        [arg] if arg == "--check" => {
            native::run(true, |_| println!("Native probes attached successfully"))
        }
        [arg] if arg == "--debug-events" => native::run(false, |event| {
            println!("{event}");
        }),
        [arg] if arg == "--debug-snapshot" => snapshot::collect().map(|sockets| {
            for socket in &sockets {
                let protocol = if socket.protocol == 6 { "TCP" } else { "UDP" };
                let family = if socket.family == 2 { "IPv4" } else { "IPv6" };

                println!(
                    "snapshot {protocol} {family} port={} cookie={} inode={}",
                    socket.local_port, socket.socket_id, socket.inode
                );
            }

            println!("snapshot TOTAL={}", sockets.len());
        }),
        [arg] if arg == "--debug-state" => native::run_with_recovery(
            false,
            |_| {},
            move |event| {
                let protocol = if event.protocol == 6 { "TCP" } else { "UDP" };
                println!(
                    "state EVENT {protocol} type={} id={} pid={} port={}",
                    event.event_type, event.socket_id, event.pid, event.local_port
                );
            },
            || println!("state RESYNC_REQUIRED"),
            |state| {
                println!("state SNAPSHOT active={}", state.len());
                for socket in state.sockets() {
                    let protocol = if socket.protocol == 6 { "TCP" } else { "UDP" };
                    println!(
                        "state SOCKET {protocol} id={} pid={} family={} port={} timestamp={}",
                        socket.socket_id,
                        socket.pid,
                        socket.family,
                        socket.local_port,
                        socket.timestamp_ns
                    );
                }
            },
        ),
        _ => {
            eprintln!(
                "Usage: port-killer-events --serve | --watch | --check | --debug-events | --debug-snapshot | --debug-state"
            );
            process::exit(2);
        }
    };
    if let Err(error) = result {
        eprintln!("port-killer-events: {error}");
        process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    fn path() -> std::path::PathBuf {
        env::temp_dir().join(format!(
            "pk-test-{}-{}",
            process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
    }
    #[test]
    fn protocol_readiness_and_invalidation() {
        let (server, client) = UnixStream::pair().unwrap();
        client
            .set_read_timeout(Some(Duration::from_millis(500)))
            .unwrap();
        let mut broker = Broker::default();
        broker.subscribe(server).unwrap();
        let mut reader = BufReader::new(client);
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        assert_eq!(line, "waiting\n");
        broker.publish("ready");
        broker.publish("change");
        broker.publish("resync");
        for expected in ["ready\n", "change\n", "resync\n"] {
            line.clear();
            reader.read_line(&mut line).unwrap();
            assert_eq!(line, expected);
        }
        assert!(broker.ready);
        broker.publish("arbitrary payload");
        assert!(reader.read_line(&mut String::new()).is_err());
    }
    #[test]
    fn disconnected_subscribers_are_removed() {
        let (server, client) = UnixStream::pair().unwrap();
        let mut broker = Broker::default();
        broker.subscribe(server).unwrap();
        drop(client);
        broker.publish("change");
        assert!(broker.clients.is_empty());
    }
    #[test]
    fn subscriptions_reclaim_idle_disconnected_clients() {
        let mut broker = Broker::default();
        for _ in 0..MAX_CLIENTS + 5 {
            let (server, client) = UnixStream::pair().unwrap();
            broker.subscribe(server).unwrap();
            assert_eq!(broker.clients.len(), 1);
            drop(client);
        }
    }
    #[test]
    fn refuses_non_socket_paths_and_active_daemons() {
        let path = path();
        fs::write(&path, "keep").unwrap();
        assert!(prepare_listener(&path).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "keep");
        fs::remove_file(&path).unwrap();
        let listener = prepare_listener(&path).unwrap();
        assert!(prepare_listener(&path).is_err());
        drop(listener);
        fs::remove_file(path).unwrap();
    }
    #[test]
    fn watcher_flushes_events_and_reports_disconnect() {
        let path = path();
        let listener = UnixListener::bind(&path).unwrap();
        let producer = thread::spawn(move || {
            let (mut client, _) = listener.accept().unwrap();
            client.write_all(b"ready\nchange\nresync\n").unwrap();
        });
        let mut output = Vec::new();
        assert_eq!(
            watch(&path, &mut output).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
        assert_eq!(output, b"ready\nchange\nresync\n");
        producer.join().unwrap();
        fs::remove_file(path).unwrap();
    }
}
