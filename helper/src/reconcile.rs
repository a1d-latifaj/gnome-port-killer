//! Deterministic classification of lifecycle events around a Netlink snapshot.
//! This module does not claim that SOCK_DIAG dumps are atomic.

use crate::event::SocketEvent;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventPosition {
    Before,
    During,
    After,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotWindow {
    pub start_ns: u64,
    pub end_ns: u64,
}

impl SnapshotWindow {
    pub fn new(start_ns: u64, end_ns: u64) -> Option<Self> {
        if start_ns == 0 || end_ns < start_ns {
            return None;
        }

        Some(Self { start_ns, end_ns })
    }

    pub fn classify(&self, event: &SocketEvent) -> EventPosition {
        if event.timestamp_ns < self.start_ns {
            EventPosition::Before
        } else if event.timestamp_ns <= self.end_ns {
            EventPosition::During
        } else {
            EventPosition::After
        }
    }
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconcileDecision {
    CoveredBySnapshot,
    Ambiguous,
    ReplayAfterSnapshot,
}

impl SnapshotWindow {
    #[cfg(test)]
    pub fn decide(&self, event: &SocketEvent) -> ReconcileDecision {
        match self.classify(event) {
            EventPosition::Before => ReconcileDecision::CoveredBySnapshot,
            EventPosition::During => ReconcileDecision::Ambiguous,
            EventPosition::After => ReconcileDecision::ReplayAfterSnapshot,
        }
    }
}

/// A snapshot is safe to adopt only when no lifecycle event
/// overlaps its collection window. Events after the window must
/// be replayed in timestamp order by the caller.
pub fn snapshot_has_overlap(window: &SnapshotWindow, events: &[SocketEvent]) -> bool {
    events
        .iter()
        .any(|event| window.classify(event) == EventPosition::During)
}

/// Rebuild state from a validated SOCK_DIAG snapshot and buffered events.
///
/// The caller must independently verify that:
/// - all lifecycle events through the snapshot boundary were drained;
/// - the kernel loss counter did not change;
/// - no event was discarded before reaching this function.
pub fn reconcile_snapshot(
    snapshot: &[crate::snapshot::SnapshotSocket],
    events: &[SocketEvent],
    window: &SnapshotWindow,
) -> Result<crate::state::SocketState, &'static str> {
    if snapshot_has_overlap(window, events) {
        return Err("lifecycle event overlaps snapshot");
    }

    if events
        .iter()
        .any(|event| !event.is_valid() || event.timestamp_ns == 0)
    {
        return Err("invalid lifecycle event");
    }

    crate::snapshot::validate(snapshot).map_err(|error| match error.to_string().as_str() {
        "duplicate socket cookie in snapshot" => "duplicate socket cookie in snapshot",
        _ => "invalid snapshot socket",
    })?;

    let mut state = crate::state::SocketState::new();

    state.replace_snapshot(
        snapshot
            .iter()
            .copied()
            .map(crate::state::SocketInfo::from_snapshot),
    );

    let mut replay: Vec<SocketEvent> = events
        .iter()
        .copied()
        .filter(|event| window.classify(event) == EventPosition::After)
        .collect();

    replay.sort_by_key(|event| event.timestamp_ns);

    for event in replay {
        match state.apply(event) {
            crate::state::StateChange::Added(_)
            | crate::state::StateChange::Removed(_)
            | crate::state::StateChange::Updated(_)
            | crate::state::StateChange::Unchanged => {}
            crate::state::StateChange::Invalid | crate::state::StateChange::ResyncRequired => {
                return Err("lifecycle replay failed");
            }
        }
    }

    Ok(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{EventType, SocketEvent, EVENT_VERSION};

    fn event(timestamp_ns: u64) -> SocketEvent {
        SocketEvent {
            socket_id: 500,
            timestamp_ns,
            pid: 4242,
            event_type: EventType::TcpClose as u16,
            family: 2,
            protocol: 6,
            local_port: 3000,
            version: EVENT_VERSION,
            reserved: 0,
        }
    }

    #[test]
    fn detects_lifecycle_overlap_during_snapshot() {
        let window = SnapshotWindow::new(100, 200).unwrap();

        assert!(!snapshot_has_overlap(&window, &[event(99), event(201)]));

        assert!(snapshot_has_overlap(
            &window,
            &[event(99), event(150), event(201)]
        ));

        assert!(snapshot_has_overlap(&window, &[event(100)]));

        assert!(snapshot_has_overlap(&window, &[event(200)]));
    }

    #[test]
    fn reconciliation_rejects_duplicate_cookie() {
        let window = SnapshotWindow::new(100, 200).unwrap();

        let first = crate::snapshot::SnapshotSocket {
            inode: 123,
            socket_id: 500,
            protocol: 6,
            family: 2,
            local_port: 3000,
        };

        let second = crate::snapshot::SnapshotSocket {
            inode: 456,
            socket_id: 500,
            protocol: 17,
            family: 10,
            local_port: 5353,
        };

        let result = reconcile_snapshot(&[first, second], &[], &window);

        assert!(matches!(result, Err("duplicate socket cookie in snapshot")));
    }

    #[test]
    fn reconciliation_rejects_invalid_snapshot_socket() {
        let window = SnapshotWindow::new(100, 200).unwrap();

        let invalid = crate::snapshot::SnapshotSocket {
            inode: 123,
            socket_id: 0,
            protocol: 6,
            family: 2,
            local_port: 3000,
        };

        let result = reconcile_snapshot(&[invalid], &[], &window);

        assert!(matches!(result, Err("invalid snapshot socket")));
    }

    #[test]
    fn reconciliation_accepts_clean_snapshot() {
        let window = SnapshotWindow::new(100, 200).unwrap();

        let snapshot = [crate::snapshot::SnapshotSocket {
            inode: 123,
            socket_id: 500,
            protocol: 6,
            family: 2,
            local_port: 3000,
        }];

        let state = reconcile_snapshot(&snapshot, &[], &window).unwrap();

        assert_eq!(state.len(), 1);
        assert_eq!(state.get(500).unwrap().pid, 0);
        assert!(!state.needs_resync());
    }

    #[test]
    fn reconciliation_replays_close_after_snapshot() {
        let window = SnapshotWindow::new(100, 200).unwrap();

        let snapshot = [crate::snapshot::SnapshotSocket {
            inode: 123,
            socket_id: 500,
            protocol: 6,
            family: 2,
            local_port: 3000,
        }];

        let state = reconcile_snapshot(&snapshot, &[event(201)], &window).unwrap();

        assert!(state.is_empty());
        assert!(!state.needs_resync());
    }

    #[test]
    fn reconciliation_rejects_overlapping_event() {
        let window = SnapshotWindow::new(100, 200).unwrap();

        let result = reconcile_snapshot(&[], &[event(150)], &window);

        assert!(result.is_err());
    }

    #[test]
    fn reconciliation_ignores_unknown_close_after_snapshot() {
        let window = SnapshotWindow::new(100, 200).unwrap();

        let state = reconcile_snapshot(&[], &[event(201)], &window).unwrap();
        assert!(state.is_empty());
        assert!(!state.needs_resync());
    }

    #[test]
    fn rejects_invalid_snapshot_window() {
        assert!(SnapshotWindow::new(0, 100).is_none());
        assert!(SnapshotWindow::new(200, 100).is_none());
    }

    #[test]
    fn classifies_event_before_snapshot() {
        let window = SnapshotWindow::new(100, 200).unwrap();

        assert_eq!(window.classify(&event(99)), EventPosition::Before);
    }

    #[test]
    fn classifies_event_during_snapshot() {
        let window = SnapshotWindow::new(100, 200).unwrap();

        for timestamp in [100, 150, 200] {
            assert_eq!(window.classify(&event(timestamp)), EventPosition::During);
        }
    }

    #[test]
    fn classifies_event_after_snapshot() {
        let window = SnapshotWindow::new(100, 200).unwrap();

        assert_eq!(window.classify(&event(201)), EventPosition::After);
    }

    #[test]
    fn snapshot_overlap_is_ambiguous() {
        let window = SnapshotWindow::new(100, 200).unwrap();

        for timestamp in [100, 150, 200] {
            assert_eq!(
                window.decide(&event(timestamp)),
                ReconcileDecision::Ambiguous
            );
        }
    }

    #[test]
    fn events_outside_snapshot_have_explicit_decisions() {
        let window = SnapshotWindow::new(100, 200).unwrap();

        assert_eq!(
            window.decide(&event(99)),
            ReconcileDecision::CoveredBySnapshot
        );

        assert_eq!(
            window.decide(&event(201)),
            ReconcileDecision::ReplayAfterSnapshot
        );
    }

    #[test]
    fn listen_and_close_during_snapshot_are_both_ambiguous() {
        let window = SnapshotWindow::new(100, 200).unwrap();

        let close = event(150);

        let listen = SocketEvent {
            event_type: EventType::TcpListen as u16,
            timestamp_ns: 120,
            ..close
        };

        assert_eq!(window.decide(&listen), ReconcileDecision::Ambiguous);

        assert_eq!(window.decide(&close), ReconcileDecision::Ambiguous);
    }
}
