//! One session's per-tick one-shots → the `player_actions` rows observers
//! animate from. The engine's own gestures resolve through
//! `player::one_shot` into the same `(rig, event)` rows a mod's
//! `FirePlayerAnimatorEvent` queues, and the two lists ride one lane:
//! [`PlayerActionKind::Animator`], observed rigs only.

use petramond_world::inventory::Hand;

use crate::events::tick::PlayerTickEvents;
use crate::net::protocol::PlayerActionKind;
use crate::player::one_shot::{self, OneShot};
use crate::player::rigs;

/// Every action row `p` produces this window, in emission order.
pub fn player_action_kinds(p: &PlayerTickEvents, mut push: impl FnMut(PlayerActionKind)) {
    let click_hand = if p.click_off_hand { Hand::Off } else { Hand::Main };
    let gestures = [
        (p.swung_hand, Hand::Main, OneShot::Swing),
        (p.broke_block.is_some(), Hand::Main, OneShot::Break),
        (p.placed_block.is_some(), click_hand, OneShot::Place),
        (p.threw_item, Hand::Main, OneShot::Throw),
        (p.used_item || p.interacted, click_hand, OneShot::Interact),
    ];
    let engine = gestures
        .into_iter()
        .filter(|(fired, ..)| *fired)
        .flat_map(|(_, hand, kind)| one_shot::fired(hand, kind));
    for (rig, event) in engine.chain(p.animator_events.iter().copied()) {
        if rigs::observed(rig) {
            push(PlayerActionKind::Animator { rig, event });
        }
    }
    if p.player_died {
        push(PlayerActionKind::Died);
    }
    if p.respawned {
        push(PlayerActionKind::Respawned);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::RigId;

    fn kinds(p: &PlayerTickEvents) -> Vec<PlayerActionKind> {
        let mut out = Vec::new();
        player_action_kinds(p, |k| out.push(k));
        out
    }

    /// An engine gesture rides the lane a mod fire rides: the observed
    /// rigs' resolved graph events, and nothing for an unobserved rig.
    #[test]
    fn engine_one_shots_become_observed_rigs_graph_events() {
        let body = rigs::id(rigs::PLAYER_BODY).expect("body rig");
        let viewmodel = rigs::id(rigs::PLAYER_FIRST_PERSON).expect("viewmodel rig");
        let mut p = PlayerTickEvents::default();
        p.broke_block = Some(petramond_world::block::Block::Stone);
        p.animator_events.push((viewmodel, 0));
        let out = kinds(&p);
        let event = one_shot::resolve(body, Hand::Main, OneShot::Break)
            .expect("the shipped body graph hears a break");
        assert_eq!(
            out,
            vec![PlayerActionKind::Animator { rig: body, event }],
            "one observed rig row per gesture; the viewmodel's rows never ride"
        );
    }

    /// The acting hand selects the row: an off-hand click places and
    /// interacts with the left hand's events.
    #[test]
    fn the_click_hand_picks_the_hand_prefix() {
        let body = rigs::id(rigs::PLAYER_BODY).expect("body rig");
        let mut p = PlayerTickEvents::default();
        p.interacted = true;
        p.click_off_hand = true;
        let event = one_shot::resolve(body, Hand::Off, OneShot::Interact)
            .expect("the shipped body graph hears an off-hand interact");
        assert_eq!(kinds(&p), vec![PlayerActionKind::Animator { rig: body, event }]);
        assert_ne!(
            Some(event),
            one_shot::resolve(body, Hand::Main, OneShot::Interact),
            "the two hands' interacts are distinct graph events"
        );
    }

    #[test]
    fn unregistered_rigs_never_ride() {
        let mut p = PlayerTickEvents::default();
        p.animator_events.push((RigId(u16::MAX), 3));
        p.player_died = true;
        assert_eq!(kinds(&p), vec![PlayerActionKind::Died]);
    }
}
