//! Combat proof for the zombies PoC mod: real chase+melee strikes flow
//! through the player damage funnel while the mod's wasm `player_damage_pre`
//! handler enforces its 20-tick i-frames — damage AND knockback land at most
//! once per window. Species registration needs the fixture pack in the
//! registry, so the assertions run in a child process (the established
//! `LLAMACRAFT_MODS` re-spawn pattern, staged by `modding::tests`).

use super::super::tick::TickEvents;
use crate::camera::Camera;
use crate::mathh::Vec3;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

#[test]
fn zombie_melee_damage_is_gated_by_the_mods_i_frames_via_wasm() {
    let Some(root) = crate::modding::tests::stage_zombies_fixture("combat") else {
        return;
    };
    crate::modding::tests::run_child_test(&root, "game::tests::zombies_mod::zombie_combat_inner");
}

/// Runs ONLY in the child process spawned above (needs `LLAMACRAFT_MODS`
/// pointing at the fixture packs before first registry touch). Uses the
/// production load path — `Game::new` → `ModHost::load` — so the handler
/// under test is the real installed wasm, dispatched from the real funnel.
#[test]
#[ignore = "spawned by zombie_melee_damage_is_gated_by_the_mods_i_frames_via_wasm with a fixture pack env"]
fn zombie_combat_inner() {
    use crate::block::Block;
    use crate::chunk::{Chunk, ChunkPos, CHUNK_SX, CHUNK_SZ};

    let zombie = crate::mob::defs()
        .iter()
        .position(|d| d.name == "zombies:zombie")
        .map(|i| crate::mob::Mob(i as u8))
        .expect("zombies:zombie registered from the fixture pack");

    let mut game =
        super::common::game_with_camera(Camera::new(Vec3::new(8.0, 66.0, 8.0), 16.0 / 9.0));
    assert_eq!(game.mods_for_test().loaded(), 1, "zombies loaded");
    // The mod spawner needs loaded dark cells in the 32-128 ring; this tiny
    // fixture has neither, so this test owns its zombies. Flat floor, player
    // standing on it.
    game.server.world.clear_world();
    let mut chunk = Chunk::new(0, 0);
    for z in 0..CHUNK_SZ {
        for x in 0..CHUNK_SX {
            chunk.set_block(x, 63, z, Block::Grass);
        }
    }
    game.server
        .world
        .insert_chunk_for_test(ChunkPos::new(0, 0), chunk);
    game.server.sessions[0].player.pos = Vec3::new(8.0, 64.0, 8.0);
    game.server.sessions[0].player.vel = Vec3::ZERO;
    game.server.sessions[0].player.on_ground = true;

    // TWO zombies in reach, facing the player: their independent 20-tick
    // melee cooldowns would land two hits inside one window — only the mod's
    // i-frames keep the applications 20+ ticks apart.
    for dx in [1.1f32, -1.1] {
        let pos = Vec3::new(8.0 + dx, 64.0, 8.0);
        let to_player = game.server.sessions[0].player.body_center() - pos;
        let yaw = (-to_player.x).atan2(-to_player.z);
        assert!(game.server.world.mobs_mut().spawn(zombie, pos, yaw));
    }

    let mut ev = TickEvents::default();
    let h0 = game.server.sessions[0].player.health();
    let mut health = h0;
    let mut hits: Vec<(i64, i32)> = Vec::new(); // (tick index, health drop)
    for tick in 0..50i64 {
        game.server.game_tick_step(&mut ev);
        let h = game.server.sessions[0].player.health();
        if h < health {
            hits.push((tick, health - h));
        }
        health = h;
    }

    assert!(
        hits.len() >= 2,
        "two adjacent zombies land repeated hits across 2.5 s: {hits:?}"
    );
    for &(_, drop) in &hits {
        assert_eq!(
            drop, 3,
            "every application is exactly one hit's damage — simultaneous \
             second strikes are cancelled whole: {hits:?}"
        );
    }
    for pair in hits.windows(2) {
        assert!(
            pair[1].0 - pair[0].0 >= 20,
            "no two damage applications within the 20-tick i-frame window: {hits:?}"
        );
    }
    // An applied (non-cancelled) strike knocks the player back; the player
    // integrates per frame, so on pure ticks the impulse shows in velocity.
    assert!(
        game.server.sessions[0].player.vel.length() > 1.0,
        "knockback reached the player's velocity: {:?}",
        game.server.sessions[0].player.vel
    );
    // The documented inspection mirror: 8-byte LE u64 end-of-window tick.
    let bytes = game
        .server
        .world
        .mod_kv_get("zombies:invuln_until")
        .expect("zombies:invuln_until mirrored to world KV");
    let until = u64::from_le_bytes(bytes.try_into().expect("8-byte LE u64 contract"));
    assert!(until > 0, "the mirror records the window end");
    let (disabled, _, _) = game.mods_for_test().probe(0);
    assert!(!disabled, "zombies stayed healthy through combat");
}

#[test]
fn zombie_sunburn_uses_ragdoll_death_path_via_wasm() {
    let Some(root) = crate::modding::tests::stage_zombies_fixture("sunburn") else {
        return;
    };
    crate::modding::tests::run_child_test(&root, "game::tests::zombies_mod::zombie_sunburn_inner");
}

