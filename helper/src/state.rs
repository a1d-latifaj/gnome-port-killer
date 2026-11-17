use std::collections::HashMap;

use crate::event::{EventType, SocketEvent};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SocketInfo {
    pub socket_id: u64,
    pub pid: u32,
    pub protocol: u16,
    pub family: u16,
    pub local_port: u16,
    pub timestamp_ns: u64,
}

impl SocketInfo {
    /// Build state from SOCK_DIAG without inventing process ownership.
    /// Timestamp zero means the socket came from a snapshot, not an event.
    pub fn from_snapshot(socket: crate::snapshot::SnapshotSocket) -> Self {
        Self {
            socket_id: socket.socket_id,
            pid: 0,
            protocol: socket.protocol,
            family: socket.family,
            local_port: socket.local_port,
            timestamp_ns: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateChange {
    Added(SocketInfo),
    Removed(SocketInfo),
    Updated(SocketInfo),
    Unchanged,
    Invalid,
    ResyncRequired,
}

#[derive(Debug, Clone, Default)]
pub struct SocketState {
    sockets: HashMap<u64, SocketInfo>,
    last_seen: HashMap<u64, u64>,
    dirty: bool,
}

impl SocketState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.sockets.len()
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.sockets.is_empty()
    }

    #[cfg(test)]
    pub fn get(&self, socket_id: u64) -> Option<&SocketInfo> {
        self.sockets.get(&socket_id)
    }

    pub fn sockets(&self) -> impl Iterator<Item = &SocketInfo> {
        self.sockets.values()
    }

    #[cfg(test)]
    pub fn needs_resync(&self) -> bool {
        self.dirty
    }

    pub fn mark_event_loss(&mut self) {
        self.dirty = true;
    }

    pub fn apply(&mut self, event: SocketEvent) -> StateChange {
        if !event.is_valid() || event.timestamp_ns == 0 {
            return StateChange::Invalid;
        }

        if self.dirty {
            return StateChange::ResyncRequired;
        }

        if let Some(&previous_timestamp) = self.last_seen.get(&event.socket_id) {
            if event.timestamp_ns <= previous_timestamp {
                return StateChange::Unchanged;
            }
        }

        let info = SocketInfo {
            socket_id: event.socket_id,
            pid: event.pid,
            protocol: event.protocol,
            family: event.family,
            local_port: event.local_port,
            timestamp_ns: event.timestamp_ns,
        };

        match event.event_type {
            x if x == EventType::TcpListen as u16 || x == EventType::UdpBind as u16 => {
                let previous = self.sockets.insert(event.socket_id, info);
                self.last_seen.insert(event.socket_id, event.timestamp_ns);

                match previous {
                    None => StateChange::Added(info),
                    Some(old)
                        if old.pid == info.pid
                            && old.protocol == info.protocol
                            && old.family == info.family
                            && old.local_port == info.local_port =>
                    {
                        StateChange::Unchanged
                    }
                    Some(_) => StateChange::Updated(info),
                }
            }

            x if x == EventType::TcpClose as u16 || x == EventType::UdpClose as u16 => {
                match self.sockets.get(&event.socket_id).copied() {
                    Some(existing)
                        if existing.protocol == event.protocol
                            && existing.family == event.family
                            && existing.local_port == event.local_port =>
                    {
                        self.sockets.remove(&event.socket_id);
                        self.last_seen.insert(event.socket_id, event.timestamp_ns);
                        StateChange::Removed(existing)
                    }
                    Some(_) => {
                        self.dirty = true;
                        StateChange::ResyncRequired
                    }
                    None => {
                        // SO_COOKIE is unique for one socket lifetime. An
                        // unknown or duplicate close cannot remove another
                        // socket; retaining its timestamp blocks stale adds.
                        self.last_seen.insert(event.socket_id, event.timestamp_ns);
                        StateChange::Unchanged
                    }
                }
            }

            _ => StateChange::Invalid,
        }
    }

    pub fn replace_snapshot(&mut self, sockets: impl IntoIterator<Item = SocketInfo>) {
        self.sockets.clear();
        self.last_seen.clear();

        for socket in sockets {
            self.last_seen.insert(socket.socket_id, socket.timestamp_ns);
            self.sockets.insert(socket.socket_id, socket);
        }

        self.dirty = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::EVENT_VERSION;

    #[test]
    fn snapshot_socket_preserves_cookie_without_inventing_pid() {
        let snapshot = crate::snapshot::SnapshotSocket {
            inode: 12345,
            socket_id: 987654321,
            protocol: 6,
            family: 10,
            local_port: 8080,
        };

        let info = SocketInfo::from_snapshot(snapshot);

        assert_eq!(info.socket_id, 987654321);
        assert_eq!(info.pid, 0);
        assert_eq!(info.protocol, 6);
        assert_eq!(info.family, 10);
        assert_eq!(info.local_port, 8080);
        assert_eq!(info.timestamp_ns, 0);
    }

    #[test]
    fn stale_close_cannot_remove_new_socket_with_same_port() {
        let mut state = SocketState::new();

        // Distinct SO_COOKIE values identify distinct socket instances,
        // even when protocol, family and local port are identical.
        let old_open = event(100, EventType::TcpListen, 3000, 100);
        let new_open = event(200, EventType::TcpListen, 3000, 200);
        let old_close = event(100, EventType::TcpClose, 3000, 300);

        assert!(matches!(state.apply(old_open), StateChange::Added(_)));

        assert!(matches!(state.apply(new_open), StateChange::Added(_)));

        assert_eq!(state.len(), 2);

        assert!(matches!(state.apply(old_close), StateChange::Removed(_)));

        assert!(state.get(100).is_none());
        assert!(state.get(200).is_some());
        assert_eq!(state.len(), 1);
        assert!(!state.needs_resync());
    }

    #[test]
    fn stale_close_cannot_remove_new_socket_with_different_port() {
        let mut state = SocketState::new();

        let old_open = event(100, EventType::TcpListen, 3000, 100);
        let new_open = event(100, EventType::TcpListen, 4000, 200);
        let stale_close = event(100, EventType::TcpClose, 3000, 300);

        assert!(matches!(state.apply(old_open), StateChange::Added(_)));
        assert!(matches!(state.apply(new_open), StateChange::Updated(_)));

        assert_eq!(state.apply(stale_close), StateChange::ResyncRequired);
        assert!(state.needs_resync());

        let current = state.get(100).expect("new socket must remain");
        assert_eq!(current.local_port, 4000);
    }

    fn event(socket_id: u64, event_type: EventType, port: u16, timestamp_ns: u64) -> SocketEvent {
        SocketEvent {
            socket_id,
            timestamp_ns,
            pid: 4242,
            event_type: event_type as u16,
            family: 2,
            protocol: match event_type {
                EventType::TcpListen | EventType::TcpClose => 6,
                EventType::UdpBind | EventType::UdpClose => 17,
            },
            local_port: port,
            version: EVENT_VERSION,
            reserved: 0,
        }
    }

    #[test]
    fn tcp_lifecycle() {
        let mut state = SocketState::new();

        assert!(matches!(
            state.apply(event(100, EventType::TcpListen, 3000, 100)),
            StateChange::Added(_)
        ));

        assert!(matches!(
            state.apply(event(100, EventType::TcpClose, 3000, 200)),
            StateChange::Removed(_)
        ));

        assert!(state.is_empty());
    }

    #[test]
    fn stale_add_cannot_resurrect_closed_socket() {
        let mut state = SocketState::new();

        state.apply(event(100, EventType::TcpListen, 3000, 200));
        state.apply(event(100, EventType::TcpClose, 3000, 300));

        assert_eq!(
            state.apply(event(100, EventType::TcpListen, 3000, 100)),
            StateChange::Unchanged
        );

        assert!(state.is_empty());
    }

    #[test]
    fn unknown_close_is_harmless_by_cookie() {
        let mut state = SocketState::new();

        assert_eq!(
            state.apply(event(999, EventType::TcpClose, 3000, 100)),
            StateChange::Unchanged
        );

        assert!(!state.needs_resync());
    }

    #[test]
    fn duplicate_close_is_ignored() {
        let mut state = SocketState::new();
        state.apply(event(100, EventType::TcpListen, 3000, 100));
        assert!(matches!(
            state.apply(event(100, EventType::TcpClose, 3000, 200)),
            StateChange::Removed(_)
        ));
        assert_eq!(
            state.apply(event(100, EventType::TcpClose, 3000, 300)),
            StateChange::Unchanged
        );
        assert!(!state.needs_resync());
    }

    #[test]
    fn mismatched_close_requires_resync() {
        let mut state = SocketState::new();

        state.apply(event(100, EventType::TcpListen, 3000, 100));

        assert_eq!(
            state.apply(event(100, EventType::TcpClose, 4000, 200)),
            StateChange::ResyncRequired
        );

        assert!(state.needs_resync());
    }

    #[test]
    fn event_loss_blocks_updates_until_snapshot() {
        let mut state = SocketState::new();

        let listen = SocketEvent {
            socket_id: 9001,
            timestamp_ns: 100,
            pid: 4242,
            event_type: 1,
            family: 2,
            protocol: 6,
            local_port: 8080,
            version: crate::event::EVENT_VERSION,
            reserved: 0,
        };

        assert!(matches!(state.apply(listen), StateChange::Added(_)));
        assert_eq!(state.len(), 1);

        state.mark_event_loss();
        assert!(state.needs_resync());

        let close = SocketEvent {
            event_type: 2,
            timestamp_ns: 200,
            ..listen
        };

        assert_eq!(state.apply(close), StateChange::ResyncRequired);
        assert_eq!(state.len(), 1);

        state.replace_snapshot(std::iter::empty());
        assert!(!state.needs_resync());
        assert_eq!(state.len(), 0);

        let reopened = SocketEvent {
            socket_id: 9002,
            timestamp_ns: 300,
            ..listen
        };

        assert!(matches!(state.apply(reopened), StateChange::Added(_)));
        assert_eq!(state.len(), 1);
    }

    #[test]
    fn event_loss_requires_resync() {
        let mut state = SocketState::new();

        state.mark_event_loss();

        assert!(state.needs_resync());

        assert_eq!(
            state.apply(event(100, EventType::TcpListen, 3000, 100)),
            StateChange::ResyncRequired
        );
    }

    #[test]
    fn snapshot_recovers_state() {
        let mut state = SocketState::new();

        state.mark_event_loss();

        state.replace_snapshot([SocketInfo {
            socket_id: 100,
            pid: 4242,
            protocol: 6,
            family: 2,
            local_port: 3000,
            timestamp_ns: 100,
        }]);

        assert!(!state.needs_resync());
        assert_eq!(state.len(), 1);
    }

    #[test]
    fn duplicate_event_is_ignored() {
        let mut state = SocketState::new();

        let e = event(100, EventType::TcpListen, 3000, 100);

        state.apply(e);

        assert_eq!(state.apply(e), StateChange::Unchanged);
        assert_eq!(state.len(), 1);
    }

    #[test]
    fn udp_lifecycle() {
        let mut state = SocketState::new();

        state.apply(event(200, EventType::UdpBind, 5353, 100));

        assert!(matches!(
            state.apply(event(200, EventType::UdpClose, 5353, 200)),
            StateChange::Removed(_)
        ));

        assert!(state.is_empty());
    }

    #[test]
    fn snapshot_then_buffered_close_removes_socket() {
        let mut state = SocketState::new();

        let socket = SocketInfo {
            socket_id: 500,
            pid: 0,
            protocol: 6,
            family: 2,
            local_port: 3000,
            timestamp_ns: 0,
        };

        // Snapshot contains a TCP listener.
        state.replace_snapshot([socket]);

        assert_eq!(state.len(), 1);

        // TCP_CLOSE occurred while the snapshot was being collected.
        let close = event(500, EventType::TcpClose, 3000, 120);

        assert!(matches!(state.apply(close), StateChange::Removed(_)));

        assert!(state.is_empty());
        assert!(!state.needs_resync());
    }
}
