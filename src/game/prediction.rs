//! Client prediction ledger: pending request ids + undo snapshots.
//!
//! See WIKI/client-prediction.md. The server remains authoritative; this module
//! only tracks disposable local overlays until [`ActionOutcome`]s arrive.

use crate::inventory::Inventory;
use crate::mathh::IVec3;
use crate::net::protocol::{ActionDenyReason, ActionOutcome, ClientRequestId};

/// Max in-flight predicted requests with snapshots; further local mutation
/// freezes until the server catches up. Ids still allocate so the wire stays
/// ordered.
pub(crate) const LEDGER_CAP: usize = 32;

/// What a pending request may need to restore on deny.
#[derive(Clone, Debug)]
pub(crate) enum PredictionSnapshot {
    /// No local mutation to undo (P0 presentation / track-only).
    None,
    /// Inventory (+ optional craft/chest/furnace/workbench/container views are
    /// restored by re-applying the last authoritative menu sync — inventory is
    /// the primary rollback target for P1).
    Inventory(Inventory),
    /// A locally cleared (or ghost-placed) cell.
    Cell {
        pos: IVec3,
        /// Previous block id at the cell (air = 0), for restore.
        prev_block_id: u8,
    },
}

#[derive(Clone, Debug)]
struct Pending {
    id: ClientRequestId,
    snapshot: PredictionSnapshot,
}

#[derive(Default)]
pub(crate) struct PredictionLedger {
    next_id: ClientRequestId,
    pending: Vec<Pending>,
    /// When true, new local mutations are refused until the queue drains.
    frozen: bool,
}

impl PredictionLedger {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn can_predict(&self) -> bool {
        !self.frozen && self.pending.len() < LEDGER_CAP
    }

    /// Always allocate a request id. When `snapshot` is not [`None`] and the
    /// ledger is at capacity, the snapshot is dropped (track-only) and
    /// `can_predict` stays false.
    pub(crate) fn begin(&mut self, snapshot: PredictionSnapshot) -> ClientRequestId {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        let snapshot = if matches!(snapshot, PredictionSnapshot::None) || self.can_predict() {
            snapshot
        } else {
            self.frozen = true;
            PredictionSnapshot::None
        };
        self.pending.push(Pending { id, snapshot });
        if self
            .pending
            .iter()
            .filter(|p| !matches!(p.snapshot, PredictionSnapshot::None))
            .count()
            >= LEDGER_CAP
        {
            self.frozen = true;
        }
        id
    }

    /// Allocate an id without a rollback snapshot (P0 presentation-only).
    pub(crate) fn begin_track_only(&mut self) -> ClientRequestId {
        self.begin(PredictionSnapshot::None)
    }

    /// Apply one batch of outcomes in order. Returns snapshots that must be
    /// restored (denies only).
    pub(crate) fn reconcile(&mut self, outcomes: &[ActionOutcome]) -> Vec<PredictionSnapshot> {
        let mut rollbacks = Vec::new();
        for outcome in outcomes {
            if let Some(idx) = self.pending.iter().position(|p| p.id == outcome.id) {
                let pending = self.pending.remove(idx);
                if !outcome.accepted {
                    rollbacks.push(pending.snapshot);
                }
            }
        }
        let predicted = self
            .pending
            .iter()
            .filter(|p| !matches!(p.snapshot, PredictionSnapshot::None))
            .count();
        if predicted < LEDGER_CAP {
            self.frozen = false;
        }
        rollbacks
    }

    #[cfg(test)]
    pub(crate) fn pending_len(&self) -> usize {
        self.pending.len()
    }

    #[cfg(test)]
    pub(crate) fn is_frozen(&self) -> bool {
        self.frozen
    }
}

/// Helper for tests / callers constructing deny outcomes.
#[allow(dead_code)]
pub(crate) fn deny(id: ClientRequestId, reason: ActionDenyReason) -> ActionOutcome {
    ActionOutcome {
        id,
        accepted: false,
        reason: Some(reason),
    }
}

#[allow(dead_code)]
pub(crate) fn accept(id: ClientRequestId) -> ActionOutcome {
    ActionOutcome {
        id,
        accepted: true,
        reason: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accept_drops_pending_deny_returns_snapshot() {
        let mut ledger = PredictionLedger::new();
        let inv = Inventory::new();
        let id = ledger.begin(PredictionSnapshot::Inventory(inv.clone()));
        assert_eq!(ledger.pending_len(), 1);
        let rollbacks = ledger.reconcile(&[accept(id)]);
        assert!(rollbacks.is_empty());
        assert_eq!(ledger.pending_len(), 0);

        let id2 = ledger.begin(PredictionSnapshot::Inventory(inv));
        let rollbacks = ledger.reconcile(&[deny(id2, ActionDenyReason::Denied)]);
        assert_eq!(rollbacks.len(), 1);
    }

    #[test]
    fn freezes_local_mutation_at_cap_but_still_allocates_ids() {
        let mut ledger = PredictionLedger::new();
        for _ in 0..LEDGER_CAP {
            ledger.begin(PredictionSnapshot::Inventory(Inventory::new()));
        }
        assert!(ledger.is_frozen());
        let id = ledger.begin(PredictionSnapshot::Inventory(Inventory::new()));
        assert!(matches!(
            ledger.pending.last().map(|p| &p.snapshot),
            Some(PredictionSnapshot::None)
        ));
        let _ = id;
    }
}
