use petramond::net::protocol::{ActionOutcome, ClientRequestId};
use petramond_math::math::IVec3;
use petramond_world::inventory::Inventory;
use rustc_hash::FxHashSet;

use super::replicated::MenuView;

pub const LEDGER_CAP: usize = 32;

#[derive(Clone, Debug)]
pub enum PredictionSnapshot {
    None,
    Inventory(Inventory),
    Menu {
        inventory: Inventory,
        menu: MenuView,
    },
    World {
        inventory: Option<Inventory>,
        cells: Vec<(IVec3, u16)>,
    },
}

#[derive(Clone, Debug)]
struct Pending {
    id: ClientRequestId,
    snapshot: PredictionSnapshot,
}

#[derive(Default)]
pub struct PredictionLedger {
    next_id: ClientRequestId,
    pending: Vec<Pending>,
    frozen: bool,
    presented: FxHashSet<IVec3>,
}

impl PredictionLedger {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn can_predict(&self) -> bool {
        !self.frozen && self.predicted_len() < LEDGER_CAP
    }

    fn predicted_len(&self) -> usize {
        self.pending
            .iter()
            .filter(|p| !matches!(p.snapshot, PredictionSnapshot::None))
            .count()
    }

    pub fn begin(&mut self, snapshot: PredictionSnapshot) -> ClientRequestId {
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

    pub fn begin_track_only(&mut self) -> ClientRequestId {
        self.begin(PredictionSnapshot::None)
    }

    pub(super) fn predicted_cells(&self) -> impl Iterator<Item = IVec3> + '_ {
        self.pending.iter().flat_map(|p| match &p.snapshot {
            PredictionSnapshot::World { cells, .. } => {
                cells.iter().map(|(c, _)| *c).collect::<Vec<_>>()
            }
            _ => Vec::new(),
        })
    }

    pub fn mark_presented(&mut self, cell: IVec3) {
        self.presented.insert(cell);
    }

    pub fn suppressed_cells(&self) -> impl Iterator<Item = IVec3> + '_ {
        self.predicted_cells().chain(self.presented.iter().copied())
    }

    #[cfg(test)]
    pub fn clear_presented_for_test(&mut self) {
        self.presented.clear();
    }

    pub fn awaits_inventory_authority(&self, outcomes: &[ActionOutcome]) -> bool {
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

    pub fn awaits_menu_authority(&self, outcomes: &[ActionOutcome]) -> bool {
        self.pending.iter().any(|p| {
            matches!(&p.snapshot, PredictionSnapshot::Menu { .. })
                && !outcomes.iter().any(|o| o.id == p.id)
        })
    }

    pub fn reconcile(&mut self, outcomes: &[ActionOutcome]) -> Vec<PredictionSnapshot> {
        let mut rollbacks = Vec::new();
        let mut i = 0;
        while i < self.pending.len() {
            let Some(outcome) = outcomes.iter().find(|o| o.id == self.pending[i].id) else {
                i += 1;
                continue;
            };
            let pending = self.pending.remove(i);
            if let PredictionSnapshot::World { cells, .. } = &pending.snapshot {
                for (c, _) in cells {
                    self.presented.remove(c);
                }
            }
            if !outcome.accepted {
                rollbacks.push(pending.snapshot);
            }
        }
        if self.predicted_len() < LEDGER_CAP {
            self.frozen = false;
        }
        rollbacks
    }

    #[cfg(test)]
    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    #[cfg(test)]
    pub fn is_frozen(&self) -> bool {
        self.frozen
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
        let rollbacks = ledger.reconcile(&[ActionOutcome::accept(id)]);
        assert!(rollbacks.is_empty());
        assert_eq!(ledger.pending_len(), 0);

        let id2 = ledger.begin(PredictionSnapshot::Inventory(inv));
        let rollbacks = ledger.reconcile(&[ActionOutcome::deny(
            id2,
            petramond::net::protocol::ActionDenyReason::Denied,
        )]);
        assert_eq!(rollbacks.len(), 1);
    }

    #[test]
    fn presented_cells_stay_suppressed_until_their_outcome_lands() {
        let mut ledger = PredictionLedger::new();
        let cell = IVec3::new(1, 64, 1);
        let id = ledger.begin(PredictionSnapshot::World {
            inventory: None,
            cells: vec![(cell, 0)],
        });
        ledger.mark_presented(cell);
        ledger.mark_presented(IVec3::new(9, 64, 9));
        assert!(ledger.suppressed_cells().any(|c| c == cell));
        ledger.reconcile(&[ActionOutcome::accept(id)]);
        let left: Vec<_> = ledger.suppressed_cells().collect();
        assert_eq!(
            left,
            vec![IVec3::new(9, 64, 9)],
            "the answered cell clears; an unanswered presented cell stays"
        );
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
