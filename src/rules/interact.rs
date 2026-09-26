//! The interact consumer registry's shared facts: which consumers exist, the
//! order they are offered a use click in, and what each one's claim means for
//! presentation.
//!
//! The server's dispatch (`server::interact`) pairs every kind with its claim
//! function and walks them in [`ConsumerKind::CLAIM_ORDER`]; the client's
//! prediction mirror names the step it predicted by the same kind and reads
//! its facts here, so neither side keeps a copy of the other's table.

/// Which registry row a consumer is.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum ConsumerKind {
    /// Registered `interact_attempt` handlers (mods).
    Registered,
    /// Engine mob use: shears on a shearable mob.
    Shear,
    /// The block's built-in capability (GUI open, door, bed).
    BuiltinBlock,
    /// A dual-natured held item (food AND placeable) trying its placement
    /// before the eat gate.
    ContextualPlace,
    /// Eating the held food.
    Eat,
    /// The held item's own use (`item_use_pre`, then the engine buckets).
    ItemUse,
    /// Ordinary placement of the held block.
    Place,
}

impl ConsumerKind {
    /// Every consumer, in claim order: the first claim wins and nothing later
    /// runs. Mods first (a handler's Cancel is a claim), then engine mob use,
    /// the clicked block's built-in capability, a dual-natured item's
    /// placement (a VALID placement wins over starting to eat), eating, the
    /// held item's own use, and finally ordinary placement.
    pub const CLAIM_ORDER: [ConsumerKind; 7] = [
        ConsumerKind::Registered,
        ConsumerKind::Shear,
        ConsumerKind::BuiltinBlock,
        ConsumerKind::ContextualPlace,
        ConsumerKind::Eat,
        ConsumerKind::ItemUse,
        ConsumerKind::Place,
    ];

    /// The consumer's own gesture IS its presentation — an eat's raise — so
    /// a claim by it plays no hand jab on either mirror.
    pub const fn presents_itself(self) -> bool {
        matches!(self, ConsumerKind::Eat)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claim_order_lists_every_kind_once() {
        let order = ConsumerKind::CLAIM_ORDER;
        for (i, kind) in order.iter().enumerate() {
            assert!(
                !order[..i].contains(kind),
                "{kind:?} appears twice in the claim order"
            );
        }
        assert_eq!(order.first(), Some(&ConsumerKind::Registered), "mods claim first");
        assert_eq!(order.last(), Some(&ConsumerKind::Place), "placement claims last");
    }

    #[test]
    fn only_eating_presents_itself() {
        let presenting: Vec<_> = ConsumerKind::CLAIM_ORDER
            .into_iter()
            .filter(|k| k.presents_itself())
            .collect();
        assert_eq!(presenting, vec![ConsumerKind::Eat]);
    }
}
