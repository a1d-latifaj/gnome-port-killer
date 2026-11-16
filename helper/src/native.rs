//! Load embedded CO-RE bytecode directly; no compiler or bpftrace at runtime.
use aya::{
    maps::{Array, RingBuf},
    programs::{KProbe, TracePoint},
    Ebpf,
};
use std::{io, os::fd::AsRawFd};

fn error(value: impl std::fmt::Display) -> io::Error {
    io::Error::other(value.to_string())
}

pub fn run(check_only: bool, mut publish: impl FnMut(&str)) -> io::Result<()> {
    let mut bpf = Ebpf::load(aya::include_bytes_aligned!(concat!(
        env!("OUT_DIR"),
        "/monitor.bpf.o"
    )))
    .map_err(error)?;
    for (name, category, event) in [
        ("enter_bind", "syscalls", "sys_enter_bind"),
        ("exit_bind", "syscalls", "sys_exit_bind"),
        ("enter_listen", "syscalls", "sys_enter_listen"),
        ("exit_listen", "syscalls", "sys_exit_listen"),
        ("tcp_state", "sock", "inet_sock_set_state"),
        ("process_exit", "sched", "sched_process_exit"),
    ] {
        let program: &mut TracePoint = bpf
            .program_mut(name)
            .ok_or_else(|| error(name))?
            .try_into()
            .map_err(error)?;
        program.load().map_err(error)?;
        program.attach(category, event).map_err(error)?;
    }
    let release: &mut KProbe = bpf
        .program_mut("release_socket")
        .ok_or_else(|| error("release_socket"))?
        .try_into()
        .map_err(error)?;
    release.load().map_err(error)?;
    release.attach("__sock_release", 0).map_err(error)?;
    let mut ring = RingBuf::try_from(
        bpf.take_map("events")
            .ok_or_else(|| error("events map missing"))?,
    )
    .map_err(error)?;
    let lost = Array::<_, u64>::try_from(
        bpf.take_map("lost")
            .ok_or_else(|| error("lost map missing"))?,
    )
    .map_err(error)?;
    let mut previous_lost = 0;
    publish("ready");
    if check_only {
        return Ok(());
    }
    loop {
        let mut changed = [false; 3];
        for _ in 0..4096 {
            let Some(event) = ring.next() else {
                break;
            };
            if event.len() != 8 {
                publish("resync");
                continue;
            }
            let proto = u32::from_ne_bytes(event[0..4].try_into().unwrap());
            if (1..=3).contains(&proto) {
                changed[(proto - 1) as usize] = true;
            } else {
                publish("resync");
            }
        }
        for (changed, message) in
            changed
                .into_iter()
                .zip(["change TCP", "change UDP", "change UNIX"])
        {
            if changed {
                publish(message);
            }
        }
        let count = lost.get(&0, 0).map_err(error)?;
        if count != previous_lost {
            previous_lost = count;
            publish("resync");
        }
        let mut pollfd = libc::pollfd {
            fd: ring.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one valid pollfd lives until poll returns. -1 blocks until an
        // event; this is not a periodic wakeup or snapshot poll.
        let result = unsafe { libc::poll(&mut pollfd, 1, -1) };
        if result < 0 {
            let cause = io::Error::last_os_error();
            if cause.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(cause);
        }
        if pollfd.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
            return Err(error("Kernel event ring disconnected"));
        }
    }
}
