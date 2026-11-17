//! On-demand TCP LISTEN / UDP BOUND snapshot using Linux SOCK_DIAG.
//! No `ss` subprocess, polling, or PID inference.

use std::io;
use std::mem::size_of;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::time::Duration;

const NETLINK_SOCK_DIAG: libc::c_int = 4;
const SOCK_DIAG_BY_FAMILY: u16 = 20;
const NLMSG_DONE: u16 = 3;
const NLMSG_ERROR: u16 = 2;
const NLMSG_OVERRUN: u16 = 4;
const NLM_F_DUMP_INTR: u16 = 0x10;
const NLM_F_REQUEST: u16 = 1;
const NLM_F_DUMP: u16 = 0x300;
const TCP_LISTEN: u32 = 10;
const NLMSG_HDRLEN: usize = 16;
const INET_DIAG_MSG_LEN: usize = 72;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct SnapshotSocket {
    pub inode: u64,
    pub socket_id: u64,
    pub protocol: u16,
    pub family: u16,
    pub local_port: u16,
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn aligned(length: usize) -> usize {
    (length + 3) & !3
}

fn parse_datagram(
    data: &[u8],
    sequence: u32,
    protocol: u16,
    family: u16,
    sockets: &mut Vec<SnapshotSocket>,
) -> io::Result<bool> {
    let mut offset = 0;

    while offset < data.len() {
        if data.len() - offset < NLMSG_HDRLEN {
            return Err(invalid("truncated netlink header"));
        }

        let length = u32::from_ne_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
        let kind = u16::from_ne_bytes(data[offset + 4..offset + 6].try_into().unwrap());
        let flags = u16::from_ne_bytes(data[offset + 6..offset + 8].try_into().unwrap());
        let received_sequence =
            u32::from_ne_bytes(data[offset + 8..offset + 12].try_into().unwrap());
        if length < NLMSG_HDRLEN || length > data.len() - offset {
            return Err(invalid("invalid netlink message length"));
        }

        if received_sequence != sequence {
            return Err(invalid("unexpected netlink sequence"));
        }
        if flags & NLM_F_DUMP_INTR != 0 {
            return Err(invalid("interrupted netlink dump"));
        }

        let payload = &data[offset + NLMSG_HDRLEN..offset + length];

        match kind {
            NLMSG_DONE => {
                if payload.len() >= 4 {
                    let code = i32::from_ne_bytes(payload[0..4].try_into().unwrap());
                    if code != 0 {
                        return Err(io::Error::from_raw_os_error(-code));
                    }
                }
                return Ok(true);
            }
            NLMSG_OVERRUN => {
                return Err(invalid("netlink receive overrun"));
            }
            NLMSG_ERROR => {
                if payload.len() < 4 {
                    return Err(invalid("short netlink error"));
                }
                let code = i32::from_ne_bytes(payload[0..4].try_into().unwrap());
                if code != 0 {
                    return Err(io::Error::from_raw_os_error(-code));
                }
            }
            SOCK_DIAG_BY_FAMILY => {
                if payload.len() < INET_DIAG_MSG_LEN {
                    return Err(invalid("short inet_diag_msg"));
                }

                let reported_family = payload[0] as u16;
                if reported_family != family {
                    return Err(invalid("unexpected socket family"));
                }
                if protocol == libc::IPPROTO_TCP as u16 && payload[1] != TCP_LISTEN as u8 {
                    return Err(invalid("unexpected TCP socket state"));
                }

                let port = u16::from_be_bytes(payload[4..6].try_into().unwrap());
                let cookie_low = u32::from_ne_bytes(payload[44..48].try_into().unwrap());
                let cookie_high = u32::from_ne_bytes(payload[48..52].try_into().unwrap());
                let socket_id = ((cookie_high as u64) << 32) | cookie_low as u64;
                let inode = u32::from_ne_bytes(payload[68..72].try_into().unwrap()) as u64;

                // UDP dumps include unbound sockets, which are outside this
                // monitor's state. Once a socket has a local port, malformed
                // identity is an error rather than a silently omitted entry.
                if port != 0 || protocol != libc::IPPROTO_UDP as u16 {
                    let socket = SnapshotSocket {
                        inode,
                        socket_id,
                        protocol,
                        family,
                        local_port: port,
                    };
                    validate_socket(&socket)?;
                    sockets.push(socket);
                }
            }
            _ => return Err(invalid("unexpected netlink message type")),
        }

        let next = aligned(length);
        if next > data.len() - offset {
            if length == data.len() - offset {
                break;
            }
            return Err(invalid("invalid netlink alignment"));
        }
        offset += next;
    }

    Ok(false)
}

fn collect_one(
    fd: &OwnedFd,
    family: u16,
    protocol: u16,
    sequence: u32,
) -> io::Result<Vec<SnapshotSocket>> {
    // nlmsghdr (16) + inet_diag_req_v2 (56)
    let mut request = [0u8; 72];

    request[0..4].copy_from_slice(&(72u32).to_ne_bytes());
    request[4..6].copy_from_slice(&SOCK_DIAG_BY_FAMILY.to_ne_bytes());
    request[6..8].copy_from_slice(&(NLM_F_REQUEST | NLM_F_DUMP).to_ne_bytes());
    request[8..12].copy_from_slice(&sequence.to_ne_bytes());

    request[16] = family as u8;
    request[17] = protocol as u8;

    let states = if protocol == libc::IPPROTO_TCP as u16 {
        1u32 << TCP_LISTEN
    } else {
        u32::MAX
    };
    request[20..24].copy_from_slice(&states.to_ne_bytes());

    let mut kernel: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
    kernel.nl_family = libc::AF_NETLINK as u16;

    let sent = unsafe {
        libc::sendto(
            fd.as_raw_fd(),
            request.as_ptr().cast(),
            request.len(),
            0,
            (&kernel as *const libc::sockaddr_nl).cast(),
            size_of::<libc::sockaddr_nl>() as libc::socklen_t,
        )
    };

    if sent < 0 {
        return Err(io::Error::last_os_error());
    }
    if sent as usize != request.len() {
        return Err(io::Error::new(
            io::ErrorKind::WriteZero,
            "short netlink request",
        ));
    }

    let mut sockets = Vec::new();
    let mut buffer = [0u8; 65536];

    loop {
        let mut sender: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
        let mut iov = libc::iovec {
            iov_base: buffer.as_mut_ptr().cast(),
            iov_len: buffer.len(),
        };
        let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };

        msg.msg_name = (&mut sender as *mut libc::sockaddr_nl).cast();
        msg.msg_namelen = size_of::<libc::sockaddr_nl>() as libc::socklen_t;
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;

        let received = unsafe { libc::recvmsg(fd.as_raw_fd(), &mut msg, 0) };

        if received < 0 {
            return Err(io::Error::last_os_error());
        }
        if received == 0 {
            return Err(invalid("netlink socket closed"));
        }
        if msg.msg_flags & libc::MSG_TRUNC != 0 {
            return Err(invalid("truncated netlink datagram"));
        }
        if msg.msg_namelen as usize != size_of::<libc::sockaddr_nl>()
            || sender.nl_pid != 0
            || sender.nl_family != libc::AF_NETLINK as u16
        {
            return Err(invalid("unexpected netlink sender"));
        }

        if parse_datagram(
            &buffer[..received as usize],
            sequence,
            protocol,
            family,
            &mut sockets,
        )? {
            return Ok(sockets);
        }
    }
}

