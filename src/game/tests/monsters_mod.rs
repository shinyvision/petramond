//! Combat proof for the monsters mod: real chase+melee strikes flow through
//! the player damage funnel, whose engine-owned global i-frames admit damage
//! and knockback at most once per window. Species registration needs the
//! fixture pack in the registry, so the assertions run in a child process
//! (the established `PETRAMOND_MODS` re-spawn pattern, staged by
//! `modding::tests`).

use super::super::tick::TickEvents;
use crate::camera::Camera;
use crate::mathh::Vec3;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

#[test]
fn zombie_melee_uses_engine_global_iframes() {
    let Some(root) = crate::modding::tests::stage_monsters_fixture("combat") else {
        return;
    };
    crate::modding::tests::run_child_test(&root, "game::tests::monsters_mod::zombie_combat_inner");
}

/// Runs ONLY in the child process spawned above (needs `PETRAMOND_MODS`
/// pointing at the fixture packs before first registry touch). Uses the
/// production load path — `Game::new` → `ModHost::load` — so the real
/// installed wasm drives melee attacks through the production damage funnel.
#[test]
#[ignore = "spawned by zombie_melee_uses_engine_global_iframes with a fixture pack env"]
fn zombie_combat_inner() {
    use crate::block::Block;
    use crate::chunk::{Chunk, ChunkPos, CHUNK_SX, CHUNK_SZ};

    let zombie = crate::mob::defs()
        .iter()
        .position(|d| d.name == "monsters:zombie")
        .map(|i| crate::mob::Mob(i as u8))
        .expect("monsters:zombie registered from the fixture pack");

    let mut game =
        super::common::game_with_camera(Camera::new(Vec3::new(8.0, 66.0, 8.0), 16.0 / 9.0));
    assert_eq!(game.mods_for_test().loaded(), 1, "monsters loaded");
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

    // TWO zombies in reach, facing the player: their independent melee
    // cooldowns can land together, but the victim's global engine i-frames
    // collapse simultaneous strikes to one application.
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
    let mut peak_knockback = 0.0_f32;
    for tick in 0..50i64 {
        game.server.game_tick_step(&mut ev);
        let h = game.server.sessions[0].player.health();
        if h < health {
            hits.push((tick, health - h));
            peak_knockback = peak_knockback.max(game.server.sessions[0].player.vel.length());
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
    // An applied (non-cancelled) strike knocks the player; server movement
    // friction decays the impulse on later ticks, so sample at hit time.
    assert!(
        peak_knockback > 1.0,
        "knockback reached the player's velocity at hit time: {peak_knockback}"
    );
    let (disabled, _, _) = game.mods_for_test().probe(0);
    assert!(!disabled, "monsters mod stayed healthy through combat");
}

#[test]
fn zombie_sunburn_uses_ragdoll_death_path_via_wasm() {
    let Some(root) = crate::modding::tests::stage_monsters_fixture("sunburn") else {
        return;
    };
    crate::modding::tests::run_child_test(&root, "game::tests::monsters_mod::zombie_sunburn_inner");
}

/// Runs ONLY in the child process spawned above (needs `PETRAMOND_MODS`
/// pointing at the fixture packs before first registry touch). The assertion
/// uses the real game tick so the mod's `damage_mob` action must flow through
/// the mob-damage funnel, emit `mob_died`, and leave a dead/ragdolling corpse
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
        .position(|d| d.name == "monsters:zombie")
        .map(|i| crate::mob::Mob(i as u8))
        .expect("monsters:zombie registered from the fixture pack");

    let mut game =
        super::common::game_with_camera(Camera::new(Vec3::new(80.0, 66.0, 8.0), 16.0 / 9.0));
    assert_eq!(game.mods_for_test().loaded(), 1, "monsters loaded");
    game.server.world.clear_world();
    game.server.sessions[0].player.pos = Vec3::new(80.0, 64.0, 8.0);
    game.server.sessions[0].player.vel = Vec3::ZERO;
    game.server.sessions[0].player.on_ground = true;

    fn install_chunk(
        game: &mut super::common::TestGame,
        cx: i32,
        cz: i32,
        sky_x2: u8,
        block_x2: u8,
    ) {
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
        sun_ids.iter().any(|id| mobs
            .iter()
            .any(|m| m.id() == *id && m.is_dead() && !m.active_emitters().is_empty())),
        "a zombie that burned to death keeps its fire emitters through the ragdoll"
    );
    assert!(
        dark_ids
            .iter()
            .all(|id| mobs.iter().any(|m| m.id() == *id && !m.is_dead())),
        "torch-lit/dark-control zombies do not burn without direct sky light"
    );
    assert!(
        dark_ids.iter().all(|id| mobs
            .iter()
            .any(|m| m.id() == *id && m.active_emitters().is_empty())),
        "unburned zombies show no fire emitters"
    );
    let (disabled, _, _) = game.mods_for_test().probe(0);
    assert!(!disabled, "zombies stayed healthy through sunburn");
}

#[test]
fn zombie_burn_escalates_in_sun_and_cools_in_darkness_via_wasm() {
    let Some(root) = crate::modding::tests::stage_monsters_fixture("burncool") else {
        return;
    };
    crate::modding::tests::run_child_test(
        &root,
        "game::tests::monsters_mod::zombie_burn_cool_inner",
    );
}

/// Runs ONLY in the child process spawned above. Drives the full burn state
/// machine through the real installed wasm and the core day/night clock:
/// ignite in sunlight (`petramond:burn_light` attaches), escalate to
/// `petramond:burn_great` after 200 sunlit light ticks, then — after the clock
/// jumps to midnight — cool one stage per 60 consecutive dark ticks until the
/// fire is out, leaving the zombie alive.
#[test]
#[ignore = "spawned by zombie_burn_escalates_in_sun_and_cools_in_darkness_via_wasm with a fixture pack env"]
fn zombie_burn_cool_inner() {
    use crate::block::Block;
    use crate::chunk::{Chunk, ChunkPos, CHUNK_SX, CHUNK_SZ, SECTION_VOLUME, SKY_FULL};

    let zombie = crate::mob::defs()
        .iter()
        .position(|d| d.name == "monsters:zombie")
        .map(|i| crate::mob::Mob(i as u8))
        .expect("monsters:zombie registered from the fixture pack");
    let light_id = crate::particle_emitters::by_key("petramond:burn_light")
        .expect("core bundle registered")
        .id;
    let great_id = crate::particle_emitters::by_key("petramond:burn_great")
        .expect("core bundle registered")
        .id;

    let mut game =
        super::common::game_with_camera(Camera::new(Vec3::new(30.0, 66.0, 8.0), 16.0 / 9.0));
    assert_eq!(game.mods_for_test().loaded(), 1, "monsters loaded");
    game.server.world.clear_world();
    game.server.sessions[0].player.pos = Vec3::new(30.0, 64.0, 8.0);
    game.server.sessions[0].player.vel = Vec3::ZERO;
    game.server.sessions[0].player.on_ground = true;

    // One full-skylight chunk; the zombie burns under the open sky until the
    // CLOCK, not the terrain, takes the sun away.
    let pos = ChunkPos::new(0, 0);
    game.server.world.insert_empty_column_for_test(pos);
    let mut chunk = Chunk::new(0, 0);
    for z in 0..CHUNK_SZ {
        for x in 0..CHUNK_SX {
            chunk.set_block(x, 63, z, Block::Grass);
        }
    }
    game.server.world.insert_chunk_for_test(pos, chunk);
    let section = game
        .server
        .world
        .section_at_world_mut_for_test(0, 64, 0)
        .expect("feet section is loaded");
    section.set_skylight(vec![SKY_FULL; SECTION_VOLUME].into());

    assert!(game
        .server
        .world
        .mobs_mut()
        .spawn(zombie, Vec3::new(8.5, 64.0, 8.5), 0.0));
    let id = game.server.world.mobs().instances()[0].id();

    let emitters = |game: &super::common::TestGame| -> Vec<u8> {
        game.server
            .world
            .mobs()
            .instances()
            .iter()
            .find(|m| m.id() == id)
            .map(|m| m.active_emitters().to_vec())
            .unwrap_or_default()
    };
    let tick_until = |game: &mut super::common::TestGame,
                      ev: &mut TickEvents,
                      cap: usize,
                      what: &str,
                      done: &dyn Fn(&super::common::TestGame) -> bool| {
        for _ in 0..cap {
            if done(game) {
                return;
            }
            game.server.game_tick_step(ev);
        }
        assert!(done(game), "{what} within {cap} ticks");
    };

    let mut ev = TickEvents::default();
    // 5%/tick ignition: even one zombie ignites all but immediately.
    tick_until(&mut game, &mut ev, 400, "light fire ignites", &|g| {
        emitters(g).contains(&light_id)
    });
    // 200 sunlit light ticks escalate (plus scheduling slack).
    tick_until(&mut game, &mut ev, 300, "great fire escalates", &|g| {
        emitters(g).contains(&great_id)
    });

    // Midnight: the core clock is authoritative for daylight, so the mod's
    // sunlight test goes dark everywhere from the next tick.
    game.server.world.mod_kv_set(
        "petramond:clock".to_owned(),
        9_000u64.to_le_bytes().to_vec(),
    );
    tick_until(
        &mut game,
        &mut ev,
        80,
        "great fire cools to light after 60 dark ticks",
        &|g| {
            let e = emitters(g);
            e.contains(&light_id) && !e.contains(&great_id)
        },
    );
    tick_until(
        &mut game,
        &mut ev,
        80,
        "light fire goes out after 60 more dark ticks",
        &|g| emitters(g).is_empty(),
    );

    let mob = &game.server.world.mobs().instances()[0];
    assert!(
        !mob.is_dead(),
        "the cooled zombie survived its shortened burn"
    );
    let (disabled, _, _) = game.mods_for_test().probe(0);
    assert!(!disabled, "zombies stayed healthy through the cool-down");
}

#[test]
fn hushjaw_spawn_rules_gate_on_depth_distance_and_spacing_via_wasm() {
    let Some(root) = crate::modding::tests::stage_monsters_fixture("hushjaw-spawn") else {
        return;
    };
    crate::modding::tests::run_child_test(
        &root,
        "game::tests::monsters_mod::hushjaw_spawn_rules_inner",
    );
}

/// The hushjaw's spawn policy, asked through the REAL wasm spawner dispatch
/// (`ModHost::hostile_spawn_kind`) with hand-built dark candidates over
/// prepared standable cells: only deep (feet Y below −16), only ≥ 32 blocks
/// from the nearest player, never within 32 blocks of another live hushjaw,
/// and even then only a fraction of eligible sites (the rest stay zombies).
#[test]
#[ignore = "spawned by hushjaw_spawn_rules_gate_on_depth_distance_and_spacing_via_wasm with a fixture pack env"]
fn hushjaw_spawn_rules_inner() {
    use crate::block::Block;
    use crate::chunk::ChunkPos;
    use crate::events::SimCtx;
    use mod_api::HostileSpawnCandidate;

    let kind_of = |name: &str| {
        crate::mob::defs()
            .iter()
            .position(|d| d.name == name)
            .map(|i| crate::mob::Mob(i as u8))
            .unwrap_or_else(|| panic!("{name} registered from the fixture pack"))
    };
    let zombie = kind_of("monsters:zombie");
    let hushjaw = kind_of("monsters:hushjaw");

    let mut game =
        super::common::game_with_camera(Camera::new(Vec3::new(8.0, 66.0, 8.0), 16.0 / 9.0));
    game.server.world.clear_world();
    game.server.world.insert_empty_column_for_test(ChunkPos::new(0, 0));
    // Standable floors under the deep and shallow candidate cells (the engine
    // re-validates body fit on whatever key the mod returns).
    assert!(game.server.world.set_block_world(8, -41, 8, Block::Stone));
    assert!(game.server.world.set_block_world(8, -11, 8, Block::Stone));
    // One tick so core day/night republishes `petramond:time` onto the cleared
    // world — without it the mod's spawner declines every candidate.
    let mut ev = TickEvents::default();
    game.server.game_tick_step(&mut ev);

    let candidate = |y: i32, player_dist: f32| HostileSpawnCandidate {
        pos: [8.5, y as f32, 8.5],
        cell: [8, y, 8],
        combined_light: 0,
        sky_light: 0,
        block_light: 0,
        nearest_player_dist: player_dist,
    };
    fn ask(
        server: &mut crate::server::game::ServerGame,
        cand: &mod_api::HostileSpawnCandidate,
        ev: &mut TickEvents,
    ) -> Option<crate::mob::Mob> {
        let crate::server::game::ServerGame {
            world,
            sessions,
            mods,
            bus,
            ..
        } = server;
        let host = &mut sessions[0];
        let mut ctx = SimCtx {
            world,
            player: &mut host.player,
            gui_state: &mut host.gui_state,
            feed: ev,
            queue: bus.queue_mut(),
        };
        mods.hostile_spawn_kind(&mut ctx, cand)
    }

    // Shallow (feet above −16): never a hushjaw, always the zombie fallback.
    for _ in 0..60 {
        assert_eq!(
            ask(&mut game.server, &candidate(-10, 100.0), &mut ev),
            Some(zombie),
            "a dark site above Y −16 is zombie territory"
        );
    }
    // Deep but near a player (< 32 blocks): still never a hushjaw.
    for _ in 0..60 {
        assert_eq!(
            ask(&mut game.server, &candidate(-40, 28.0), &mut ev),
            Some(zombie),
            "a deep site within 32 blocks of a player is zombie territory"
        );
    }
    // Deep + far: over many asks the seeded claim roll yields BOTH species —
    // hushjaws exist down here, and they don't monopolize the depths.
    let mut kinds = std::collections::HashSet::new();
    for _ in 0..200 {
        let kind = ask(&mut game.server, &candidate(-40, 100.0), &mut ev)
            .expect("a dark deep far candidate always admits some monster");
        kinds.insert(kind);
    }
    assert!(
        kinds.contains(&hushjaw),
        "eligible deep sites are sometimes claimed by the hushjaw"
    );
    assert!(
        kinds.contains(&zombie),
        "unclaimed deep sites still fall through to zombies"
    );
    // Spacing: with a live hushjaw 4 blocks from the site, the claim never
    // happens again — they hunt alone.
    assert!(game
        .server
        .world
        .spawn_mob(hushjaw, Vec3::new(12.5, -40.0, 8.5), 0.0));
    for _ in 0..100 {
        assert_eq!(
            ask(&mut game.server, &candidate(-40, 100.0), &mut ev),
            Some(zombie),
            "no hushjaw spawns within 32 blocks of another hushjaw"
        );
    }
    let (disabled, _, _) = game.mods_for_test().probe(0);
    assert!(!disabled, "the monsters mod stayed healthy through the asks");
}

#[test]
fn hushjaw_bites_a_silent_player_that_touches_it_via_wasm() {
    let Some(root) = crate::modding::tests::stage_monsters_fixture("hushjaw-bump") else {
        return;
    };
    crate::modding::tests::run_child_test(&root, "game::tests::monsters_mod::hushjaw_bump_inner");
}

/// Touch perception end to end: a player who never makes a sound but stands
/// OVERLAPPING the hushjaw is felt (the push pass records the contact), locked
/// by `chase_contact`, and bitten — the counterpart to the hearing test's
/// silent-at-a-distance phase, which stays safe.
#[test]
#[ignore = "spawned by hushjaw_bites_a_silent_player_that_touches_it_via_wasm with a fixture pack env"]
fn hushjaw_bump_inner() {
    use crate::block::Block;
    use crate::chunk::{Chunk, ChunkPos, CHUNK_SX, CHUNK_SZ};

    let hushjaw = crate::mob::defs()
        .iter()
        .position(|d| d.name == "monsters:hushjaw")
        .map(|i| crate::mob::Mob(i as u8))
        .expect("monsters:hushjaw registered from the fixture pack");

    let mut game =
        super::common::game_with_camera(Camera::new(Vec3::new(8.0, 66.0, 8.0), 16.0 / 9.0));
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
    // The hushjaw spawns overlapping the still, silent player: their combined
    // half-widths are 0.75, the horizontal gap 0.5.
    assert!(game
        .server
        .world
        .mobs_mut()
        .spawn(hushjaw, Vec3::new(8.5, 64.0, 8.0), 0.0));

    let mut ev = TickEvents::default();
    let h0 = game.server.sessions[0].player.health();
    let mut bitten_at = None;
    for tick in 0..60 {
        game.server.game_tick_step(&mut ev);
        if game.server.sessions[0].player.health() < h0 {
            bitten_at = Some(tick);
            break;
        }
    }
    let tick = bitten_at.expect("the touched hushjaw bites without ever hearing a sound");
    assert_eq!(
        h0 - game.server.sessions[0].player.health(),
        8,
        "the bite is the row's 8 half-hearts"
    );
    assert!(
        tick >= 2,
        "the touch takes the perception round-trip (contact → lock → strike), never tick 0"
    );
    let (disabled, _, _) = game.mods_for_test().probe(0);
    assert!(!disabled, "the monsters mod stayed healthy through the bump");
}

#[test]
fn hushjaw_hunts_footsteps_by_sound_and_ignores_a_silent_player_via_wasm() {
    let Some(root) = crate::modding::tests::stage_monsters_fixture("hushjaw-hunt") else {
        return;
    };
    crate::modding::tests::run_child_test(
        &root,
        "game::tests::monsters_mod::hushjaw_hearing_inner",
    );
}

/// The hushjaw's whole hunting loop, end to end in a real `Game`: a SILENT
/// player shares its floor unharmed; the moment the player's velocity reads as
/// audible footsteps, the noise seam feeds `chase_sound`, the hushjaw locks,
/// closes, and lands its 8-half-heart bite through the player damage funnel.
#[test]
#[ignore = "spawned by hushjaw_hunts_footsteps_by_sound_and_ignores_a_silent_player_via_wasm with a fixture pack env"]
fn hushjaw_hearing_inner() {
    use crate::block::Block;
    use crate::chunk::{Chunk, ChunkPos, CHUNK_SX, CHUNK_SZ};

    let hushjaw = crate::mob::defs()
        .iter()
        .position(|d| d.name == "monsters:hushjaw")
        .map(|i| crate::mob::Mob(i as u8))
        .expect("monsters:hushjaw registered from the fixture pack");

    let mut game =
        super::common::game_with_camera(Camera::new(Vec3::new(8.0, 66.0, 8.0), 16.0 / 9.0));
    // One 16×16 floor island: every point on it is within the hushjaw's
    // 12-block hearing of the centred player, and wander never leaves it
    // (destinations must be standable footholds).
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
    // The hushjaw starts across the island, 6+ blocks out, facing away.
    assert!(game
        .server
        .world
        .mobs_mut()
        .spawn(hushjaw, Vec3::new(14.5, 64.0, 8.5), 0.0));

    let mut ev = TickEvents::default();
    let h0 = game.server.sessions[0].player.health();

    // Phase 1 — a still player is inaudible: 150 ticks, no damage. Melee is
    // perception-gated, so even a wander that brushes the player bites
    // nothing while no noise ever locked them.
    for _ in 0..150 {
        game.server.game_tick_step(&mut ev);
    }
    assert_eq!(
        game.server.sessions[0].player.health(),
        h0,
        "a silent, still player is invisible to a blind hunter"
    );

    // Phase 2 — the player walks in place through the REAL input path: the
    // latched movement intent alternates direction every few ticks, so the
    // integrated speed is audible while the net drift stays tiny. The hushjaw
    // must lock on, cross the island, and land exactly its row's 8 half-hearts.
    let mut first_hit: Option<(i64, i32, f32)> = None;
    for tick in 0..400i64 {
        let dir = if (tick / 4) % 2 == 0 { 1.0 } else { -1.0 };
        game.server.sessions[0].move_wishdir = Vec3::new(dir, 0.0, 0.0);
        let before = game.server.sessions[0].player.health();
        game.server.game_tick_step(&mut ev);
        let after = game.server.sessions[0].player.health();
        if after < before {
            let mob_pos = game.server.world.mobs().instances()[0].pos;
            let gap = (mob_pos - game.server.sessions[0].player.pos).length();
            first_hit = Some((tick, before - after, gap));
            break;
        }
    }
    let (tick, drop, gap) = first_hit.expect("the walking player is hunted down and bitten");
    assert_eq!(
        drop, 8,
        "the bite lands the row's 8 half-hearts through the damage funnel"
    );
    assert!(
        gap < 4.0,
        "the bite landed in melee range (gap {gap}), not from across the island"
    );
    assert!(
        tick > 0,
        "the hunt takes time — the hushjaw crossed the island first"
    );
    let (disabled, _, _) = game.mods_for_test().probe(0);
    assert!(!disabled, "the monsters mod stayed healthy through the hunt");
}
