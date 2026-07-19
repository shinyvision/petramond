//! World-anchored sounds are event-driven and POSITIONAL: the
//! app buffers a spatial cue per world event and plays NOTHING immediately
//! off the `GameEvents` one-shots that used to drive local plays — so an
//! action can never sound twice (once locally, once via its broadcast event).

use super::app;
use crate::app::{render::tick_idle_mob_sounds, MobSoundState};
use crate::audio::SpatialListener;
use crate::game::presentation::MobPresentation;
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
        open_gui: Some((crate::gui::GuiKind::Chest, Some(pos))),
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

#[test]
fn idle_sound_deadlines_are_consumed_while_inventory_is_open() {
    let mut test_app = app();
    test_app.toggle_inventory();
    assert!(test_app.screen.inventory_open());

    let mobs = [mob_presentation(41), mob_presentation(73)];
    let positions: Vec<_> = mobs.iter().map(|mob| (mob.id, mob.pos)).collect();
    let due_tick = 100;
    for mob in &mobs {
        test_app.mob_sound_state.insert(
            mob.id,
            MobSoundState {
                next_idle_tick: due_tick,
                sequence: 0,
            },
        );
    }
    let first_handle = test_app.next_mob_sound_handle;
    let listener = SpatialListener {
        pos: Vec3::ZERO,
        right: Vec3::X,
    };

    let app = &mut test_app.app;
    tick_idle_mob_sounds(
        &mut app.audio,
        &mut app.mob_sound_state,
        &mut app.next_mob_sound_handle,
        listener,
        &mobs,
        &positions,
        due_tick,
    );

    assert_eq!(app.next_mob_sound_handle, first_handle + 2);
    for mob in &mobs {
        let state = &app.mob_sound_state[&mob.id];
        assert_eq!(state.sequence, 1);
        assert!(state.next_idle_tick > due_tick);
    }

    app.toggle_inventory();
    assert!(!app.screen.inventory_open());
    tick_idle_mob_sounds(
        &mut app.audio,
        &mut app.mob_sound_state,
        &mut app.next_mob_sound_handle,
        listener,
        &mobs,
        &positions,
        due_tick,
    );

    assert_eq!(
        app.next_mob_sound_handle,
        first_handle + 2,
        "closing inventory at the same tick must not release a mob-sound chorus"
    );
}

fn mob_presentation(id: u64) -> MobPresentation {
    MobPresentation {
        id,
        kind: crate::mob::Mob::Sheep,
        prev_pos: Vec3::ZERO,
        pos: Vec3::ZERO,
        prev_yaw: 0.0,
        yaw: 0.0,
        prev_anim_time: 0.0,
        anim_time: 0.0,
        moving: false,
        idle_anim: None,
        prev_head_yaw: 0.0,
        head_yaw: 0.0,
        prev_head_pitch: 0.0,
        head_pitch: 0.0,
        skylight: 0,
        blocklight: 0,
        hurt_flash: 0.0,
        dead: false,
        shorn: false,
        emitters: Vec::new(),
        anims: Vec::new(),
        emitter_tint: [1.0; 3],
        ragdoll_pose: None,
    }
}
