use aya::{
    maps::RingBuf,
    programs::TracePoint,
    Ebpf,
};
use std::{
    error::Error,
    os::fd::AsRawFd,
};

fn main() -> Result<(), Box<dyn Error>> {
    let object = std::fs::read(
        "experiments/socket-identity/identity.bpf.o"
    )?;

    let mut bpf = Ebpf::load(&object)?;

    for (name, event) in [
        ("enter_listen", "sys_enter_listen"),
        ("exit_listen", "sys_exit_listen"),
    ] {
        let program: &mut TracePoint = bpf
            .program_mut(name)
            .ok_or("missing tracepoint program")?
            .try_into()?;

        program.load()?;
        program.attach("syscalls", event)?;
    }

    {
        let program: &mut TracePoint = bpf
            .program_mut("inspect_tcp_state")
            .ok_or("missing TCP state program")?
            .try_into()?;

        program.load()?;
        program.attach("sock", "inet_sock_set_state")?;
    }

    for (name, event) in [
        ("enter_bind_udp", "sys_enter_bind"),
        ("exit_bind_udp", "sys_exit_bind"),
    ] {
        let program: &mut TracePoint = bpf
            .program_mut(name)
            .ok_or("missing UDP bind program")?
            .try_into()?;

        program.load()?;
        program.attach("syscalls", event)?;
    }

    {
        use aya::programs::KProbe;

        let program: &mut KProbe = bpf
            .program_mut("release_udp")
            .ok_or("missing UDP release program")?
            .try_into()?;

        program.load()?;
        program.attach("__sock_release", 0)?;
    }

    let mut ring = RingBuf::try_from(
        bpf.take_map("events").ok_or("missing ring buffer")?
    )?;

    println!("Socket identity experiment running.");
    println!("Open a TCP listener in another terminal.");
    println!("Press Ctrl+C to stop.");

    loop {
        while let Some(event) = ring.next() {
            if event.len() != 40 {
                eprintln!("Unexpected event size: {}", event.len());
                continue;
            }

            let socket_ptr =
                u64::from_ne_bytes(event[0..8].try_into()?);

            let cookie =
                u64::from_ne_bytes(event[8..16].try_into()?);

            let inode =
                u64::from_ne_bytes(event[16..24].try_into()?);

            let pid =
                u32::from_ne_bytes(event[24..28].try_into()?);

            let family =
                u16::from_ne_bytes(event[28..30].try_into()?);

            let port =
                u16::from_ne_bytes(event[30..32].try_into()?);

            let kind = u32::from_ne_bytes(event[32..36].try_into()?);

            let label = match kind {
                1 => "LISTEN_SUCCESS",
                2 => "LISTEN_STATE",
                3 => "LEAVE_LISTEN",
                4 => "UDP_BIND",
                5 => "UDP_CLOSE",
                _ => "UNKNOWN",
            };

            println!(
                "{} pid={} family={} port={} inode={} cookie={} socket_ptr={:#x}",
                label, pid, family, port, inode, cookie, socket_ptr
            );
        }

        let mut pollfd = libc::pollfd {
            fd: ring.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };

        let result = unsafe {
            libc::poll(&mut pollfd, 1, -1)
        };

        if result < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
    }
}
