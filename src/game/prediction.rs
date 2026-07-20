//! Client prediction ledger: pending request ids + undo snapshots.
//!
//! The server remains authoritative; this module
//! only tracks disposable local overlays until [`ActionOutcome`]s arrive.

use crate::inventory::Inventory;
use crate::mathh::IVec3;
use crate::net::protocol::{ActionDenyReason, ActionOutcome, ClientRequestId};

use super::replicated::MenuView;

/// Max in-flight predicted requests with snapshots; further local mutation
/// freezes until the server catches up. Ids still allocate so the wire stays
/// ordered.
pub(crate) const LEDGER_CAP: usize = 32;

/// What a pending request may need to restore on deny.
#[derive(Clone, Debug)]
pub(crate) enum PredictionSnapshot {
    /// No local mutation to undo (P0 presentation / track-only).
    None,
    /// Inventory-only prediction used by ordinary clicks and drops.
    Inventory(Inventory),
    /// One atomic menu transport prediction. Both halves must roll back
    /// together because a drag may span player inventory and an open block
    /// container.
    Menu {
        inventory: Inventory,
        menu: MenuView,
    },
    /// A predicted world mutation: optional pre-mutation inventory (place
    /// hotbar decrement) plus every replica cell written, with the previous
    /// block id. Multi-cell clears (door, model) list the full footprint.
    World {
        inventory: Option<Inventory>,
        cells: Vec<(IVec3, u8)>,
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
        !self.frozen && self.predicted_len() < LEDGER_CAP
    }

    /// In-flight entries holding a rollback snapshot. Track-only (`None`)
    /// entries never count against the cap — a burst of presentation-only
    /// jabs must not freeze real prediction.
    fn predicted_len(&self) -> usize {
        self.pending
            .iter()
            .filter(|p| !matches!(p.snapshot, PredictionSnapshot::None))
            .count()
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
        if self.predicted_len() >= LEDGER_CAP {
            self.frozen = true;
        }
        id
    }

    /// Allocate an id without a rollback snapshot (P0 presentation-only).
    pub(crate) fn begin_track_only(&mut self) -> ClientRequestId {
        self.begin(PredictionSnapshot::None)
    }

    /// Cells covered by pending World snapshots (and optionally by snapshots
    /// about to be reconciled). Used to suppress wire place/break presentation.
    pub(crate) fn predicted_cells(&self) -> impl Iterator<Item = IVec3> + '_ {
        self.pending.iter().flat_map(|p| match &p.snapshot {
            PredictionSnapshot::World { cells, .. } => {
                cells.iter().map(|(c, _)| *c).collect::<Vec<_>>()
            }
            _ => Vec::new(),
        })
    }

    /// Whether an inventory-mutating prediction stays pending past this
    /// batch's `outcomes` — the batch's `SelfState` inventory snapshot then
    /// predates that prediction and must not be adopted over it.
    pub(crate) fn awaits_inventory_authority(&self, outcomes: &[ActionOutcome]) -> bool {
        self.pending.iter().any(|p| {
            let holds_inventory = matches!(
                &p.snapshot,
                PredictionSnapshot::Inventory(_)
                    | PredictionSnapshot::Menu { .. }
                    | PredictionSnapshot::World {
                        inventory: Some(_),
                        ..
                    }
            );
            holds_inventory && !outcomes.iter().any(|o| o.id == p.id)
        })
    }

    /// Whether a menu-mirror-mutating prediction stays pending past this
    /// batch's `outcomes` — the batch's menu sync then predates it.
    pub(crate) fn awaits_menu_authority(&self, outcomes: &[ActionOutcome]) -> bool {
        self.pending.iter().any(|p| {
            matches!(&p.snapshot, PredictionSnapshot::Menu { .. })
                && !outcomes.iter().any(|o| o.id == p.id)
        })
    }

    /// Apply one batch of outcomes. Returns `(rollbacks, resolved_cells)`:
    /// deny snapshots to restore, and every World cell whose pending entry
    /// was answered (accept or deny) so presentation suppress can clear.
    ///
    /// Rollbacks come back OLDEST-FIRST by construction — the pending list is
    /// walked in allocation order, never the batch's emission order (the
    /// server may emit an immediate deny for a newer id before a tick-time
    /// deny for an older one). The caller applies them newest-first so the
    /// oldest snapshot wins.
    pub(crate) fn reconcile(
        &mut self,
        outcomes: &[ActionOutcome],
    ) -> (Vec<PredictionSnapshot>, Vec<IVec3>) {
        let mut rollbacks = Vec::new();
        let mut resolved_cells = Vec::new();
        let mut i = 0;
        while i < self.pending.len() {
            let Some(outcome) = outcomes.iter().find(|o| o.id == self.pending[i].id) else {
                i += 1;
                continue;
            };
            let pending = self.pending.remove(i);
            if let PredictionSnapshot::World { cells, .. } = &pending.snapshot {
                resolved_cells.extend(cells.iter().map(|(c, _)| *c));
            }
            if !outcome.accepted {
                rollbacks.push(pending.snapshot);
            }
        }
        if self.predicted_len() < LEDGER_CAP {
            self.frozen = false;
        }
        (rollbacks, resolved_cells)
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

/// The one deny-outcome constructor, shared by the server's message-time
/// denials and the ledger tests.
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
        assert!(rollbacks.0.is_empty());
        assert_eq!(ledger.pending_len(), 0);

        let id2 = ledger.begin(PredictionSnapshot::Inventory(inv));
        let rollbacks = ledger.reconcile(&[deny(id2, ActionDenyReason::Denied)]);
        assert_eq!(rollbacks.0.len(), 1);
    }

    #[test]
    fn track_only_entries_do_not_freeze_prediction() {
        let mut ledger = PredictionLedger::new();
        for _ in 0..LEDGER_CAP + 5 {
            ledger.begin_track_only();
        }
        assert!(
            ledger.can_predict(),
            "presentation-only jabs must not consume prediction capacity"
        );
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
