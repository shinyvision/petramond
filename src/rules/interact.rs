pub const USE_REPEAT_TICKS: u32 = 5;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum ConsumerKind {
    Registered,
    Shear,
    BuiltinBlock,
    ContextualPlace,
    Eat,
    ItemUse,
    Place,
}

impl ConsumerKind {
    pub const CLAIM_ORDER: [ConsumerKind; 7] = [
        ConsumerKind::Registered,
        ConsumerKind::Shear,
        ConsumerKind::BuiltinBlock,
        ConsumerKind::ContextualPlace,
        ConsumerKind::Eat,
        ConsumerKind::ItemUse,
        ConsumerKind::Place,
    ];

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
        assert_eq!(
            order.first(),
            Some(&ConsumerKind::Registered),
            "mods claim first"
        );
        assert_eq!(
            order.last(),
            Some(&ConsumerKind::Place),
            "placement claims last"
        );
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
