//! World-anchored sounds are event-driven and POSITIONAL: the
//! app buffers a spatial cue per world event and plays NOTHING immediately
//! off the `GameEvents` one-shots that used to drive local plays — so an
//! action can never sound twice (once locally, once via its broadcast event).

use super::app;
use crate::app::{
    render::{tick_footstep_sounds, tick_idle_mob_sounds},
    MobSoundState,
};
use petramond_audio::SpatialListener;
use crate::game::presentation::{FootstepSource, MobPresentation};
use crate::game::{GameEvents, WorldEvent};
use petramond::mathh::{IVec3, Vec3};

#[test]
fn world_anchored_sounds_come_from_events_once_never_from_one_shots() {
    let mut app = app();
    let pos = IVec3::new(4, 64, 4);
    let events = GameEvents {
        // The actor's own one-shots (hand animation feeds) — their former
        // local sound plays are gone.
        placed_block: Some(petramond::block::Block::Dirt),
        toggled_door: Some(true),
        open_gui: Some((petramond::gui_state::GuiKind::Chest, Some(pos))),
        interacted: true,
        // The broadcast events every observer presents, positionally.
        world_events: vec![
            WorldEvent::BlockPlaced {
                pos,
                block: petramond::block::Block::Dirt,
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
        kind: petramond::mob::Mob::Sheep,
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
        blocklight: petramond::light::BlockLight6::DARK,
        hurt_flash: 0.0,
        dead: false,
        shorn: false,
        emitters: Vec::new(),
        anims: Vec::new(),
        emitter_tint: [1.0; 3],
        ragdoll_pose: None,
    }
}

/// The footstep cadence: one step per body per [`FOOTSTEP_INTERVAL_TICKS`], a
/// step the MOMENT a body starts walking, and silence for a body that is not.
/// The handle pool is the observable — every step allocates exactly one, so it
/// counts plays without asserting on clip identity.
#[test]
fn footsteps_fire_on_first_sight_then_hold_their_cadence() {
    let mut test_app = app();
    let listener = SpatialListener {
        pos: Vec3::ZERO,
        right: Vec3::X,
    };
    let walking = |id: u64| FootstepSource {
        id,
        pos: Vec3::new(0.0, 64.0, 0.0),
        ground: Some(petramond::block::Block::Stone),
        sprinting: false,
    };
    // A sneaking body arrives exactly as a standing one — presentation, not
    // `App`, is what silences it.
    let standing = |id: u64| FootstepSource {
        ground: None,
        ..walking(id)
    };
    let sprinting = |id: u64| FootstepSource {
        sprinting: true,
        ..walking(id)
    };
    let app = &mut test_app.app;
    let steps = |app: &mut crate::app::App, rows: &[FootstepSource], tick: u64| -> u64 {
        let before = app.next_mob_sound_handle;
        tick_footstep_sounds(
            &mut app.audio,
            &mut app.footstep_next_tick,
            &mut app.next_mob_sound_handle,
            listener,
            rows,
            tick,
        );
        app.next_mob_sound_handle - before
    };

    // Two bodies seen walking for the first time both step at once.
    assert_eq!(steps(app, &[walking(0), walking(3)], 500), 2);
    // …and neither steps again until the interval has passed.
    for t in 501..510 {
        assert_eq!(steps(app, &[walking(0), walking(3)], t), 0, "tick {t}");
    }
    assert_eq!(steps(app, &[walking(0), walking(3)], 510), 2);

    // A body that stops walking is silent, and does NOT bank steps: resuming
    // after a long pause plays one, not one per tick missed.
    for t in 511..560 {
        assert_eq!(steps(app, &[standing(0)], t), 0, "standing at {t}");
    }
    assert_eq!(steps(app, &[walking(0)], 560), 1);
    assert_eq!(steps(app, &[walking(0)], 561), 0);

    // A SPRINT tightens the interval to 7 ticks.
    assert_eq!(steps(app, &[sprinting(0)], 571), 1);
    for t in 572..578 {
        assert_eq!(steps(app, &[sprinting(0)], t), 0, "sprinting at {t}");
    }
    assert_eq!(steps(app, &[sprinting(0)], 578), 1);

    // Bodies that leave take their cadence state with them.
    assert!(app.footstep_next_tick.contains_key(&0));
    steps(app, &[], 600);
    assert!(app.footstep_next_tick.is_empty());
}