/// Runs ONLY in the child process spawned above (needs `LLAMACRAFT_MODS`
/// pointing at the fixture packs before first registry touch). The assertion
/// uses the real game tick so the mod's `hurt_mob` action must flow through
/// the mob-hurt funnel, emit `mob_died`, and leave a dead/ragdolling corpse
/// instead of removing the mob with `despawn_mob`.
#[test]
#[ignore = "spawned by zombie_sunburn_uses_ragdoll_death_path_via_wasm with a fixture pack env"]
fn zombie_sunburn_inner() {
    use crate::block::Block;
    use crate::chunk::{Chunk, ChunkPos, CHUNK_SX, CHUNK_SZ, SECTION_VOLUME, SKY_FULL};
    use crate::events::{PostEvent, PostEventKind};
    use crate::mob::MobSoundCategory;

    let zombie = crate::mob::defs()
        .iter()
        .position(|d| d.name == "zombies:zombie")
        .map(|i| crate::mob::Mob(i as u8))
        .expect("zombies:zombie registered from the fixture pack");

    let mut game =
        super::common::game_with_camera(Camera::new(Vec3::new(80.0, 66.0, 8.0), 16.0 / 9.0));
    assert_eq!(game.mods_for_test().loaded(), 1, "zombies loaded");
    game.server.world.clear_world();
    game.server.sessions[0].player.pos = Vec3::new(80.0, 64.0, 8.0);
    game.server.sessions[0].player.vel = Vec3::ZERO;
    game.server.sessions[0].player.on_ground = true;

    fn install_chunk(game: &mut super::common::TestGame, cx: i32, cz: i32, sky_x2: u8, block_x2: u8) {
        let pos = ChunkPos::new(cx, cz);
        game.server.world.insert_empty_column_for_test(pos);
        let mut chunk = Chunk::new(cx, cz);
        for z in 0..CHUNK_SZ {
            for x in 0..CHUNK_SX {
                chunk.set_block(x, 63, z, Block::Grass);
            }
        }
        game.server.world.insert_chunk_for_test(pos, chunk);
        let section = game
            .server
            .world
            .section_at_world_mut_for_test(cx * 16, 64, cz * 16)
            .expect("feet section is loaded");
        section.set_skylight(vec![sky_x2; SECTION_VOLUME].into());
        section.set_blocklight(vec![block_x2; SECTION_VOLUME].into());
    }

    install_chunk(&mut game, 0, 0, SKY_FULL, 0);
    install_chunk(&mut game, 3, 0, 0, SKY_FULL);

    let mut sun_ids = Vec::new();
    for x in [2.5, 5.5, 8.5, 11.5] {
        for z in [2.5, 5.5, 8.5, 11.5] {
            assert!(game
                .server
                .world
                .mobs_mut()
                .spawn(zombie, Vec3::new(x, 64.0, z), 0.0));
            sun_ids.push(game.server.world.mobs().instances().last().unwrap().id());
        }
    }
    // The dark controls must survive the whole loop: keep them outside the
    // chase radius (so they stay in their block-lit chunk) but inside the
    // random-despawn eligibility distance (so they can't be culled).
    let mut dark_ids = Vec::new();
    for z in [4.5, 7.5, 10.5, 13.5] {
        assert!(game
            .server
            .world
            .mobs_mut()
            .spawn(zombie, Vec3::new(52.5, 64.0, z), 0.0));
        dark_ids.push(game.server.world.mobs().instances().last().unwrap().id());
    }

    let deaths = Arc::new(AtomicUsize::new(0));
    {
        let deaths = deaths.clone();
        game.server
            .bus
            .on_post(PostEventKind::MobDied, 0, move |_, ev| {
                if matches!(ev, PostEvent::MobDied { kind, .. } if *kind == zombie) {
                    deaths.fetch_add(1, Ordering::Relaxed);
                }
            });
    }

    let mut ev = TickEvents::default();
    for _ in 0..1_200 {
        game.server.game_tick_step(&mut ev);
        if deaths.load(Ordering::Relaxed) > 0 {
            break;
        }
    }

    assert!(
        deaths.load(Ordering::Relaxed) > 0,
        "sunlit zombies eventually burn through the mob death path"
    );
    assert!(
        ev.world
            .mob_sounds
            .iter()
            .any(|s| s.kind == zombie && s.category == MobSoundCategory::Death),
        "the same death path queued the zombie's data-driven death sound"
    );
    let mobs = game.server.world.mobs().instances();
    assert!(
        sun_ids
            .iter()
            .any(|id| mobs.iter().any(|m| m.id() == *id && m.is_dead())),
        "a sunburned zombie remains as a dead/ragdolling mob instead of being despawned"
    );
    assert!(
        dark_ids
            .iter()
            .all(|id| mobs.iter().any(|m| m.id() == *id && !m.is_dead())),
        "torch-lit/dark-control zombies do not burn without direct sky light"
    );
    let (disabled, _, _) = game.mods_for_test().probe(0);
    assert!(!disabled, "zombies stayed healthy through sunburn");
}
