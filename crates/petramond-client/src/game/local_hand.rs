//! The LOCAL hand's predicted one-shots.
//!
//! The server never echoes a gesture the client already animated, so these
//! latches are the ONLY source of the own hand animation: the prediction
//! paths latch what they predicted this frame (a consumed use click, a swing,
//! a throw, a finished break, a committed place), and event assembly takes
//! the whole frame at once. The attack follow-through window — the client's
//! mirror of the server's attack cooldown — lives here too, since it decides
//! whether a press swings at all.

use petramond::player::one_shot::OneShot;
use petramond_world::block::Block;
use petramond_world::inventory::Hand;

#[derive(Copy, Clone, Debug, Default)]
pub(super) struct UseJab {
    pub consumed: bool,
    pub off_hand: bool,
    pub presents_itself: bool,
    pub places: bool,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct HandFrame {
    pub interacted: bool,
    pub interacted_off_hand: bool,
    pub interacted_presents_itself: bool,
    pub interacted_places: bool,
    pub swung: bool,
    pub threw: bool,
    pub broke: Option<Block>,
    pub placed: Option<Block>,
    pub placed_off_hand: bool,
}

#[derive(Default)]
pub(super) struct LocalHand {
    use_jab: UseJab,
    swung: bool,
    threw: bool,
    broke: Option<Block>,
    placed: Option<Block>,
    placed_off_hand: bool,
    attack_recovery: f32,
    attack_queued: bool,
    swing_events: mod_api::HandSwing,
}

impl LocalHand {
    pub(super) fn latch_use(&mut self, jab: UseJab) {
        self.use_jab = UseJab {
            off_hand: jab.consumed && jab.off_hand,
            ..jab
        };
    }

    pub(super) fn latch_swing(&mut self) {
        self.swung = true;
    }

    pub(super) fn latch_throw(&mut self, held_something: bool) {
        self.threw |= held_something;
    }

    pub(super) fn latch_break(&mut self, block: Block) {
        self.broke = Some(block);
    }

    pub(super) fn latch_place(&mut self, block: Block, hand: Hand) {
        self.placed = Some(block);
        self.placed_off_hand = hand == Hand::Off;
    }

    pub(super) fn recover(&mut self, dt: f32) {
        self.attack_recovery = (self.attack_recovery - dt).max(0.0);
    }

    /// Whether the primary button swings the hand THIS frame: a fresh press,
    /// or one held over from the swing still following through.
    ///
    /// A swing plays WHOLE — the recovery mirrors the server's attack window
    /// (`recovery_secs`, scaled by the same claimed attribute: a pack pacing a
    /// tool off its own animation claims it to zero and paces the hand
    /// itself). A press landing inside that window is neither spent nor
    /// predicted on the spot — it is HELD, one deep, and fires by itself the
    /// frame the hand comes home. The server holds the press it receives the
    /// same way, so a queued swing cannot die in the gap between the clocks.
    ///
    /// A mining press (`mining`) swings nothing (that arc is the dig loop's),
    /// so it neither arms the recovery nor is held by one. A `denied` action
    /// did not happen: nothing of it is held over.
    pub(super) fn attack_press(
        &mut self,
        denied: bool,
        mining: bool,
        clicked: bool,
        recovery_secs: impl FnOnce() -> f32,
    ) -> bool {
        if denied {
            self.attack_queued = false;
            return false;
        }
        if mining {
            return clicked;
        }
        if self.attack_recovery > 0.0 {
            self.attack_queued |= clicked;
            return false;
        }
        if !clicked && !self.attack_queued {
            return false;
        }
        self.attack_queued = false;
        self.attack_recovery = recovery_secs();
        true
    }

    pub(super) fn take_frame(
        &mut self,
        used_unpredicted: bool,
        used_unpredicted_off: bool,
    ) -> HandFrame {
        let jab = std::mem::take(&mut self.use_jab);
        let interacted = jab.consumed || used_unpredicted;
        let off = jab.off_hand || used_unpredicted_off;
        HandFrame {
            interacted,
            interacted_off_hand: interacted && off,
            interacted_presents_itself: interacted && jab.presents_itself && !used_unpredicted,
            interacted_places: interacted && jab.places && !used_unpredicted,
            swung: std::mem::take(&mut self.swung),
            threw: std::mem::take(&mut self.threw),
            broke: self.broke.take(),
            placed: self.placed.take(),
            placed_off_hand: std::mem::take(&mut self.placed_off_hand),
        }
    }

