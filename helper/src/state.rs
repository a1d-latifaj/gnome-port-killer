use std::collections::HashMap;

use crate::event::{EventType, SocketEvent};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SocketInfo {
    pub socket_id: u64,
    pub pid: u32,
    pub protocol: u16,
    pub family: u16,
    pub local_port: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateChange {
    Added(SocketInfo),
    Removed(SocketInfo),
    Updated(SocketInfo),
    Unchanged,
    Invalid,
}

#[derive(Default)]
pub struct SocketState {
    sockets: HashMap<u64, SocketInfo>,
}

impl SocketState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.sockets.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sockets.is_empty()
    }

    pub fn get(&self, socket_id: u64) -> Option<&SocketInfo> {
        self.sockets.get(&socket_id)
    }

    pub fn apply(&mut self, event: SocketEvent) -> StateChange {
        if !event.is_valid() {
            return StateChange::Invalid;
        }

        let info = SocketInfo {
            socket_id: event.socket_id,
            pid: event.pid,
            protocol: event.protocol,
            family: event.family,
            local_port: event.local_port,
        };

        match event.event_type {
            x if x == EventType::TcpListen as u16 || x == EventType::UdpBind as u16 => {
                match self.sockets.insert(event.socket_id, info) {
                    None => StateChange::Added(info),
                    Some(previous) if previous == info => StateChange::Unchanged,
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
                        StateChange::Removed(existing)
                    }
                    _ => StateChange::Unchanged,
                }
            }

            _ => StateChange::Invalid,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::EVENT_VERSION;

    fn event(socket_id: u64, event_type: EventType, port: u16) -> SocketEvent {
        SocketEvent {
            socket_id,
            timestamp_ns: 1000,
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
    fn tcp_listen_adds_socket() {
        let mut state = SocketState::new();

        let result = state.apply(event(100, EventType::TcpListen, 3000));

        assert!(matches!(result, StateChange::Added(_)));
        assert_eq!(state.len(), 1);
        assert_eq!(state.get(100).unwrap().local_port, 3000);
    }

    #[test]
    fn tcp_close_removes_socket() {
        let mut state = SocketState::new();

        state.apply(event(100, EventType::TcpListen, 3000));

        let result = state.apply(event(100, EventType::TcpClose, 3000));

        assert!(matches!(result, StateChange::Removed(_)));
        assert!(state.is_empty());
    }

    #[test]
    fn udp_bind_and_close() {
        let mut state = SocketState::new();

        assert!(matches!(
            state.apply(event(200, EventType::UdpBind, 5353)),
            StateChange::Added(_)
        ));

        assert!(matches!(
            state.apply(event(200, EventType::UdpClose, 5353)),
            StateChange::Removed(_)
        ));

        assert!(state.is_empty());
    }

    #[test]
    fn duplicate_event_does_not_duplicate_socket() {
        let mut state = SocketState::new();

        state.apply(event(100, EventType::TcpListen, 3000));

        assert_eq!(
            state.apply(event(100, EventType::TcpListen, 3000)),
            StateChange::Unchanged
        );

        assert_eq!(state.len(), 1);
    }

    #[test]
    fn invalid_event_is_rejected() {
        let mut state = SocketState::new();

        let result = state.apply(event(0, EventType::TcpListen, 3000));

        assert_eq!(result, StateChange::Invalid);
        assert!(state.is_empty());
    }

    #[test]
    fn close_for_unknown_socket_is_ignored() {
        let mut state = SocketState::new();

        assert_eq!(
            state.apply(event(999, EventType::TcpClose, 3000)),
            StateChange::Unchanged
        );

        assert!(state.is_empty());
    }

    #[test]
    fn close_with_mismatched_port_does_not_remove_socket() {
        let mut state = SocketState::new();

        state.apply(event(100, EventType::TcpListen, 3000));

        assert_eq!(
            state.apply(event(100, EventType::TcpClose, 4000)),
            StateChange::Unchanged
        );

        assert_eq!(state.len(), 1);
    }

    #[test]
    fn multiple_sockets_are_tracked_independently() {
        let mut state = SocketState::new();

        state.apply(event(100, EventType::TcpListen, 3000));
        state.apply(event(200, EventType::TcpListen, 8080));
        state.apply(event(300, EventType::UdpBind, 5353));

        assert_eq!(state.len(), 3);

        state.apply(event(200, EventType::TcpClose, 8080));

        assert_eq!(state.len(), 2);
        assert!(state.get(100).is_some());
        assert!(state.get(200).is_none());
        assert!(state.get(300).is_some());
    }
}
