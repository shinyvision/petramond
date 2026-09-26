//! The use-click consumer chain, defined once: the two-pass hand ladder and
//! the walk down the consumer registry in [`ConsumerKind::CLAIM_ORDER`].
//!
//! Both mirrors run THIS walk. The server's consumers execute (a claim is a
//! world change, a GUI open, an eat start); the client's consumers predict
//! against the replica (a claim is a jab, a place ghost). What a consumer
//! does with its turn is the implementor's; who is offered the click, in
//! which order, with which hand, and when the walk stops is this module's —
//! so the order cannot drift between the authority and the prediction.
//!
//! The per-consumer GATES that are pure reads live beside the walk
//! ([`registered_offered`], [`builtin_claims_at`], and the item-use rules in
//! [`super::item_use`]), so each consumer's claim test is written once too.

use super::interact::ConsumerKind;
use petramond_world::world::WorldData;
use petramond_math::math::IVec3;
use petramond_world::inventory::Hand;

/// One consumer's verdict on the attempt.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Claim<P> {
    /// Not this consumer's business — the walk continues.
    Pass,
    /// The attempt is consumed; the walk ends.
    Claimed,
    /// Consumed by a placement: the server names the anchor it landed at
    /// (the ghost-accept convention needs it), the client its place
    /// prediction.
    Placed(P),
}

/// The consumer registry as one mirror implements it. The walk hands every
/// consumer the click in turn; the implementor owns the acting-hand context
/// every held read resolves through.
pub trait UseClickConsumers {
    /// What a placement claim carries (see [`Claim::Placed`]).
    type Placement;

    /// Whether the acting body holds anything in its off hand. An empty off
    /// hand runs no second pass: empty-hand interactions stay a main-hand
    /// affair.
    fn off_hand_occupied(&self) -> bool;

    /// Make `hand` the acting hand. The attempt never names a hand — every
    /// consumer (and every mod host call) resolves the held item through
    /// this context, so the off-hand pass needs no new vocabulary.
    fn set_acting_hand(&mut self, hand: Hand);

    /// Offer the attempt to the `kind` consumer.
    fn offer(&mut self, kind: ConsumerKind) -> Claim<Self::Placement>;
}

/// How one use click resolved.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct UseClickOutcome<P> {
    /// The consumer that claimed it; `None` = every consumer passed with
    /// both hands.
    pub claimant: Option<ConsumerKind>,
    /// The hand that acted (`Main` when nothing did).
    pub hand: Hand,
    /// The placement a [`Claim::Placed`] carried.
    pub placement: Option<P>,
}

impl<P> UseClickOutcome<P> {
    /// Whether anything claimed the click.
    pub fn consumed(&self) -> bool {
        self.claimant.is_some()
    }

    /// Whether the OFF hand's pass is the one that claimed it.
    pub fn off_hand_acted(&self) -> bool {
        self.consumed() && self.hand == Hand::Off
    }

    /// Whether the claimant presents its own claim (an eat's raise), leaving
    /// the hand no jab to play.
    pub fn presents_itself(&self) -> bool {
        self.claimant.is_some_and(ConsumerKind::presents_itself)
    }
}

/// One pass down the registry with the current acting hand: the first
/// consumer that does not pass wins and nothing later is offered the click.
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

/// The whole use click: if the MAIN hand can act, it acts; only when the
/// whole registry passes does the walk run again with the OFF hand acting
/// (and only if it holds something). The acting hand is dispatch context
/// only — it is reset to `Main` before returning, so level-state reads after
/// the click (the roster, replication, the render frame) stay main-hand.
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

/// Whether the registered (mod) consumer is offered the attempt at all: it
/// needs something under the crosshair — a block cell or a live mob (a
/// targeted mob blanks the block look, and is still an attempt).
pub fn registered_offered(block: Option<IVec3>, mob: Option<u64>) -> bool {
    block.is_some() || mob.is_some()
}

/// Whether the block built-in consumer claims a click on `pos`: the cell's
/// row declares a capability and the shared claim rule
/// (`block::builtin_claims_click` — built-ins pass on sneak) takes it.
pub fn builtin_claims_at(world: &WorldData, pos: IVec3, sneaking: bool) -> bool {
    let block = petramond_world::block::Block::from_id(world.chunk_block(pos.x, pos.y, pos.z));
    petramond_world::block::builtin_claims_click(block, sneaking)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scripted registry: which (hand, kind) pairs claim, and a log of
    /// every offer the walk made.
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
        let mut reg = Scripted::new(
            false,
            vec![(Hand::Main, ConsumerKind::Eat, Claim::Claimed)],
        );
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
        let main = reg.offered.iter().filter(|&&(h, _)| h == Hand::Main).count();
        assert_eq!(main, ConsumerKind::CLAIM_ORDER.len(), "the main pass ran whole");
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
        assert!(registered_offered(None, Some(3)), "a mob click is an attempt");
    }
}