pub fn collect() -> io::Result<Vec<SnapshotSocket>> {
    let raw_fd = unsafe {
        libc::socket(
            libc::AF_NETLINK,
            libc::SOCK_RAW | libc::SOCK_CLOEXEC,
            NETLINK_SOCK_DIAG,
        )
    };

    if raw_fd < 0 {
        return Err(io::Error::last_os_error());
    }

    let fd = unsafe { OwnedFd::from_raw_fd(raw_fd) };

    let timeout = libc::timeval {
        tv_sec: Duration::from_secs(3).as_secs() as libc::time_t,
        tv_usec: 0,
    };

    let result = unsafe {
        libc::setsockopt(
            fd.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_RCVTIMEO,
            (&timeout as *const libc::timeval).cast(),
            size_of::<libc::timeval>() as libc::socklen_t,
        )
    };

    if result < 0 {
        return Err(io::Error::last_os_error());
    }

    let mut address: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
    address.nl_family = libc::AF_NETLINK as u16;

    let result = unsafe {
        libc::bind(
            fd.as_raw_fd(),
            (&address as *const libc::sockaddr_nl).cast(),
            size_of::<libc::sockaddr_nl>() as libc::socklen_t,
        )
    };

    if result < 0 {
        return Err(io::Error::last_os_error());
    }

    let mut sockets = Vec::new();
    let mut sequence = 1;

    for family in [libc::AF_INET as u16, libc::AF_INET6 as u16] {
        for protocol in [libc::IPPROTO_TCP as u16, libc::IPPROTO_UDP as u16] {
            sockets.extend(collect_one(&fd, family, protocol, sequence)?);
            sequence += 1;
        }
    }

    validate(&sockets)?;
    Ok(sockets)
}

