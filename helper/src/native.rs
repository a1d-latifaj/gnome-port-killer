//! Load embedded CO-RE bytecode directly; no compiler or bpftrace at runtime.
use aya::{
    maps::{Array, RingBuf},
    programs::{FEntry, FExit, TracePoint},
    Btf, Ebpf,
};
use std::{io, os::fd::AsRawFd};

use crate::event::SocketEvent;
use crate::event_batch::{BatchResult, EventBatch};
use crate::recovery_gate::RecoveryGate;
use crate::state::{SocketState, StateChange};

const SNAPSHOT_ATTEMPTS_PER_WAKE: usize = 3;

fn error(value: impl std::fmt::Display) -> io::Error {
    io::Error::other(value.to_string())
}

fn monotonic_ns() -> io::Result<u64> {
    let mut time: libc::timespec = unsafe { std::mem::zeroed() };
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut time) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let seconds = u64::try_from(time.tv_sec).map_err(|_| error("negative monotonic time"))?;
    let nanos = u64::try_from(time.tv_nsec).map_err(|_| error("negative monotonic time"))?;
    seconds
        .checked_mul(1_000_000_000)
        .and_then(|value| value.checked_add(nanos))
        .ok_or_else(|| error("monotonic time overflow"))
}

fn decode_event(raw: &[u8]) -> io::Result<SocketEvent> {
    if raw.len() != std::mem::size_of::<SocketEvent>() {
        return Err(error("invalid lifecycle event length"));
    }
    Ok(SocketEvent {
        socket_id: u64::from_ne_bytes(raw[0..8].try_into().map_err(error)?),
        timestamp_ns: u64::from_ne_bytes(raw[8..16].try_into().map_err(error)?),
        pid: u32::from_ne_bytes(raw[16..20].try_into().map_err(error)?),
        event_type: u16::from_ne_bytes(raw[20..22].try_into().map_err(error)?),
        family: u16::from_ne_bytes(raw[22..24].try_into().map_err(error)?),
        protocol: u16::from_ne_bytes(raw[24..26].try_into().map_err(error)?),
        local_port: u16::from_ne_bytes(raw[26..28].try_into().map_err(error)?),
        version: u16::from_ne_bytes(raw[28..30].try_into().map_err(error)?),
        reserved: u16::from_ne_bytes(raw[30..32].try_into().map_err(error)?),
    })
}

fn reconcile_attempt(
    snapshot: &[crate::snapshot::SnapshotSocket],
    events: &[SocketEvent],
    start_ns: u64,
    end_ns: u64,
    lost_before: u64,
    lost_after: u64,
) -> io::Result<SocketState> {
    if lost_before != lost_after {
        return Err(error("kernel event loss during snapshot reconciliation"));
    }
    let window = crate::reconcile::SnapshotWindow::new(start_ns, end_ns)
        .ok_or_else(|| error("invalid snapshot window"))?;
    crate::reconcile::reconcile_snapshot(snapshot, events, &window).map_err(error)
}

/// One conservative initialization or recovery attempt. The counter is
/// cumulative: only a change during this attempt invalidates the snapshot.
fn collect_reconciled_snapshot(
    ring: &mut RingBuf<aya::maps::MapData>,
    lifecycle_ring: &mut RingBuf<aya::maps::MapData>,
    lost: &Array<aya::maps::MapData, u64>,
) -> io::Result<(SocketState, u64)> {
    let lost_before = lost.get(&0, 0).map_err(error)?;
    let start_ns = monotonic_ns()?;
    let snapshot = crate::snapshot::collect()?;
    let end_ns = monotonic_ns()?;

    // Records through end_ns were committed before this drain. Later records
    // either join the replay set or remain queued for the next batch.
    let mut events = Vec::new();
    while let Some(raw) = lifecycle_ring.next() {
        events.push(decode_event(&raw)?);
    }
    // Generic invalidations carry no state and are superseded by the complete
    // snapshot callback. They still must be drained so their ring cannot force
    // avoidable loss while publication is blocked.
    while ring.next().is_some() {}
    let drained_at = monotonic_ns()?;
    if events.iter().any(|event| event.timestamp_ns > drained_at) {
        return Err(error("lifecycle event timestamp is in the future"));
    }
    let lost_after = lost.get(&0, 0).map_err(error)?;
    let state = reconcile_attempt(
        &snapshot,
        &events,
        start_ns,
        end_ns,
        lost_before,
        lost_after,
    )?;
    Ok((state, lost_after))
}

