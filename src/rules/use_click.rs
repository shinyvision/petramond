use super::interact::ConsumerKind;
use petramond_math::math::IVec3;
use petramond_world::inventory::Hand;
use petramond_world::world::WorldData;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Claim<P> {
    Pass,
    Claimed,
    Placed(P),
}

pub trait UseClickConsumers {
    type Placement;

    fn off_hand_occupied(&self) -> bool;

    fn set_acting_hand(&mut self, hand: Hand);

    fn offer(&mut self, kind: ConsumerKind) -> Claim<Self::Placement>;
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct UseClickOutcome<P> {
    pub claimant: Option<ConsumerKind>,
    pub hand: Hand,
    pub placement: Option<P>,
}

impl<P> UseClickOutcome<P> {
    pub fn consumed(&self) -> bool {
        self.claimant.is_some()
    }

    pub fn off_hand_acted(&self) -> bool {
        self.consumed() && self.hand == Hand::Off
    }

    pub fn presents_itself(&self) -> bool {
        self.claimant.is_some_and(ConsumerKind::presents_itself)
    }
}

pub fn walk_consumers<C: UseClickConsumers + ?Sized>(
    consumers: &mut C,
) -> Option<(ConsumerKind, Claim<C::Placement>)> {
    ConsumerKind::CLAIM_ORDER
        .into_iter()
        .find_map(|kind| match consumers.offer(kind) {
            Claim::Pass => None,
            claim => Some((kind, claim)),
        })
}

pub fn run_use_click<C: UseClickConsumers + ?Sized>(
    consumers: &mut C,
) -> UseClickOutcome<C::Placement> {
    let mut outcome = UseClickOutcome {
        claimant: None,
        hand: Hand::Main,
        placement: None,
    };
    for hand in [Hand::Main, Hand::Off] {
        if hand == Hand::Off && !consumers.off_hand_occupied() {
            break;
        }
        consumers.set_acting_hand(hand);
        if let Some((kind, claim)) = walk_consumers(consumers) {
            outcome = UseClickOutcome {
                claimant: Some(kind),
                hand,
                placement: match claim {
                    Claim::Placed(p) => Some(p),
                    Claim::Pass | Claim::Claimed => None,
                },
            };
            break;
        }
    }
    consumers.set_acting_hand(Hand::Main);
    outcome
}

pub fn registered_offered(block: Option<IVec3>, mob: Option<u64>) -> bool {
    block.is_some() || mob.is_some()
}

pub fn builtin_claims_at(world: &WorldData, pos: IVec3, sneaking: bool) -> bool {
    let block = petramond_world::block::Block::from_id(world.chunk_block(pos.x, pos.y, pos.z));
    petramond_world::block::builtin_claims_click(block, sneaking)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scripted {
        claims: Vec<(Hand, ConsumerKind, Claim<u8>)>,
        off_hand: bool,
        acting: Hand,
        offered: Vec<(Hand, ConsumerKind)>,
    }

    impl Scripted {
        fn new(off_hand: bool, claims: Vec<(Hand, ConsumerKind, Claim<u8>)>) -> Self {
            Self {
                claims,
                off_hand,
                acting: Hand::Main,
                offered: Vec::new(),
            }
        }
    }

    impl UseClickConsumers for Scripted {
        type Placement = u8;

        fn off_hand_occupied(&self) -> bool {
            self.off_hand
        }

        fn set_acting_hand(&mut self, hand: Hand) {
            self.acting = hand;
        }

        fn offer(&mut self, kind: ConsumerKind) -> Claim<u8> {
            self.offered.push((self.acting, kind));
            self.claims
                .iter()
                .find(|&&(h, k, _)| h == self.acting && k == kind)
                .map_or(Claim::Pass, |&(_, _, c)| c)
        }
    }

    #[test]
    fn every_consumer_is_offered_in_claim_order_until_one_claims() {
        let mut reg = Scripted::new(false, vec![(Hand::Main, ConsumerKind::Eat, Claim::Claimed)]);
        let out = run_use_click(&mut reg);
        assert_eq!(out.claimant, Some(ConsumerKind::Eat));
        assert!(out.presents_itself());
        let offered: Vec<_> = reg.offered.iter().map(|&(_, k)| k).collect();
        let until_eat: Vec<_> = ConsumerKind::CLAIM_ORDER
            .into_iter()
            .take_while(|&k| k != ConsumerKind::Eat)
            .chain([ConsumerKind::Eat])
            .collect();
        assert_eq!(offered, until_eat, "nothing after the claimant is offered");
        assert_eq!(reg.acting, Hand::Main);
    }

    #[test]
    fn the_off_hand_acts_only_when_the_whole_main_pass_passes() {
        let mut reg = Scripted::new(
            true,
            vec![(Hand::Off, ConsumerKind::Place, Claim::Placed(7))],
        );
        let out = run_use_click(&mut reg);
        assert_eq!(out.claimant, Some(ConsumerKind::Place));
        assert_eq!(out.placement, Some(7));
        assert!(out.off_hand_acted());
        let main = reg
            .offered
            .iter()
            .filter(|&&(h, _)| h == Hand::Main)
            .count();
        assert_eq!(
            main,
            ConsumerKind::CLAIM_ORDER.len(),
            "the main pass ran whole"
        );
        assert_eq!(reg.acting, Hand::Main, "the acting hand never leaks");
    }

    #[test]
    fn a_main_hand_claim_never_reaches_the_off_hand() {
        let mut reg = Scripted::new(
            true,
            vec![
                (Hand::Main, ConsumerKind::BuiltinBlock, Claim::Claimed),
                (Hand::Off, ConsumerKind::Registered, Claim::Claimed),
            ],
        );
        let out = run_use_click(&mut reg);
        assert_eq!(out.claimant, Some(ConsumerKind::BuiltinBlock));
        assert!(!out.off_hand_acted());
        assert!(reg.offered.iter().all(|&(h, _)| h == Hand::Main));
    }

    #[test]
    fn an_empty_off_hand_runs_no_second_pass() {
        let mut reg = Scripted::new(
            false,
            vec![(Hand::Off, ConsumerKind::Place, Claim::Placed(1))],
        );
        let out = run_use_click(&mut reg);
        assert!(!out.consumed());
        assert_eq!(out.hand, Hand::Main);
        assert!(reg.offered.iter().all(|&(h, _)| h == Hand::Main));
    }

    #[test]
    fn the_registered_consumer_needs_a_target() {
        assert!(!registered_offered(None, None));
        assert!(registered_offered(Some(IVec3::ZERO), None));
        assert!(
            registered_offered(None, Some(3)),
            "a mob click is an attempt"
        );
    }
}