    pub(super) fn latch_swing_events(
        &mut self,
        one_shots: impl IntoIterator<Item = (Hand, OneShot)>,
    ) {
        for (hand, kind) in one_shots {
            let slot = match hand {
                Hand::Main => &mut self.swing_events.main,
                Hand::Off => &mut self.swing_events.off,
            };
            if slot.is_none() {
                *slot = Some(match kind {
                    OneShot::Swing => mod_api::SwingKind::Attack,
                    OneShot::Break => mod_api::SwingKind::Break,
                    OneShot::Place => mod_api::SwingKind::Place,
                    OneShot::Throw => mod_api::SwingKind::Throw,
                    OneShot::Interact => mod_api::SwingKind::Interact,
                });
            }
        }
    }

    pub(super) fn take_swing_events(&mut self) -> mod_api::HandSwing {
        std::mem::take(&mut self.swing_events)
    }

    #[cfg(test)]
    pub(super) fn placed(&self) -> Option<Block> {
        self.placed
    }

    #[cfg(test)]
    pub(super) fn broke(&self) -> Option<Block> {
        self.broke
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_is_taken_whole_and_resets() {
        let mut hand = LocalHand::default();
        hand.latch_use(UseJab {
            consumed: true,
            off_hand: true,
            presents_itself: false,
            places: true,
        });
        hand.latch_swing();
        hand.latch_throw(false);
        hand.latch_place(Block::Dirt, Hand::Off);
        let frame = hand.take_frame(false, false);
        assert!(frame.interacted && frame.interacted_off_hand && frame.interacted_places);
        assert!(frame.swung && !frame.threw);
        assert_eq!(frame.placed, Some(Block::Dirt));
        assert!(frame.placed_off_hand);
        assert_eq!(hand.take_frame(false, false), HandFrame::default());
    }

    #[test]
    fn an_off_hand_flag_without_a_consumed_click_is_meaningless() {
        let mut hand = LocalHand::default();
        hand.latch_use(UseJab {
            consumed: false,
            off_hand: true,
            presents_itself: false,
            places: false,
        });
        let frame = hand.take_frame(false, false);
        assert!(!frame.interacted && !frame.interacted_off_hand);
    }

    #[test]
    fn an_unpredicted_echo_jabs_but_never_presents_itself() {
        let mut hand = LocalHand::default();
        hand.latch_use(UseJab {
            consumed: false,
            off_hand: false,
            presents_itself: true,
            places: true,
        });
        let frame = hand.take_frame(true, true);
        assert!(frame.interacted && frame.interacted_off_hand);
        assert!(!frame.interacted_presents_itself && !frame.interacted_places);
    }

    #[test]
    fn a_press_inside_the_follow_through_is_held_one_deep() {
        let mut hand = LocalHand::default();
        assert!(hand.attack_press(false, false, true, || 0.4));
        assert!(!hand.attack_press(false, false, true, || 0.4), "mid-swing");
        assert!(!hand.attack_press(false, false, false, || 0.4));
        hand.recover(0.5);
        assert!(
            hand.attack_press(false, false, false, || 0.4),
            "the held press fires when the hand comes home"
        );
        hand.recover(0.5);
        assert!(
            !hand.attack_press(false, false, false, || 0.4),
            "held only once"
        );
    }

    #[test]
    fn a_denied_press_is_dropped_and_a_mining_press_passes_through() {
        let mut hand = LocalHand::default();
        assert!(hand.attack_press(false, false, true, || 0.4));
        assert!(!hand.attack_press(false, false, true, || 0.4));
        assert!(!hand.attack_press(true, false, false, || 0.4), "denied");
        hand.recover(0.5);
        assert!(
            !hand.attack_press(false, false, false, || 0.4),
            "nothing held"
        );
        assert!(hand.attack_press(false, true, true, || panic!("mining arms nothing")));
    }

    #[test]
    fn swing_events_keep_the_first_gesture_per_hand() {
        let mut hand = LocalHand::default();
        hand.latch_swing_events([
            (Hand::Main, OneShot::Place),
            (Hand::Main, OneShot::Swing),
            (Hand::Off, OneShot::Throw),
        ]);
        let swing = hand.take_swing_events();
        assert_eq!(swing.main, Some(mod_api::SwingKind::Place));
        assert_eq!(swing.off, Some(mod_api::SwingKind::Throw));
        assert_eq!(hand.take_swing_events().main, None);
    }
}