fn collect_reconciled_snapshot_with_retries(
    ring: &mut RingBuf<aya::maps::MapData>,
    lifecycle_ring: &mut RingBuf<aya::maps::MapData>,
    lost: &Array<aya::maps::MapData, u64>,
) -> io::Result<(SocketState, u64)> {
    let mut last_error = None;
    for _ in 0..SNAPSHOT_ATTEMPTS_PER_WAKE {
        match collect_reconciled_snapshot(ring, lifecycle_ring, lost) {
            Ok(result) => return Ok(result),
            Err(cause) => last_error = Some(cause),
        }
    }
    Err(last_error.unwrap_or_else(|| error("snapshot recovery was not attempted")))
}

fn apply_events_atomically(
    state: &SocketState,
    events: &[SocketEvent],
) -> Result<SocketState, &'static str> {
    let mut candidate = state.clone();
    for &event in events {
        match candidate.apply(event) {
            StateChange::Added(_)
            | StateChange::Removed(_)
            | StateChange::Updated(_)
            | StateChange::Unchanged => {}
            StateChange::Invalid | StateChange::ResyncRequired => {
                return Err("lifecycle batch cannot be applied atomically");
            }
        }
    }
    Ok(candidate)
}

pub fn run(check_only: bool, publish: impl FnMut(&str)) -> io::Result<()> {
    run_with_state(check_only, publish, |_| {})
}

pub fn run_with_state(
    check_only: bool,
    publish: impl FnMut(&str),
    on_socket: impl FnMut(SocketEvent),
) -> io::Result<()> {
    run_with_recovery(check_only, publish, on_socket, || {}, |_| {})
}

