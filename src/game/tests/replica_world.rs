//! End-to-end contract tests for the replica-world cutover (multiplayer
//! C2c-ii): the in-process pipe streams the server world into `Game.replica`
//! (columns before sections), per-tick deltas keep it converged (door toggles
//! included), the open-chest set replicates, and terrain leaving the keep
//! shape unloads from the replica.

use super::super::tick::TICK_DT;
use super::common::game;
use crate::block::Block;
use crate::chunk::{Chunk, ChunkPos, CHUNK_SX, CHUNK_SZ};
use crate::facing::Facing;
use crate::game::GameInput;
use crate::mathh::{IVec3, Vec3};

/// A flat stone floor at y=64 in column (0,0) on the SERVER world, with the
/// player (client + session) standing on it — the fixture the pipe then
/// replicates.
fn floored_game_at(feet: Vec3) -> super::common::TestGame {
    let mut game = game();
    game.server.world.clear_world();
    let mut chunk = Chunk::new(0, 0);
    for z in 0..CHUNK_SZ {
        for x in 0..CHUNK_SX {
            chunk.set_block(x, 64, z, Block::Stone);
        }
    }
    game.server
        .world
        .insert_chunk_for_test(ChunkPos::new(0, 0), chunk);
    place_player(&mut game, feet);
    game
}

fn place_player(game: &mut super::common::TestGame, feet: Vec3) {
    game.player.pos = feet;
    game.player.vel = Vec3::ZERO;
    game.server.sessions[0].player.pos = feet;
    game.server.sessions[0].player.vel = Vec3::ZERO;
}

/// One frame that executes exactly one fixed tick (dt = TICK_DT).
fn frame(game: &mut super::common::TestGame) {
    game.tick(TICK_DT, &GameInput::default());
}

#[test]
fn local_pipe_streams_terrain_into_the_replica_and_deltas_converge_it() {
    let mut game = floored_game_at(Vec3::new(8.5, 65.0, 8.5));

    // The first pumps stream the fixture into the replica. Sections ship only
    // once the server's light bake lands (the light-final ship gate) and the
    // bake runs on async workers — pump bounded until the floor arrives.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while game.replica.chunk_block(8, 64, 8) != Block::Stone.id() {
        assert!(
            std::time::Instant::now() < deadline,
            "the server floor replicated"
        );
        frame(&mut game);
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(
        game.replica.loaded_section_count() > 0,
        "replica sections appear from the pipe"
    );
    assert!(
        game.replica
            .section_at_world_for_test(8, 64, 8)
            .is_some_and(|s| s.has_baked_light() && !s.light_dirty),
        "the install seeded the server's baked light — the replica never bakes"
    );
    assert!(
        game.replica.chunk_loaded(0, 0),
        "the column data replicated (heightmap/biome/summaries)"
    );

    // A post-join server edit reaches the replica through the delta pipe.
    assert!(game.server.world.set_block_world(8, 66, 8, Block::Dirt));
    frame(&mut game);
    assert_eq!(
        game.replica.chunk_block(8, 66, 8),
        Block::Dirt.id(),
        "a block placed server-side shows up in the replica after the pump"
    );

    // A door placed server-side replicates with its state, and a TOGGLE (no
    // block-id change) flips the replica's door map — collision + resting
    // swing angle read it.
    let door = IVec3::new(5, 65, 5);
    assert!(game
        .server
        .world
        .place_door(door, Block::OakDoor, Facing::East));
    frame(&mut game);
    assert_eq!(
        game.replica
            .door_state_at(door.x, door.y, door.z)
            .map(|s| s.open),
        Some(false),
        "the placed door replicated closed"
    );
    assert_eq!(game.server.world.toggle_door(door), Some(door));
    frame(&mut game);
    assert_eq!(
        game.replica
            .door_state_at(door.x, door.y, door.z)
            .map(|s| s.open),
        Some(true),
        "the toggle updated the replica door map"
    );

    // Terrain leaving the keep shape unloads from the replica (the far column
    // gets a ColumnUnload once the anchor moves away).
    place_player(&mut game, Vec3::new(328.5, 65.0, 328.5));
    for _ in 0..3 {
        frame(&mut game);
    }
    assert!(
        !game.replica.chunk_loaded(0, 0),
        "the left-behind column unloaded from the replica"
    );
    assert!(
        game.replica.section_at_world_for_test(8, 64, 8).is_none(),
        "its sections dropped with it"
    );
}

/// Light is server-owned end to end: a post-ship edit that changes light
/// (a torch appearing) reaches the replica as a `LightData` rebake — the
/// replica itself never marks light dirty or bakes.
#[test]
fn server_rebakes_replicate_as_light_data() {
    let mut game = floored_game_at(Vec3::new(8.5, 65.0, 8.5));
    let torch = IVec3::new(6, 65, 6);

    // Wait for the lit floor section to ship.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while game.replica.chunk_block(8, 64, 8) != Block::Stone.id() {
        assert!(std::time::Instant::now() < deadline, "the floor replicated");
        frame(&mut game);
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    let block_at = |g: &super::common::TestGame| {
        g.replica
            .section_at_world_for_test(torch.x, torch.y, torch.z)
            .map(|s| s.blocklight_at(6, 1, 6))
            .unwrap_or(0)
    };
    assert_eq!(block_at(&game), 0, "no emitter yet — block light is dark");

    // The server edit dirties light server-side only; the replica applies the
    // delta immediately (block visible) and receives the light as LightData.
    // (Emitters come from the torch placement map — mirror the place funnel.)
    assert!(game
        .server
        .world
        .set_block_world(torch.x, torch.y, torch.z, Block::Torch));
    game.server
        .world
        .insert_torch(torch, crate::torch::TorchPlacement::Floor);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while block_at(&game) == 0 {
        assert!(
            std::time::Instant::now() < deadline,
            "the server's rebake reached the replica as LightData"
        );
        frame(&mut game);
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(
        game.replica
            .section_at_world_for_test(torch.x, torch.y, torch.z)
            .is_some_and(|s| !s.light_dirty),
        "the replica never holds dirty light — it waits for the server"
    );
}

#[test]
fn open_chest_state_replicates_and_drives_the_lid_target() {
    let mut game = floored_game_at(Vec3::new(8.5, 65.0, 8.5));
    let pos = IVec3::new(3, 65, 3);
    assert!(game
        .server
        .world
        .set_block_world(pos.x, pos.y, pos.z, Block::Chest));
    game.server.world.insert_chest(pos, Facing::West);

    game.server
        .open_chest_screen_for(0, pos, &mut Default::default());
    frame(&mut game);
    assert!(
        game.open_chests.contains(&pos),
        "an open chest screen replicates into the batch's open set"
    );

    game.close_open_menu();
    frame(&mut game);
    assert!(
        game.open_chests.is_empty(),
        "closing the screen empties the replicated set on the next batch"
    );
}
