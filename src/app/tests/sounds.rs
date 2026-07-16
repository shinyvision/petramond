//! World-anchored sounds are event-driven and POSITIONAL: the
//! app buffers a spatial cue per world event and plays NOTHING immediately
//! off the `GameEvents` one-shots that used to drive local plays — so an
//! action can never sound twice (once locally, once via its broadcast event).

use super::app;
use crate::game::{GameEvents, WorldEvent};
use crate::mathh::{IVec3, Vec3};

#[test]
fn world_anchored_sounds_come_from_events_once_never_from_one_shots() {
    let mut app = app();
    let pos = IVec3::new(4, 64, 4);
    let events = GameEvents {
        // The actor's own one-shots (hand animation feeds) — their former
        // local sound plays are gone.
        placed_block: Some(crate::block::Block::Dirt),
        toggled_door: Some(true),
        open_chest: Some(pos),
        interacted: true,
        // The broadcast events every observer presents, positionally.
        world_events: vec![
            WorldEvent::BlockPlaced {
                pos,
                block: crate::block::Block::Dirt,
            },
            WorldEvent::DoorToggled {
                lower: pos,
                open: true,
            },
            WorldEvent::ChestOpened { pos },
            WorldEvent::ChestClosed { pos },
            // A FOREIGN pickup cues positionally; the local player's own
            // pickup keeps the non-positional `picked_up_item` play instead.
            WorldEvent::ItemPickedUp {
                pos: Vec3::new(4.5, 64.5, 4.5),
                by_self: false,
            },
            WorldEvent::ItemPickedUp {
                pos: Vec3::new(1.5, 64.5, 1.5),
                by_self: true,
            },
        ],
        ..Default::default()
    };

    app.play_game_event_sounds(&events, None, 0.0);

    assert!(
        app.audio.take_played_for_test().is_empty(),
        "no immediate local play for placed/door/chest one-shots (they play \
         positionally from the buffered event cues at the next render)"
    );
    assert_eq!(
        app.world_sound_cues.len(),
        5,
        "one positional cue per world event: place, door, chest open+close, \
         and the FOREIGN pickup (the self pickup stays non-positional)"
    );
}