/// `on_snapshot` observes the one authoritative native state after a complete
/// startup or recovery replacement. A caller that mirrors it must replace,
/// never merge, its old state.
pub fn run_with_recovery(
    check_only: bool,
    mut publish: impl FnMut(&str),
    mut on_socket: impl FnMut(SocketEvent),
    mut on_resync: impl FnMut(),
    mut on_snapshot: impl FnMut(&SocketState),
) -> io::Result<()> {
    let mut bpf = Ebpf::load(aya::include_bytes_aligned!(concat!(
        env!("OUT_DIR"),
        "/monitor.bpf.o"
    )))
    .map_err(error)?;
    let kernel_btf = Btf::from_sys_fs().map_err(error)?;

    let state_program: &mut TracePoint = bpf
        .program_mut("tcp_state")
        .ok_or_else(|| error("tcp_state"))?
        .try_into()
        .map_err(error)?;
    state_program.load().map_err(error)?;
    state_program
        .attach("sock", "inet_sock_set_state")
        .map_err(error)?;

    for (name, kernel_function) in [
        ("udp_bind4", "inet_bind"),
        ("udp_bind6", "inet6_bind"),
        ("tcp_listen", "inet_listen"),
    ] {
        let program: &mut FExit = bpf
            .program_mut(name)
            .ok_or_else(|| error(name))?
            .try_into()
            .map_err(error)?;
        program.load(kernel_function, &kernel_btf).map_err(error)?;
        program.attach().map_err(error)?;
    }

    let release: &mut FEntry = bpf
        .program_mut("release_socket")
        .ok_or_else(|| error("release_socket"))?
        .try_into()
        .map_err(error)?;
    release.load("__sock_release", &kernel_btf).map_err(error)?;
    release.attach().map_err(error)?;

    let mut ring = RingBuf::try_from(
        bpf.take_map("events")
            .ok_or_else(|| error("events map missing"))?,
    )
    .map_err(error)?;
    let mut lifecycle_ring = RingBuf::try_from(
        bpf.take_map("lifecycle_events")
            .ok_or_else(|| error("lifecycle_events map missing"))?,
    )
    .map_err(error)?;
    let lost = Array::<_, u64>::try_from(
        bpf.take_map("lost")
            .ok_or_else(|| error("lost map missing"))?,
    )
    .map_err(error)?;

    let _initialized_lost = lost.get(&0, 0).map_err(error)?;
    if check_only {
        publish("ready");
        return Ok(());
    }

    let (mut socket_state, snapshot_lost) =
        collect_reconciled_snapshot_with_retries(&mut ring, &mut lifecycle_ring, &lost)?;
    let mut previous_lost = snapshot_lost;
    // This counter read is the installation boundary. Later loss is a new
    // HEALTHY -> RESYNC transition, observed before another batch is applied.
    if lost.get(&0, 0).map_err(error)? != previous_lost {
        return Err(error("kernel event loss at initial snapshot installation"));
    }
    on_snapshot(&socket_state);
    publish("ready");

    let mut recovery_gate = RecoveryGate::new();
    loop {
        let observed_lost = lost.get(&0, 0).map_err(error)?;
        if observed_lost != previous_lost && recovery_gate.mark_loss() {
            socket_state.mark_event_loss();
            on_resync();
            publish("resync");
        }
        previous_lost = observed_lost;

        if !recovery_gate.can_publish_lifecycle() {
            if recovery_gate.begin_recovery() {
                match collect_reconciled_snapshot_with_retries(
                    &mut ring,
                    &mut lifecycle_ring,
                    &lost,
                ) {
                    Ok((recovered, recovery_lost))
                        if lost.get(&0, 0).map_err(error)? == recovery_lost =>
                    {
                        socket_state = recovered;
                        previous_lost = recovery_lost;
                        on_snapshot(&socket_state);
                        recovery_gate.complete_recovery();
                        publish("change");
                        continue;
                    }
                    Ok(_) => {
                        recovery_gate.fail_recovery();
                        eprintln!("port-killer-events: recovery invalidated by event loss");
                    }
                    Err(cause) => {
                        recovery_gate.fail_recovery();
                        eprintln!("port-killer-events: recovery failed: {cause}");
                    }
                }
                previous_lost = lost.get(&0, 0).map_err(error)?;
            }
            poll_rings(&ring, &lifecycle_ring)?;
            continue;
        }

        let lost_before = previous_lost;
        let mut generic_invalid = false;
        let mut changed = [false; 3];
        for _ in 0..4096 {
            let Some(record) = ring.next() else { break };
            if record.len() != 8 {
                generic_invalid = true;
                continue;
            }
            let proto = u32::from_ne_bytes(record[0..4].try_into().map_err(error)?);
            if (1..=3).contains(&proto) {
                changed[(proto - 1) as usize] = true;
            } else {
                generic_invalid = true;
            }
        }

        let mut batch = EventBatch::new();
        for _ in 0..4096 {
            let Some(raw) = lifecycle_ring.next() else {
                break;
            };
            match decode_event(&raw) {
                Ok(event) => batch.push(event),
                Err(_) => batch.invalidate(),
            }
        }
        let lost_after = lost.get(&0, 0).map_err(error)?;
        let drained_at = monotonic_ns()?;

        let mut events = Vec::new();
        let result = batch.finish_at(lost_before, lost_after, drained_at, |event| {
            events.push(event)
        });
        let mut candidate = if !generic_invalid && matches!(result, BatchResult::Accepted(_)) {
            apply_events_atomically(&socket_state, &events).ok()
        } else {
            None
        };
        // Validate loss again after semantic application but before commit.
        // Loss after this boundary is treated as the next transition and is
        // observed at the top of the loop before another batch is accepted.
        let commit_lost = lost.get(&0, 0).map_err(error)?;
        if commit_lost != lost_after {
            candidate = None;
        }
        previous_lost = commit_lost;

        if let Some(candidate) = candidate {
            let mut publication_state = socket_state.clone();
            socket_state = candidate;
            let mut lifecycle_changed = [false; 2];
            for event in events {
                let change = publication_state.apply(event);
                if !matches!(
                    change,
                    StateChange::Added(_)
                        | StateChange::Removed(_)
                        | StateChange::Updated(_)
                ) {
                    continue;
                }

                match event.event_type {
                    1 | 2 => lifecycle_changed[0] = true,
                    3 | 4 => lifecycle_changed[1] = true,
                    _ => unreachable!(),
                }

                let kind = match event.event_type {
                    1 => "TCP_LISTEN",
                    2 => "TCP_CLOSE",
                    3 => "UDP_BIND",
                    4 => "UDP_CLOSE",
                    _ => unreachable!(),
                };
                on_socket(event);
                publish(&format!(
                    "lifecycle {kind} id={} pid={} family={} port={}",
                    event.socket_id, event.pid, event.family, event.local_port
                ));
            }
            for (did_change, message) in
                [changed[0] || lifecycle_changed[0], lifecycle_changed[1], changed[2]]
                    .into_iter()
                    .zip(["change TCP", "change UDP", "change UNIX"])
            {
                if did_change {
                    publish(message);
                }
            }
        } else if recovery_gate.mark_loss() {
            socket_state.mark_event_loss();
            on_resync();
            publish("resync");
            continue;
        }

        poll_rings(&ring, &lifecycle_ring)?;
    }
}

