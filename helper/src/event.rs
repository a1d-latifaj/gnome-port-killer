use std::mem::{align_of, size_of};

pub const EVENT_VERSION: u16 = 2;

#[repr(u16)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventType {
    TcpListen = 1,
    TcpClose = 2,
    UdpBind = 3,
    UdpClose = 4,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SocketEvent {
    pub socket_id: u64,
    pub timestamp_ns: u64,
    pub pid: u32,
    pub event_type: u16,
    pub family: u16,
    pub protocol: u16,
    pub local_port: u16,
    pub version: u16,
    pub reserved: u16,
}

impl SocketEvent {
    pub fn is_valid(&self) -> bool {
        self.version == EVENT_VERSION
            && self.socket_id != 0
            && self.local_port != 0
            && matches!(self.family, 2 | 10)
            && matches!(self.protocol, 6 | 17)
            && matches!(self.event_type, 1..=4)
            && match self.event_type {
                1 | 2 => self.protocol == 6,
                3 | 4 => self.protocol == 17,
                _ => false,
            }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_layout_is_stable() {
        assert_eq!(size_of::<SocketEvent>(), 32);
        assert_eq!(align_of::<SocketEvent>(), 8);
    }

    #[test]
    fn valid_tcp_listen_event() {
        let event = SocketEvent {
            socket_id: 123,
            timestamp_ns: 1000,
            pid: 4242,
            event_type: EventType::TcpListen as u16,
            family: 2,
            protocol: 6,
            local_port: 3000,
            version: EVENT_VERSION,
            reserved: 0,
        };

        assert!(event.is_valid());
    }

    #[test]
    fn invalid_socket_identity_is_rejected() {
        let event = SocketEvent {
            socket_id: 0,
            timestamp_ns: 1000,
            pid: 4242,
            event_type: EventType::UdpBind as u16,
            family: 2,
            protocol: 17,
            local_port: 5353,
            version: EVENT_VERSION,
            reserved: 0,
        };

        assert!(!event.is_valid());
    }

    #[test]
    fn invalid_protocol_combination_is_rejected() {
        let event = SocketEvent {
            socket_id: 123,
            timestamp_ns: 1000,
            pid: 4242,
            event_type: EventType::TcpListen as u16,
            family: 2,
            protocol: 17,
            local_port: 3000,
            version: EVENT_VERSION,
            reserved: 0,
        };

        assert!(!event.is_valid());
    }
}
