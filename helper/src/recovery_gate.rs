//! Conservative state gate for lifecycle event publication.
//! Recovery cannot be completed until snapshot reconciliation is integrated.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryStatus {
    Healthy,
    ResyncNeeded,
    Recovering,
    Failed,
}

#[derive(Debug)]
pub struct RecoveryGate {
    status: RecoveryStatus,
}

impl RecoveryGate {
    pub fn new() -> Self {
        Self {
            status: RecoveryStatus::Healthy,
        }
    }

    #[cfg(test)]
    pub fn status(&self) -> RecoveryStatus {
        self.status
    }

    pub fn mark_loss(&mut self) -> bool {
        if self.status != RecoveryStatus::Healthy {
            return false;
        }

        self.status = RecoveryStatus::ResyncNeeded;
        true
    }

    pub fn can_publish_lifecycle(&self) -> bool {
        self.status == RecoveryStatus::Healthy
    }

    pub fn begin_recovery(&mut self) -> bool {
        if !matches!(
            self.status,
            RecoveryStatus::ResyncNeeded | RecoveryStatus::Failed
        ) {
            return false;
        }
        self.status = RecoveryStatus::Recovering;
        true
    }

    pub fn fail_recovery(&mut self) {
        debug_assert_eq!(self.status, RecoveryStatus::Recovering);
        self.status = RecoveryStatus::Failed;
    }

    /// Call only after a complete, validated snapshot reconciliation.
    /// This method does not perform reconciliation itself.
    pub fn complete_recovery(&mut self) {
        debug_assert_eq!(self.status, RecoveryStatus::Recovering);
        self.status = RecoveryStatus::Healthy;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_healthy() {
        let gate = RecoveryGate::new();

        assert_eq!(gate.status(), RecoveryStatus::Healthy);
        assert!(gate.can_publish_lifecycle());
    }

    #[test]
    fn loss_blocks_lifecycle_publication() {
        let mut gate = RecoveryGate::new();

        gate.mark_loss();

        assert_eq!(gate.status(), RecoveryStatus::ResyncNeeded);
        assert!(!gate.can_publish_lifecycle());
    }

    #[test]
    fn repeated_loss_remains_blocked() {
        let mut gate = RecoveryGate::new();

        gate.mark_loss();
        gate.mark_loss();

        assert_eq!(gate.status(), RecoveryStatus::ResyncNeeded);
        assert!(!gate.can_publish_lifecycle());
    }

    #[test]
    fn reports_only_first_transition_to_resync() {
        let mut gate = RecoveryGate::new();

        assert!(gate.mark_loss());
        assert!(!gate.mark_loss());
        assert!(!gate.mark_loss());

        assert_eq!(gate.status(), RecoveryStatus::ResyncNeeded);
        assert!(!gate.can_publish_lifecycle());
    }

    #[test]
    fn explicit_recovery_restores_publication() {
        let mut gate = RecoveryGate::new();

        assert!(gate.mark_loss());
        assert!(!gate.can_publish_lifecycle());

        assert!(gate.begin_recovery());
        gate.complete_recovery();

        assert_eq!(gate.status(), RecoveryStatus::Healthy);
        assert!(gate.can_publish_lifecycle());

        // A later loss must block publication again.
        assert!(gate.mark_loss());
        assert!(!gate.can_publish_lifecycle());
    }

    #[test]
    fn no_implicit_recovery() {
        let mut gate = RecoveryGate::new();

        gate.mark_loss();

        for _ in 0..100 {
            assert!(!gate.can_publish_lifecycle());
        }

        assert_eq!(gate.status(), RecoveryStatus::ResyncNeeded);
    }

    #[test]
    fn failed_recovery_stays_blocked_and_can_be_retried() {
        let mut gate = RecoveryGate::new();
        gate.mark_loss();
        assert!(gate.begin_recovery());
        assert_eq!(gate.status(), RecoveryStatus::Recovering);
        assert!(!gate.can_publish_lifecycle());

        gate.fail_recovery();
        assert_eq!(gate.status(), RecoveryStatus::Failed);
        assert!(!gate.can_publish_lifecycle());
        assert!(gate.begin_recovery());
        gate.complete_recovery();
        assert!(gate.can_publish_lifecycle());
    }
}