fn poll_rings(
    ring: &RingBuf<aya::maps::MapData>,
    lifecycle_ring: &RingBuf<aya::maps::MapData>,
) -> io::Result<()> {
    let mut pollfds = [
        libc::pollfd {
            fd: ring.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        },
        libc::pollfd {
            fd: lifecycle_ring.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        },
    ];
    let result = unsafe { libc::poll(pollfds.as_mut_ptr(), pollfds.len() as libc::nfds_t, -1) };
    if result < 0 {
        let cause = io::Error::last_os_error();
        if cause.kind() == io::ErrorKind::Interrupted {
            return Ok(());
        }
        return Err(cause);
    }
    if pollfds
        .iter()
        .any(|fd| fd.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0)
    {
        return Err(error("kernel event ring disconnected"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{EventType, EVENT_VERSION};
    use crate::snapshot::SnapshotSocket;

    fn snapshot(id: u64, protocol: u16, family: u16, port: u16) -> SnapshotSocket {
        SnapshotSocket {
            inode: id + 1000,
            socket_id: id,
            protocol,
            family,
            local_port: port,
        }
    }

    fn event(id: u64, kind: EventType, timestamp_ns: u64) -> SocketEvent {
        let protocol = if matches!(kind, EventType::TcpListen | EventType::TcpClose) {
            6
        } else {
            17
        };
        SocketEvent {
            socket_id: id,
            timestamp_ns,
            pid: 42,
            event_type: kind as u16,
            family: 2,
            protocol,
            local_port: 3000,
            version: EVENT_VERSION,
            reserved: 0,
        }
    }

    #[test]
    fn cumulative_historical_loss_does_not_invalidate_snapshot() {
        assert!(reconcile_attempt(&[], &[], 100, 200, 9, 9)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn loss_change_during_attempt_invalidates_snapshot() {
        assert!(reconcile_attempt(&[], &[], 100, 200, 9, 10).is_err());
    }

    #[test]
    fn initialization_accepts_tcp_udp_ipv4_ipv6_and_unknown_pid() {
        let sockets = [
            snapshot(1, 6, 2, 3000),
            snapshot(2, 6, 10, 3001),
            snapshot(3, 17, 2, 3002),
            snapshot(4, 17, 10, 3003),
        ];
        let state = reconcile_attempt(&sockets, &[], 100, 200, 0, 0).unwrap();
        assert_eq!(state.len(), 4);
        assert!(state
            .sockets()
            .all(|socket| socket.pid == 0 && socket.timestamp_ns == 0));
    }

    #[test]
    fn recovery_replays_events_after_snapshot() {
        let sockets = [snapshot(1, 6, 2, 3000)];
        let state = reconcile_attempt(
            &sockets,
            &[event(1, EventType::TcpClose, 201)],
            100,
            200,
            5,
            5,
        )
        .unwrap();
        assert!(state.is_empty());
    }

    #[test]
    fn recovery_rejects_event_during_snapshot() {
        let bind = event(1, EventType::UdpBind, 150);
        assert!(reconcile_attempt(&[], &[bind], 100, 200, 5, 5).is_err());
    }

    #[test]
    fn batch_application_is_transactional() {
        let mut state = SocketState::new();
        state.replace_snapshot([crate::state::SocketInfo::from_snapshot(snapshot(
            1, 6, 2, 3000,
        ))]);
        let close = event(1, EventType::TcpClose, 300);
        let mut mismatched = event(1, EventType::TcpClose, 400);
        mismatched.local_port = 4000;
        assert!(apply_events_atomically(&state, &[mismatched, close]).is_err());
        assert!(state.get(1).is_some());
    }

    #[test]
    fn failed_recovery_blocks_then_successful_retry_restores_publication() {
        let mut gate = RecoveryGate::new();
        let mut current = SocketState::new();
        current.replace_snapshot([crate::state::SocketInfo::from_snapshot(snapshot(
            1, 6, 2, 3000,
        ))]);

        assert!(gate.mark_loss());
        current.mark_event_loss();
        assert!(gate.begin_recovery());
        assert!(reconcile_attempt(
            &[snapshot(2, 17, 2, 3000)],
            &[event(2, EventType::UdpBind, 150)],
            100,
            200,
            4,
            4,
        )
        .is_err());
        gate.fail_recovery();
        assert!(!gate.can_publish_lifecycle());
        assert!(current.needs_resync());
        assert!(current.get(1).is_some());

        assert!(gate.begin_recovery());
        current = reconcile_attempt(&[snapshot(2, 17, 2, 3000)], &[], 300, 400, 4, 4).unwrap();
        gate.complete_recovery();
        assert!(gate.can_publish_lifecycle());
        assert!(!current.needs_resync());
        assert!(current.get(1).is_none());
        assert!(current.get(2).is_some());
    }
}
