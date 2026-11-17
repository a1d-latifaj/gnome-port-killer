use crate::event::SocketEvent;

#[derive(Debug, Default)]
pub struct EventBatch {
    events: Vec<SocketEvent>,
    invalid: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum BatchResult {
    Accepted(usize),
    ResyncRequired,
}

impl EventBatch {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, event: SocketEvent) {
        if !event.is_valid() {
            self.invalid = true;
            return;
        }

        self.events.push(event);
    }

    pub fn invalidate(&mut self) {
        self.invalid = true;
    }

    #[cfg(test)]
    pub fn finish(
        self,
        lost_before: u64,
        lost_after: u64,
        on_event: impl FnMut(SocketEvent),
    ) -> BatchResult {
        self.finish_at(lost_before, lost_after, u64::MAX, on_event)
    }

    pub fn finish_at(
        mut self,
        lost_before: u64,
        lost_after: u64,
        latest_timestamp_ns: u64,
        mut on_event: impl FnMut(SocketEvent),
    ) -> BatchResult {
        if self
            .events
            .iter()
            .any(|event| event.timestamp_ns > latest_timestamp_ns)
        {
            self.invalid = true;
        }
        if self.invalid || lost_before != lost_after {
            return BatchResult::ResyncRequired;
        }

        // Ring-buffer reservation order across CPUs need not be timestamp
        // order. Stable sorting gives state application an explicit order.
        self.events.sort_by_key(|event| event.timestamp_ns);
        let count = self.events.len();

        for event in self.events {
            on_event(event);
        }

        BatchResult::Accepted(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{EventType, EVENT_VERSION};

    fn event(id: u64) -> SocketEvent {
        SocketEvent {
            socket_id: id,
            timestamp_ns: 100,
            pid: 4242,
            event_type: EventType::TcpListen as u16,
            family: 2,
            protocol: 6,
            local_port: 3000,
            version: EVENT_VERSION,
            reserved: 0,
        }
    }

    #[test]
    fn valid_batch_is_delivered_in_order() {
        let mut batch = EventBatch::new();
        batch.push(event(1));
        batch.push(event(2));

        let mut delivered = Vec::new();

        assert_eq!(
            batch.finish(0, 0, |e| delivered.push(e.socket_id)),
            BatchResult::Accepted(2)
        );

        assert_eq!(delivered, vec![1, 2]);
    }

    #[test]
    fn loss_discards_entire_batch() {
        let mut batch = EventBatch::new();
        batch.push(event(1));
        batch.push(event(2));

        let mut delivered = Vec::new();

        assert_eq!(
            batch.finish(0, 1, |e| delivered.push(e.socket_id)),
            BatchResult::ResyncRequired
        );

        assert!(delivered.is_empty());
    }

    #[test]
    fn invalid_event_discards_entire_batch() {
        let mut batch = EventBatch::new();
        batch.push(event(1));

        let mut invalid = event(2);
        invalid.version = 999;

        batch.push(invalid);

        let mut delivered = Vec::new();

        assert_eq!(
            batch.finish(0, 0, |e| delivered.push(e.socket_id)),
            BatchResult::ResyncRequired
        );

        assert!(delivered.is_empty());
    }

    #[test]
    fn explicit_invalidation_discards_batch() {
        let mut batch = EventBatch::new();
        batch.push(event(1));
        batch.invalidate();

        let mut delivered = Vec::new();

        assert_eq!(
            batch.finish(0, 0, |e| delivered.push(e.socket_id)),
            BatchResult::ResyncRequired
        );

        assert!(delivered.is_empty());
    }

    #[test]
    fn empty_batch_is_accepted() {
        let batch = EventBatch::new();

        assert_eq!(
            batch.finish(0, 0, |_| panic!("Unexpected event")),
            BatchResult::Accepted(0)
        );
    }

    #[test]
    fn events_are_normalized_by_timestamp() {
        let mut batch = EventBatch::new();
        let mut later = event(1);
        later.timestamp_ns = 200;
        let mut earlier = event(2);
        earlier.timestamp_ns = 100;
        batch.push(later);
        batch.push(earlier);

        let mut delivered = Vec::new();
        assert_eq!(
            batch.finish(7, 7, |event| delivered.push(event.socket_id)),
            BatchResult::Accepted(2)
        );
        assert_eq!(delivered, vec![2, 1]);
    }

    #[test]
    fn zero_timestamp_invalidates_entire_batch() {
        let mut batch = EventBatch::new();
        batch.push(event(1));
        let mut invalid = event(2);
        invalid.timestamp_ns = 0;
        batch.push(invalid);

        let mut delivered = Vec::new();
        assert_eq!(
            batch.finish(7, 7, |event| delivered.push(event.socket_id)),
            BatchResult::ResyncRequired
        );
        assert!(delivered.is_empty());
    }

    #[test]
    fn future_timestamp_invalidates_entire_batch() {
        let mut batch = EventBatch::new();
        batch.push(event(1));
        let mut future = event(2);
        future.timestamp_ns = 201;
        batch.push(future);

        let mut delivered = Vec::new();
        assert_eq!(
            batch.finish_at(7, 7, 200, |event| delivered.push(event.socket_id)),
            BatchResult::ResyncRequired
        );
        assert!(delivered.is_empty());
    }
}
