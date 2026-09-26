//! A server tick system's spatial sound commands reach the client's
//! `GameEvents` intact. (The bus and stage-ordering contracts themselves are
//! server behaviour, tested in `src/server/game/tests/events.rs`.)

use super::common::game;
use crate::game::{GameInput, SpatialSoundCommand};
use petramond::events::{Attach, Stage};
use petramond_math::world_pos::WorldPos;

#[test]
fn spatial_sound_commands_reach_game_events_without_loss() {
    let mut game = game();
    let sound = petramond_world::sound_registry::by_name("petramond:item_pickup")
        .expect("engine sound exists");
    game.sim_mut()
        .systems
        .attach(Attach::Before(Stage::Mining), 0, move |ctx| {
            ctx.feed
                .world
                .spatial_sounds
                .push(SpatialSoundCommand::PlayAt {
                    handle: 7,
                    sound,
                    pos: WorldPos::new(3.0, 81.0, -2.0),
                    volume: 0.6,
                    pitch: 1.1,
                });
        });

    let events = game.tick(0.05, &GameInput::default());
    assert_eq!(
        events.spatial_sounds,
        vec![SpatialSoundCommand::PlayAt {
            handle: 7,
            sound,
            pos: WorldPos::new(3.0, 81.0, -2.0),
            volume: 0.6,
            pitch: 1.1,
        }]
    );
}