fn validate_socket(socket: &SnapshotSocket) -> io::Result<()> {
    if socket.socket_id == 0 || socket.socket_id == u64::MAX {
        return Err(invalid("invalid snapshot socket cookie"));
    }
    if socket.inode == 0 {
        return Err(invalid("invalid snapshot inode"));
    }
    if socket.local_port == 0 {
        return Err(invalid("invalid snapshot local port"));
    }
    if !matches!(socket.family, 2 | 10) {
        return Err(invalid("unsupported snapshot family"));
    }
    if !matches!(socket.protocol, 6 | 17) {
        return Err(invalid("unsupported snapshot protocol"));
    }
    Ok(())
}

pub fn validate(sockets: &[SnapshotSocket]) -> io::Result<()> {
    let mut cookies = std::collections::HashSet::with_capacity(sockets.len());
    for socket in sockets {
        validate_socket(socket)?;
        if !cookies.insert(socket.socket_id) {
            return Err(invalid("duplicate socket cookie in snapshot"));
        }
    }
    Ok(())
}

/// Compare two snapshots without depending on Netlink response order.
/// Equality does not prove that no socket changes occurred between dumps.
#[cfg(test)]
pub fn same_contents(a: &[SnapshotSocket], b: &[SnapshotSocket]) -> bool {
    if a.len() != b.len() {
        return false;
    }

    let mut left = a.to_vec();
    let mut right = b.to_vec();

    left.sort_unstable();
    right.sort_unstable();

    left == right
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_snapshots_ignore_netlink_order() {
        let tcp = SnapshotSocket {
            inode: 100,
            socket_id: 100,
            protocol: 6,
            family: 2,
            local_port: 3000,
        };

        let udp = SnapshotSocket {
            inode: 200,
            socket_id: 200,
            protocol: 17,
            family: 10,
            local_port: 5353,
        };

        assert!(same_contents(&[tcp, udp], &[udp, tcp]));
    }

    #[test]
    fn snapshot_comparison_detects_socket_changes() {
        let first = SnapshotSocket {
            inode: 100,
            socket_id: 100,
            protocol: 6,
            family: 2,
            local_port: 3000,
        };

        let second = SnapshotSocket {
            inode: 101,
            socket_id: 101,
            protocol: 6,
            family: 2,
            local_port: 3000,
        };

        assert!(!same_contents(&[first], &[second]));
        assert!(!same_contents(&[first], &[]));
    }

    #[test]
    fn snapshot_comparison_preserves_multiplicity() {
        let socket = SnapshotSocket {
            inode: 100,
            socket_id: 100,
            protocol: 6,
            family: 2,
            local_port: 3000,
        };

        assert!(!same_contents(&[socket, socket], &[socket]));
        assert!(same_contents(&[socket, socket], &[socket, socket]));
    }

    #[test]
    fn parses_valid_tcp_listener() {
        let mut message = vec![0u8; NLMSG_HDRLEN + INET_DIAG_MSG_LEN];
        let length = message.len() as u32;

        message[0..4].copy_from_slice(&length.to_ne_bytes());
        message[4..6].copy_from_slice(&SOCK_DIAG_BY_FAMILY.to_ne_bytes());
        message[8..12].copy_from_slice(&7u32.to_ne_bytes());

        message[16] = libc::AF_INET as u8;
        message[17] = TCP_LISTEN as u8;
        message[20..22].copy_from_slice(&3000u16.to_be_bytes());
        message[60..64].copy_from_slice(&67890u32.to_ne_bytes());
        message[84..88].copy_from_slice(&12345u32.to_ne_bytes());

        let mut sockets = Vec::new();
        let done = parse_datagram(
            &message,
            7,
            libc::IPPROTO_TCP as u16,
            libc::AF_INET as u16,
            &mut sockets,
        )
        .unwrap();

        assert!(!done);
        assert_eq!(
            sockets,
            vec![SnapshotSocket {
                inode: 12345,
                socket_id: 67890,
                protocol: 6,
                family: 2,
                local_port: 3000,
            }]
        );
    }

    #[test]
    fn collect_finds_live_tcp_udp_ipv4_ipv6() {
        use std::net::{TcpListener, UdpSocket};
        use std::os::fd::AsRawFd;

        fn inode(fd: libc::c_int) -> u64 {
            let mut stat: libc::stat = unsafe { std::mem::zeroed() };

            let result = unsafe { libc::fstat(fd, &mut stat) };
            assert_eq!(result, 0, "fstat failed");

            stat.st_ino
        }

        let tcp4 = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let udp4 = UdpSocket::bind(("127.0.0.1", 0)).unwrap();
        let tcp6 = TcpListener::bind(("::1", 0)).unwrap();
        let udp6 = UdpSocket::bind(("::1", 0)).unwrap();

        fn cookie(fd: libc::c_int) -> u64 {
            let mut value = 0u64;
            let mut length = std::mem::size_of::<u64>() as libc::socklen_t;

            let result = unsafe {
                libc::getsockopt(
                    fd,
                    libc::SOL_SOCKET,
                    57, // SO_COOKIE
                    (&mut value as *mut u64).cast(),
                    &mut length,
                )
            };

            assert_eq!(result, 0, "SO_COOKIE failed");
            assert_eq!(length as usize, std::mem::size_of::<u64>());
            value
        }

        let expected = [
            SnapshotSocket {
                inode: inode(tcp4.as_raw_fd()),
                socket_id: cookie(tcp4.as_raw_fd()),
                protocol: libc::IPPROTO_TCP as u16,
                family: libc::AF_INET as u16,
                local_port: tcp4.local_addr().unwrap().port(),
            },
            SnapshotSocket {
                inode: inode(udp4.as_raw_fd()),
                socket_id: cookie(udp4.as_raw_fd()),
                protocol: libc::IPPROTO_UDP as u16,
                family: libc::AF_INET as u16,
                local_port: udp4.local_addr().unwrap().port(),
            },
            SnapshotSocket {
                inode: inode(tcp6.as_raw_fd()),
                socket_id: cookie(tcp6.as_raw_fd()),
                protocol: libc::IPPROTO_TCP as u16,
                family: libc::AF_INET6 as u16,
                local_port: tcp6.local_addr().unwrap().port(),
            },
            SnapshotSocket {
                inode: inode(udp6.as_raw_fd()),
                socket_id: cookie(udp6.as_raw_fd()),
                protocol: libc::IPPROTO_UDP as u16,
                family: libc::AF_INET6 as u16,
                local_port: udp6.local_addr().unwrap().port(),
            },
        ];

        let observed = collect().expect("SOCK_DIAG snapshot failed");

        for socket in expected {
            assert!(observed.contains(&socket), "snapshot missing {socket:?}");
        }
    }

    #[test]
    fn rejects_wrong_sequence() {
        let mut message = vec![0u8; NLMSG_HDRLEN];
        message[0..4].copy_from_slice(&(NLMSG_HDRLEN as u32).to_ne_bytes());
        message[4..6].copy_from_slice(&NLMSG_DONE.to_ne_bytes());
        message[8..12].copy_from_slice(&99u32.to_ne_bytes());

        let mut sockets = Vec::new();
        assert!(parse_datagram(&message, 7, 6, 2, &mut sockets).is_err());
    }

    #[test]
    fn rejects_netlink_overrun() {
        let sequence = 42u32;
        let mut message = [0u8; 16];

        message[0..4].copy_from_slice(&16u32.to_ne_bytes());
        message[4..6].copy_from_slice(&NLMSG_OVERRUN.to_ne_bytes());
        message[8..12].copy_from_slice(&sequence.to_ne_bytes());

        let result = parse_datagram(
            &message,
            sequence,
            libc::IPPROTO_TCP as u16,
            libc::AF_INET as u16,
            &mut Vec::new(),
        );

        assert!(result.is_err());
    }

    #[test]
    fn rejects_interrupted_netlink_dump() {
        let sequence = 42u32;
        let mut message = [0u8; 16];

        message[0..4].copy_from_slice(&16u32.to_ne_bytes());
        message[4..6].copy_from_slice(&NLMSG_DONE.to_ne_bytes());
        message[6..8].copy_from_slice(&NLM_F_DUMP_INTR.to_ne_bytes());
        message[8..12].copy_from_slice(&sequence.to_ne_bytes());

        let result = parse_datagram(
            &message,
            sequence,
            libc::IPPROTO_TCP as u16,
            libc::AF_INET as u16,
            &mut Vec::new(),
        );

        assert!(result.is_err());
    }

    #[test]
    fn rejects_non_listening_tcp_socket() {
        let sequence = 42u32;
        let mut message = [0u8; NLMSG_HDRLEN + INET_DIAG_MSG_LEN];

        message[0..4].copy_from_slice(&((NLMSG_HDRLEN + INET_DIAG_MSG_LEN) as u32).to_ne_bytes());
        message[4..6].copy_from_slice(&SOCK_DIAG_BY_FAMILY.to_ne_bytes());
        message[8..12].copy_from_slice(&sequence.to_ne_bytes());

        message[16] = libc::AF_INET as u8;
        message[17] = 1; // TCP_ESTABLISHED, not TCP_LISTEN
        message[20..22].copy_from_slice(&3000u16.to_be_bytes());
        message[84..88].copy_from_slice(&12345u32.to_ne_bytes());

        let result = parse_datagram(
            &message,
            sequence,
            libc::IPPROTO_TCP as u16,
            libc::AF_INET as u16,
            &mut Vec::new(),
        );

        assert!(result.is_err());
    }

    #[test]
    fn rejects_truncated_netlink_message() {
        let sequence = 42u32;
        let mut message = [0u8; 16];

        message[0..4].copy_from_slice(&88u32.to_ne_bytes());
        message[4..6].copy_from_slice(&SOCK_DIAG_BY_FAMILY.to_ne_bytes());
        message[8..12].copy_from_slice(&sequence.to_ne_bytes());

        let result = parse_datagram(
            &message,
            sequence,
            libc::IPPROTO_TCP as u16,
            libc::AF_INET as u16,
            &mut Vec::new(),
        );

        assert!(result.is_err());
    }

    #[test]
    fn validation_rejects_duplicate_cookies() {
        let first = SnapshotSocket {
            inode: 1,
            socket_id: 10,
            protocol: 6,
            family: 2,
            local_port: 3000,
        };
        let second = SnapshotSocket {
            inode: 2,
            protocol: 17,
            family: 10,
            local_port: 5353,
            ..first
        };
        assert!(validate(&[first, second]).is_err());
    }

    #[test]
    fn validation_rejects_every_invalid_snapshot_field() {
        let valid = SnapshotSocket {
            inode: 1,
            socket_id: 10,
            protocol: 6,
            family: 2,
            local_port: 3000,
        };
        for invalid_socket in [
            SnapshotSocket {
                socket_id: 0,
                ..valid
            },
            SnapshotSocket {
                socket_id: u64::MAX,
                ..valid
            },
            SnapshotSocket { inode: 0, ..valid },
            SnapshotSocket {
                local_port: 0,
                ..valid
            },
            SnapshotSocket { family: 1, ..valid },
            SnapshotSocket {
                protocol: 1,
                ..valid
            },
        ] {
            assert!(
                validate(&[invalid_socket]).is_err(),
                "accepted {invalid_socket:?}"
            );
        }
    }
}
