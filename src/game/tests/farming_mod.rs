//! Full-loop proof for the farming mod AND the Slice-1 engine seams it rides
//! on: name-addressed item use (the hoe through `item_use_pre` +
//! `ResolveItem`), contextual placeable food (the carrot planting vs eating),
//! `block_place_pre` planting validation, mod block behaviors (farmland
//! hydration reconcile, scheduled crop growth with the dry pause and
//! random-tick re-arming), `block_interact` right-click harvesting, and the
//! data-only recipe surface (catalyst flour, bucket-remainder dough,
//! `kitchen:cooking` baking, Well Fed damage mutation).
//!
//! Pack registration needs the fixture in the registry, so assertions run in
//! child processes (the established `PETRAMOND_MODS` re-spawn pattern).
//! Growth-delay numbers, yields, and effect durations are balance data — the
//! assertions pin BEHAVIOR (pauses, resets, ranges, ordering), not the
//! editable values.

use super::super::tick::TickEvents;
use crate::camera::Camera;
use crate::mathh::Vec3;

#[test]
fn farming_cultivation_grows_pauses_and_harvests_via_wasm() {
    let Some(root) =
        crate::modding::tests::stage_mods_fixture("farming-cultivation", &["kitchen", "farming"])
    else {
        return;
    };
    crate::modding::tests::run_child_test(
        &root,
        "game::tests::farming_mod::farming_cultivation_inner",
    );
}

#[test]
fn farming_processing_and_well_fed_via_wasm() {
    let Some(root) =
        crate::modding::tests::stage_mods_fixture("farming-processing", &["kitchen", "farming"])
    else {
        return;
    };
    crate::modding::tests::run_child_test(
        &root,
        "game::tests::farming_mod::farming_processing_inner",
    );
}

#[test]
fn farming_wild_patches_generate_deterministically_via_wasm() {
    let Some(root) =
        crate::modding::tests::stage_mods_fixture("farming-worldgen", &["kitchen", "farming"])
    else {
        return;
    };
    crate::modding::tests::run_child_test(
        &root,
        "game::tests::farming_mod::farming_worldgen_inner",
    );
}

#[test]
fn farming_is_dependency_disabled_without_kitchen() {
    let Some(root) = crate::modding::tests::stage_mods_fixture("farming-nokitchen", &["farming"])
    else {
        return;
    };
    crate::modding::tests::run_child_test(
        &root,
        "game::tests::farming_mod::farming_nokitchen_inner",
    );
}

#[test]
fn farming_rain_compost_and_fertile_soil_via_wasm() {
    let Some(root) =
        crate::modding::tests::stage_mods_fixture("farming-soil", &["kitchen", "farming"])
    else {
        return;
    };
    crate::modding::tests::run_child_test(&root, "game::tests::farming_mod::farming_soil_inner");
}

#[test]
fn farming_sheep_follow_the_wheat_lure_via_wasm() {
    let Some(root) =
        crate::modding::tests::stage_mods_fixture("farming-lure", &["kitchen", "farming"])
    else {
        return;
    };
    crate::modding::tests::run_child_test(&root, "game::tests::farming_mod::farming_lure_inner");
}

#[test]
fn farming_fertilized_grass_and_sapling_boost_via_wasm() {
    let Some(root) =
        crate::modding::tests::stage_mods_fixture("farming-landscape", &["kitchen", "farming"])
    else {
        return;
    };
    crate::modding::tests::run_child_test(
        &root,
        "game::tests::farming_mod::farming_landscape_inner",
    );
}

fn by_key(key: &str) -> crate::item::ItemType {
    crate::item::ItemType::by_key(key)
        .unwrap_or_else(|| panic!("{key} registered from the fixture packs"))
}

fn block_by_name(name: &str) -> crate::block::Block {
    crate::registry::names()
        .blocks
        .id(name)
        .map(crate::block::Block)
        .unwrap_or_else(|| panic!("block {name} registered from the fixture packs"))
}

fn at(game: &super::common::TestGame, x: i32, y: i32, z: i32) -> crate::block::Block {
    crate::block::Block::from_id(game.server.world.chunk_block(x, y, z))
}

/// One authoritative use click at `target`: stand the player in reach, select
/// `slot`, latch the look, queue the click, and run the tick that resolves it.
fn use_click(
    game: &mut super::common::TestGame,
    ev: &mut TickEvents,
    slot: u8,
    target: crate::mathh::IVec3,
    normal: crate::mathh::IVec3,
) {
    let sess = &mut game.server.sessions[0];
    sess.player.pos = Vec3::new(target.x as f32 + 0.5, 65.0, target.z as f32 - 1.5);
    sess.player.inventory.set_active(slot);
    sess.look = Some(super::common::hit(target, normal));
    game.server.queue_place_click_for_test(0);
    game.server.game_tick_step(ev);
}

/// The standard farming stage: a full-skylight grass-floor island with the
/// session player standing ready on it. Random ticks (the reconcile/growth
/// heartbeat) only run around a streaming anchor, which a direct-stepping
/// test must still arm itself (`set_load_target_for_test`).
fn farming_floor_game() -> super::common::TestGame {
    use crate::chunk::{SECTION_VOLUME, SKY_FULL};
    let mut game =
        super::common::game_with_camera(Camera::new(Vec3::new(8.0, 66.0, 8.0), 16.0 / 9.0));
    super::common::flat_floor_loaded_air(&mut game.server.world, crate::block::Block::Grass);
    let section = game
        .server
        .world
        .section_at_world_mut_for_test(0, 64, 0)
        .expect("floor section loaded");
    section.set_skylight(vec![SKY_FULL; SECTION_VOLUME].into());
    let sess = &mut game.server.sessions[0];
    sess.player.pos = Vec3::new(8.0, 64.0, 5.0);
    sess.player.vel = Vec3::ZERO;
    sess.player.on_ground = true;
    game
}

/// Runs ONLY in the child process spawned above (needs `PETRAMOND_MODS`).
#[test]
#[ignore = "spawned by farming_cultivation_grows_pauses_and_harvests_via_wasm with a fixture pack env"]
fn farming_cultivation_inner() {
    use crate::block::Block;
    use crate::chunk::{Chunk, ChunkPos, CHUNK_SX, CHUNK_SZ};
    use crate::item::ItemStack;
    use crate::mathh::IVec3;

    let hoe = by_key("farming:iron_hoe");
    let seeds = by_key("farming:wheat_seeds");
    let carrot = by_key("farming:carrot");
    let wheat_item = by_key("farming:wheat");
    let farmland_dry = block_by_name("farming:farmland_dry");
    let farmland_wet = block_by_name("farming:farmland_wet");
    let wheat: Vec<Block> = (0..4)
        .map(|i| block_by_name(&format!("farming:wheat_{i}")))
        .collect();
    let carrots: Vec<Block> = (0..4)
        .map(|i| block_by_name(&format!("farming:carrots_{i}")))
        .collect();
    let wild_wheat = block_by_name("farming:wild_wheat");

    let mut game =
        super::common::game_with_camera(Camera::new(Vec3::new(8.0, 66.0, 8.0), 16.0 / 9.0));
    assert_eq!(
        game.mods_for_test().loaded(),
        2,
        "kitchen + farming wasm both loaded"
    );
    game.server.world.clear_world();
    let cp = ChunkPos::new(0, 0);
    game.server.world.insert_empty_column_for_test(cp);
    let mut chunk = Chunk::new(0, 0);
    for z in 0..CHUNK_SZ {
        for x in 0..CHUNK_SX {
            chunk.set_block(x, 62, z, Block::Stone);
            chunk.set_block(x, 63, z, Block::Stone);
        }
    }
    // A grass field along z = 8, plus one enclosed water cell at (5, 63, 6):
    // everything the sim must tick stays >= SIM_READ_REACH cells inside the
    // one real chunk, or the stream-finality guard would drop the work at the
    // absent-neighbour frontier (a fixture artifact, not a farm rule).
    // Its four horizontal neighbours stay solid stone, so the source cannot
    // spread — a still irrigation "well" 2 cells from the wet farm spots.
    for x in 0..CHUNK_SX {
        chunk.set_block(x, 63, 8, Block::Grass);
    }
    chunk.set_block(8, 63, 5, Block::Grass); // pad for the wild crop
                                             // A sealed room over two farmland cells at z = 10 (interior light 0):
                                             // the crop light rules need real darkness inside the random-tick band.
    for x in 4..=7 {
        for z in 9..=11 {
            for y in 64..=65 {
                if z == 10 && (x == 5 || x == 6) {
                    continue; // the dark interior
                }
                chunk.set_block(x, y, z, Block::Stone);
            }
            chunk.set_block(x, 66, z, Block::Stone);
        }
    }
    chunk.set_block(5, 63, 10, farmland_dry); // stays empty: idle-reverts
    chunk.set_block(6, 63, 10, farmland_dry); // gets the doomed dark crop
    game.server.world.insert_chunk_for_test(cp, chunk);
    // Random ticks (the crop re-arm heartbeat) only run around a streaming
    // anchor; a direct-stepping test must arm it itself.
    game.server.world.set_load_target_for_test(0, 4, 0, 4);
    game.server.world.set_block_world(5, 63, 6, Block::Water);
    {
        // The fixture's cached skylight is full everywhere (the async light
        // pipeline never runs in a direct-stepping test): zero the sealed
        // room's cells by hand, the zombies-test pattern. The walls keep the
        // local write-relights from flooding light back in.
        use crate::chunk::{section_idx, SECTION_VOLUME, SKY_FULL};
        let section = game
            .server
            .world
            .section_at_world_mut_for_test(0, 64, 0)
            .expect("the y=64 section is loaded");
        let mut sky: Vec<u8> = match section.skylight_arc() {
            Some(a) => a.to_vec(),
            None => vec![SKY_FULL; SECTION_VOLUME],
        };
        for lx in 4..=7 {
            for lz in 9..=11 {
                for ly in 0..=2 {
                    sky[section_idx(lx, ly, lz)] = 0;
                }
            }
        }
        section.set_skylight(sky.into());
    }
    game.server.world.set_block_world(6, 64, 10, carrots[0]);

    let sess = &mut game.server.sessions[0];
    sess.player.pos = Vec3::new(4.0, 64.0, 5.0);
    sess.player.vel = Vec3::ZERO;
    sess.player.on_ground = true;
    sess.player.inventory.add(ItemStack::new(hoe, 1)); // slot 0
    sess.player.inventory.add(ItemStack::new(seeds, 8)); // slot 1
    sess.player.inventory.add(ItemStack::new(carrot, 8)); // slot 2

    let mut ev = TickEvents::default();

    // --- Tilling. Grass near the water tills straight to WET farmland; far
    // grass tills DRY. The hoe is identified by name through the generic
    // item-use seam — an eligible use consumes the click, replaces the block,
    // and drops nothing.
    let wet_a = IVec3::new(5, 63, 8); // 2 cells from water: hydrated
    let wet_b = IVec3::new(6, 63, 8);
    let dry_a = IVec3::new(10, 63, 8); // 5 cells away: outside the radius-4 rule
                                       // The one-shot echo asserts read a FRESH event buffer per click:
                                       // `TickEvents` accumulate across steps by design (production drains them
                                       // per pump; this test never does).
    let mut click_ev = TickEvents::default();
    use_click(&mut game, &mut click_ev, 0, wet_a, IVec3::Y);
    assert!(
        click_ev.player_at(0).used_unpredicted,
        "a consumed till click echoes the hand jab the client could not predict"
    );
    for pos in [wet_b, dry_a] {
        use_click(&mut game, &mut ev, 0, pos, IVec3::Y);
    }
    assert_eq!(
        at(&game, 5, 63, 8),
        farmland_wet,
        "tilling beside water starts wet"
    );
    assert_eq!(at(&game, 6, 63, 8), farmland_wet);
    assert_eq!(
        at(&game, 10, 63, 8),
        farmland_dry,
        "tilling far from water starts dry"
    );
    assert!(
        game.server.world.item_entities().is_empty(),
        "tilling drops no dirt/grass item"
    );

    // Hold-to-interact: HOLDING the use button repeats the whole use-click
    // ladder server-side — the hoe tills a fresh cell with NO new click,
    // through the same `item_use_pre` seam (the repeat cadence itself is
    // pinned in `game::tests::placement`).
    {
        let sess = &mut game.server.sessions[0];
        sess.look = Some(super::common::hit(IVec3::new(7, 63, 8), IVec3::Y));
        sess.intent_gameplay = true;
        sess.intent_use_held = true;
    }
    for _ in 0..12 {
        game.server.game_tick_step(&mut ev);
    }
    assert_eq!(
        at(&game, 7, 63, 8),
        farmland_wet,
        "a held use button tills with no new click"
    );
    {
        let sess = &mut game.server.sessions[0];
        sess.intent_use_held = false;
        sess.intent_gameplay = false;
    }

    // An ineligible target (stone) is a quiet no-op that keeps the hoe.
    use_click(&mut game, &mut ev, 0, IVec3::new(6, 63, 4), IVec3::Y);
    assert_eq!(at(&game, 6, 63, 4), Block::Stone);
    assert_eq!(
        game.server.sessions[0]
            .player
            .inventory
            .slot(0)
            .map(|s| s.item),
        Some(hoe),
        "the hoe is never consumed or altered"
    );

    // --- Torches never mount on farmland: its sunken top offers no complete
    // support face (the engine torch rule; floor AND wall mounts refuse).
    game.server.sessions[0]
        .player
        .inventory
        .add(ItemStack::new(crate::item::ItemType::Torch, 2));
    let torch_slot = (0..9)
        .find(|&i| {
            game.server.sessions[0]
                .player
                .inventory
                .slot(i)
                .is_some_and(|s| s.item == crate::item::ItemType::Torch)
        })
        .expect("torches on the hotbar") as u8;
    use_click(&mut game, &mut ev, torch_slot, wet_b, IVec3::Y);
    assert_eq!(
        at(&game, 6, 64, 8),
        Block::Air,
        "a torch cannot be placed on farmland"
    );
    assert_eq!(
        game.server.sessions[0]
            .player
            .inventory
            .slot(torch_slot as usize)
            .map(|s| s.count),
        Some(2),
        "the refused torch is kept"
    );

    // --- Planting. Seeds place a stage-0 crop on farmland (wet or dry) and
    // consume one seed; anywhere else the placement is refused and the seed
    // kept. The carrot is planting stock through the CONTEXTUAL PLACEABLE
    // FOOD rule: valid placement wins, no eating starts.
    use_click(&mut game, &mut ev, 1, wet_a, IVec3::Y);
    assert_eq!(
        at(&game, 5, 64, 8),
        wheat[0],
        "seeds plant wheat_0 above wet farmland"
    );
    use_click(&mut game, &mut ev, 1, dry_a, IVec3::Y);
    assert_eq!(
        at(&game, 10, 64, 8),
        wheat[0],
        "dry farmland accepts seeds too"
    );
    assert_eq!(
        game.server.sessions[0]
            .player
            .inventory
            .slot(1)
            .map(|s| s.count),
        Some(6),
        "one seed consumed per successful planting"
    );
    use_click(&mut game, &mut ev, 1, IVec3::new(6, 63, 4), IVec3::Y);
    assert_eq!(at(&game, 6, 64, 4), Block::Air, "seeds refuse non-farmland");
    assert_eq!(
        game.server.sessions[0]
            .player
            .inventory
            .slot(1)
            .map(|s| s.count),
        Some(6),
        "a refused placement keeps the seed"
    );

    // The CLIENT never ghosts a mod-registered block: its placement law
    // (`block_place_pre` — crops demand farmland) lives mod-side, invisible
    // to the replica. A ghost would flash a phantom crop on ANY surface and
    // roll back on the deny; the ladder classifies Plausible instead (jab
    // only, a real placement arrives unpredicted).
    game.sync_self_view_for_test();
    game.game.self_view.inventory.set_active(1);
    assert!(
        matches!(
            game.game
                .predict_place_at_for_test(IVec3::new(6, 63, 4), IVec3::Y, false),
            crate::game::tick::PlacePrediction::Plausible
        ),
        "a mod block never ghost-predicts"
    );

    // Darkness refuses planting outright (the light gate; the seed is kept).
    use_click(&mut game, &mut ev, 1, IVec3::new(5, 63, 10), IVec3::Y);
    assert_eq!(
        at(&game, 5, 64, 10),
        Block::Air,
        "planting on farmland in darkness quietly does nothing"
    );
    assert_eq!(
        game.server.sessions[0]
            .player
            .inventory
            .slot(1)
            .map(|s| s.count),
        Some(6),
        "the dark-refused seed is kept"
    );

    use_click(&mut game, &mut ev, 2, wet_b, IVec3::Y);
    assert_eq!(
        at(&game, 6, 64, 8),
        carrots[0],
        "a carrot plants on valid farmland"
    );
    assert!(
        game.server.sessions[0].eating.is_none(),
        "planting wins over eating for the dual-natured carrot"
    );
    assert_eq!(
        game.server.sessions[0]
            .player
            .inventory
            .slot(2)
            .map(|s| s.count),
        Some(7),
        "planting consumed one carrot"
    );

    // No valid placement -> the same click begins EATING the carrot.
    game.server.sessions[0].intent_use_held = true;
    use_click(&mut game, &mut ev, 2, IVec3::new(6, 63, 4), IVec3::Y);
    assert!(
        game.server.sessions[0].eating.is_some(),
        "with no valid placement the carrot eats instead"
    );
    let eat_ticks = carrot.food().expect("carrot is food").eat_ticks;
    for _ in 0..eat_ticks {
        game.server.game_tick_step(&mut ev);
    }
    game.server.sessions[0].intent_use_held = false;
    assert_eq!(
        game.server.sessions[0]
            .player
            .inventory
            .slot(2)
            .map(|s| s.count),
        Some(6),
        "the held eat consumed the carrot"
    );

    // A crop planted WITHOUT the placement event (worldgen-style install):
    // nothing armed its growth — the random-tick re-arm must adopt it.
    game.server.world.set_block_world(7, 63, 8, farmland_wet);
    game.server.world.set_block_world(7, 64, 8, wheat[0]);

    // --- Growth. Hydrated crops advance a stage after the jittered delay;
    // dry crops PAUSE (retrying, never dying, never regressing). Stage delay
    // is balance data — derive the horizon from "well past one max delay".
    game.server.sessions[0].player.pos = Vec3::new(8.0, 65.0, 5.0);
    for _ in 0..3800 {
        game.server.game_tick_step(&mut ev);
    }
    let stage_of = |b: Block, table: &[Block]| table.iter().position(|s| *s == b);
    assert!(
        stage_of(at(&game, 5, 64, 8), &wheat) >= Some(1),
        "hydrated wheat advanced past its first due attempt, got {:?}",
        at(&game, 5, 64, 8)
    );
    assert!(
        stage_of(at(&game, 6, 64, 8), &carrots) >= Some(1),
        "hydrated carrots advanced too"
    );
    assert_eq!(
        at(&game, 10, 64, 8),
        wheat[0],
        "the dry crop safely paused at stage 0 — dry never destroys or grows"
    );

    // Restoring water beside the dry farmland: the paused-but-ready crop
    // resumes on its short retry cadence (growth probes REAL hydration —
    // the farmland's wet look is random-tick based and lags on purpose).
    game.server.world.set_block_world(9, 63, 8, Block::Water); // enclosed by solid floor cells
    for _ in 0..310 {
        game.server.game_tick_step(&mut ev);
    }
    assert!(
        stage_of(at(&game, 10, 64, 8), &wheat) >= Some(1),
        "a ready crop resumes promptly once hydration returns, got {:?}",
        at(&game, 10, 64, 8)
    );

    // --- To maturity. Keep ticking until the first wheat is mature (bounded:
    // three transitions of at most ~120 s each, plus scheduling margins).
    let mut matured = false;
    for _ in 0..60 {
        for _ in 0..200 {
            game.server.game_tick_step(&mut ev);
        }
        if at(&game, 5, 64, 8) == wheat[3] {
            matured = true;
            break;
        }
    }
    assert!(
        matured,
        "hydrated wheat reaches maturity, got {:?}",
        at(&game, 5, 64, 8)
    );
    assert_eq!(
        at(&game, 10, 63, 8),
        farmland_wet,
        "the wet look reconciled through the block's own random ticks"
    );
    assert!(
        stage_of(at(&game, 7, 64, 8), &wheat) >= Some(1),
        "the event-less crop was re-armed by random ticks (reload can never freeze a crop)"
    );

    // --- Right-click harvest: produce pops as item entities, the plant
    // resets to stage 0 in the same tick, and the next cycle is armed.
    let drops_before = game.server.world.item_entities().len();
    let mut click_ev = TickEvents::default();
    use_click(&mut game, &mut click_ev, 0, IVec3::new(5, 64, 8), IVec3::Y);
    assert_eq!(
        at(&game, 5, 64, 8),
        wheat[0],
        "harvest retains the plant at stage 0"
    );
    assert!(
        click_ev.player_at(0).used_unpredicted,
        "the consumed harvest click echoes the hand jab too"
    );
    let harvested: u32 = game.server.world.item_entities()[drops_before..]
        .iter()
        .filter(|e| e.stack.item == wheat_item)
        .map(|e| e.stack.count as u32)
        .sum();
    assert!(
        (1..=2).contains(&harvested),
        "right-click harvest yields produce (1-2 wheat), got {harvested}"
    );

    // An immature crop consumes its click without harvesting, planting, or
    // eating — even with the dual-natured carrot in hand.
    let carrots_before = game.server.sessions[0]
        .player
        .inventory
        .slot(2)
        .map(|s| s.count);
    use_click(&mut game, &mut ev, 2, IVec3::new(5, 64, 8), IVec3::Y);
    assert_eq!(
        at(&game, 5, 64, 8),
        wheat[0],
        "immature wheat is not harvested or replaced"
    );
    assert!(
        game.server.sessions[0].eating.is_none(),
        "aiming at an immature crop never starts eating the carrot"
    );
    assert_eq!(
        game.server.sessions[0]
            .player
            .inventory
            .slot(2)
            .map(|s| s.count),
        carrots_before,
        "nor does it plant one"
    );

    // --- Wild crops never right-click harvest; breaking is the only take.
    game.server.world.set_block_world(8, 64, 5, wild_wheat);
    let drops_before = game.server.world.item_entities().len();
    let mut click_ev = TickEvents::default();
    use_click(&mut game, &mut click_ev, 0, IVec3::new(8, 64, 5), IVec3::Y);
    assert_eq!(
        at(&game, 8, 64, 5),
        wild_wheat,
        "a wild crop ignores right-click"
    );
    assert!(
        !click_ev.player_at(0).used_unpredicted,
        "an unconsumed click echoes nothing — no phantom jab"
    );
    assert_eq!(
        game.server.world.item_entities().len(),
        drops_before,
        "and yields nothing for it"
    );
    game.server.finish_player_break(
        0,
        crate::mining::BreakEvent {
            pos: IVec3::new(8, 64, 5),
            block: wild_wheat,
            harvested: true,
        },
        &mut ev,
        true,
    );
    let broken: Vec<_> = game.server.world.item_entities()[drops_before..]
        .iter()
        .map(|e| e.stack)
        .collect();
    let wild_grain: u32 = broken
        .iter()
        .filter(|s| s.item == wheat_item)
        .map(|s| s.count as u32)
        .sum();
    let wild_seeds: u32 = broken
        .iter()
        .filter(|s| s.item == seeds)
        .map(|s| s.count as u32)
        .sum();
    assert_eq!(wild_grain, 1, "wild wheat drops exactly 1 wheat");
    assert!(
        (1..=3).contains(&wild_seeds),
        "and 1-3 wheat seeds, got {wild_seeds}"
    );

    // --- Supporting-soil invalidation: replacing the farmland under a crop
    // (not even breaking it — stone is solid) pops the planting stock.
    let drops_before = game.server.world.item_entities().len();
    game.server.world.set_block_world(10, 63, 8, Block::Stone);
    for _ in 0..4 {
        game.server.game_tick_step(&mut ev);
    }
    assert_eq!(
        at(&game, 10, 64, 8),
        Block::Air,
        "a crop with its farmland replaced pops rather than floats"
    );
    let stock: u32 = game.server.world.item_entities()[drops_before..]
        .iter()
        .filter(|e| e.stack.item == seeds)
        .map(|e| e.stack.count as u32)
        .sum();
    assert_eq!(stock, 1, "the planting stock returns — never lost");

    // --- Building over farmland presses it back to dirt: break the carrot
    // crop, then place a stone into the freed cell above its farmland.
    game.server.finish_player_break(
        0,
        crate::mining::BreakEvent {
            pos: IVec3::new(6, 64, 8),
            block: at(&game, 6, 64, 8),
            harvested: true,
        },
        &mut ev,
        true,
    );
    game.server.sessions[0]
        .player
        .inventory
        .add(ItemStack::new(crate::item::ItemType::Stone, 4));
    let stone_slot = (0..9)
        .find(|&i| {
            game.server.sessions[0]
                .player
                .inventory
                .slot(i)
                .is_some_and(|s| s.item == crate::item::ItemType::Stone)
        })
        .expect("stone on the hotbar") as u8;
    use_click(
        &mut game,
        &mut ev,
        stone_slot,
        IVec3::new(6, 63, 8),
        IVec3::Y,
    );
    assert_eq!(at(&game, 6, 64, 8), Block::Stone, "the stone placed above");
    for _ in 0..3 {
        game.server.game_tick_step(&mut ev);
    }
    assert_eq!(
        at(&game, 6, 63, 8),
        Block::Dirt,
        "a non-crop block placed on farmland presses it back to dirt"
    );

    // --- Darkness aftermath: the sealed room's crop breaks on a dark random
    // tick (its planting stock pops), after which BOTH untended farmland
    // cells idle back to dirt — three cropless random ticks in a row, wet or
    // dry alike. Random-tick draws are sparse (3 per section per tick), so
    // wait bounded-until instead of assuming the maturity loop left enough
    // time.
    // (Reverted dirt may since have grown grass — the engine dirt ecology —
    // so "no longer farmland" accepts both.)
    let reverted = |b: Block| b == Block::Dirt || b == Block::Grass;
    for _ in 0..80 {
        if at(&game, 6, 64, 10) == Block::Air
            && reverted(at(&game, 6, 63, 10))
            && reverted(at(&game, 5, 63, 10))
        {
            break;
        }
        for _ in 0..200 {
            game.server.game_tick_step(&mut ev);
        }
    }
    assert_eq!(
        at(&game, 6, 64, 10),
        Block::Air,
        "a crop random-ticked in darkness breaks"
    );
    assert!(
        reverted(at(&game, 6, 63, 10)),
        "its farmland, cropless since, pressed back to dirt; got {:?}",
        at(&game, 6, 63, 10)
    );
    assert!(
        reverted(at(&game, 5, 63, 10)),
        "never-planted farmland idle-reverts to dirt; got {:?}",
        at(&game, 5, 63, 10)
    );

    // --- Water washes crops away as a NATURAL break (drops roll, nothing is
    // silently deleted): flood the harvested wheat and the wild pad.
    // The player must not vacuum the evidence: step well out of pickup range.
    game.server.sessions[0].player.pos = Vec3::new(14.0, 65.0, 14.0);
    let count_of = |game: &super::common::TestGame, item: crate::item::ItemType| -> u32 {
        game.server
            .world
            .item_entities()
            .iter()
            .filter(|e| e.stack.item == item)
            .map(|e| e.stack.count as u32)
            .sum()
    };
    let (wheat_before, seeds_before) = (count_of(&game, wheat_item), count_of(&game, seeds));
    // The wild pad was emptied by the break test above — regrow it for the wash.
    game.server.world.set_block_world(8, 64, 5, wild_wheat);
    game.server.world.set_block_world(5, 65, 8, Block::Water);
    game.server.world.set_block_world(8, 65, 5, Block::Water);
    for _ in 0..40 {
        game.server.game_tick_step(&mut ev);
    }
    assert_eq!(
        at(&game, 5, 64, 8),
        Block::Water,
        "flow claimed the crop cell"
    );
    assert_eq!(at(&game, 8, 64, 5), Block::Water, "and the wild crop cell");
    assert!(
        count_of(&game, wheat_item) > wheat_before,
        "washing wild wheat dropped its grain"
    );
    assert!(
        count_of(&game, seeds) > seeds_before,
        "and seeds — a wash breaks, it never deletes"
    );

    let (disabled, _, _) = game.mods_for_test().probe(1);
    assert!(!disabled, "the farming mod never trapped");
}

/// Runs ONLY in the child process spawned above (needs `PETRAMOND_MODS`).
#[test]
#[ignore = "spawned by farming_processing_and_well_fed_via_wasm with a fixture pack env"]
fn farming_processing_inner() {
    use crate::block::Block;
    use crate::events::DamageSource;
    use crate::item::{ItemStack, ItemType};

    let wheat = by_key("farming:wheat");
    let flour = by_key("farming:flour");
    let dough = by_key("farming:dough");
    let bread = by_key("farming:bread");
    let carrot = by_key("farming:carrot");
    let lunch = by_key("farming:farmers_lunch");

    // --- The data-only recipe surface.
    let recipes = crate::crafting::load_recipes();
    assert_eq!(
        recipes.process("kitchen:cooking", dough).map(|s| s.item),
        Some(bread),
        "dough bakes to bread under the kitchen's class"
    );
    assert_eq!(
        recipes.smelt(dough),
        None,
        "farming adds no petramond:smelting path — a furnace cannot bake it"
    );
    assert_eq!(
        recipes.process("kitchen:milling", wheat).map(|s| s.item),
        Some(flour),
        "wheat grinds to flour under the kitchen miller's class (one farming data row)"
    );
    assert_eq!(
        recipes.smelt(wheat),
        None,
        "and no furnace path for wheat either"
    );

    let mut game =
        super::common::game_with_camera(Camera::new(Vec3::new(8.0, 66.0, 8.0), 16.0 / 9.0));
    super::common::flat_floor_loaded_air(&mut game.server.world, Block::Stone);
    game.server.sessions[0].player.pos = Vec3::new(4.0, 64.0, 4.0);
    game.server.sessions[0].player.vel = Vec3::ZERO;
    game.server.sessions[0].player.on_ground = true;

    // --- Stable-key crafting at the pack's own station. The mod's recipes
    // join the same authoritative catalog as engine recipes; the farmer's
    // workbench is a pack-registered crafting station, admitted like the
    // engine pair — no grid or mod-specific path.
    {
        let workbench = crate::crafting::CraftingStation::from_key("farming:farmers_workbench")
            .expect("loading the pack's recipes registered its station");
        assert!(
            recipes
                .crafting()
                .get_at("farming:dough", crate::crafting::CraftingStation::CraftingTable)
                .is_none(),
            "farming craftables left the crafting table for the workbench"
        );
        assert!(
            recipes
                .crafting()
                .get_at(
                    "farming:farmers_workbench",
                    crate::crafting::CraftingStation::CraftingTable
                )
                .is_some(),
            "the workbench itself is a crafting-table recipe"
        );
        assert!(
            recipes
                .crafting()
                .get_at("petramond:oak_planks", workbench)
                .is_none(),
            "the workbench admits only its own tier — no inventory recipes (per Rachel)"
        );

        let sess = &mut game.server.sessions[0];
        let (menu, inv) = (&mut sess.menu, &mut sess.player.inventory);
        menu.open_crafting(crate::crafting::CraftingStation::Inventory);
        assert_eq!(
            menu.craft_recipe(inv, &recipes, "farming:dough", false),
            Err(crate::game::container::CraftMenuFailure::InvalidRecipe),
            "a workbench-tier recipe is refused away from the workbench"
        );
        menu.open_crafting(workbench);

        // --- Dough: aggregate quantities are consumed from inventory and the
        // water bucket's remainder is safely returned there.
        inv.add(ItemStack::new(wheat, 3));
        inv.add(ItemStack::new(ItemType::WaterBucket, 1));
        assert_eq!(
            menu.craft_recipe(inv, &recipes, "farming:dough", false),
            Ok(Vec::new())
        );
        assert_eq!(
            menu.craft_output().map(|s| (s.item, s.count)),
            Some((dough, 3)),
            "the name-addressed mod recipe produces one real output stack"
        );
        assert_eq!(
            inventory_count(inv, ItemType::WoodenBucket),
            1,
            "the water bucket's remainder returns to inventory"
        );
        menu.click(
            &mut game.server.world,
            inv,
            &recipes,
            crate::gui::MenuSlot::CraftResult,
            crate::controls::PointerButton::Primary,
            false,
            false,
        );
        assert!(menu.craft_output().is_none());
        assert_eq!(
            menu.craft_recipe(inv, &recipes, "farming:dough", false),
            Err(crate::game::container::CraftMenuFailure::MissingIngredients),
            "the returned empty bucket cannot satisfy another water-bucket craft"
        );
        inv.click_slot(30); // park the cursor stack somewhere out of the way

        // --- Farmer's lunch: 1 bread + 2 carrots.
        inv.add(ItemStack::new(bread, 1));
        inv.add(ItemStack::new(carrot, 2));
        assert_eq!(
            menu.craft_recipe(inv, &recipes, "farming:farmers_lunch", false),
            Ok(Vec::new())
        );
        menu.click(
            &mut game.server.world,
            inv,
            &recipes,
            crate::gui::MenuSlot::CraftResult,
            crate::controls::PointerButton::Primary,
            false,
            false,
        );
        assert_eq!(
            inv.cursor().map(|s| s.item),
            Some(lunch),
            "bread + 2 carrots make the Farmer's Lunch"
        );
        inv.click_slot(3); // park the lunch on the hotbar for the eat below
    }

    // --- The workbench block's `open_gui` kind runs the ordinary crafting
    // session (never a mod GUI session): the one seam that makes a pack
    // station a real station.
    {
        let workbench = crate::crafting::CraftingStation::from_key("farming:farmers_workbench")
            .expect("station registered");
        let kind = crate::gui::resolve_kind("farming:farmers_workbench")
            .expect("blocks.json interned the workbench GUI kind");
        game.server.sessions[0].pending_menu_actions.push(
            crate::server::player::PendingMenuAction::OpenGui { kind, pos: None },
        );
        let mut ev = TickEvents::default();
        game.server.tick_menu(0, &mut ev);
        assert_eq!(
            game.server.sessions[0].menu.crafting_station(),
            Some(workbench),
            "opening the workbench kind begins a crafting session at its station"
        );
        game.server.close_open_menu_for(0, &mut ev);
    }

    // --- Well Fed: eat the lunch, then route damage through the ordinary
    // player pipeline. Each positive instance is reduced by one half-heart,
    // never below one; the reduction only exists while the effect is active.
    let mut ev = TickEvents::default();
    game.server.sessions[0].player.set_health(20);
    game.server
        .damage_player(0, 6, DamageSource::Fall, None, &mut ev);
    assert_eq!(
        game.server.sessions[0].player.health(),
        14,
        "without Well Fed the full 6 applies"
    );

    game.server.sessions[0].player.inventory.set_active(3);
    assert_eq!(
        game.server.sessions[0]
            .player
            .inventory
            .selected()
            .map(|s| s.item),
        Some(lunch),
        "the crafted lunch is in hand"
    );
    game.server.sessions[0].look = None;
    game.server.sessions[0].intent_use_held = true;
    game.server.queue_place_click_for_test(0);
    let eat_ticks = lunch.food().expect("the lunch is food").eat_ticks;
    for _ in 0..(eat_ticks + 2) {
        game.server.game_tick_step(&mut ev);
    }
    game.server.sessions[0].intent_use_held = false;

    game.server.sessions[0].player.set_health(20);
    game.server
        .damage_player(0, 6, DamageSource::Fall, None, &mut ev);
    assert_eq!(
        game.server.sessions[0].player.health(),
        15,
        "Well Fed reduces a routed damage instance by one half-heart"
    );
    for _ in 0..crate::damage::PLAYER_DAMAGE_IFRAME_TICKS {
        game.server.sessions[0].player.tick_damage_immunity();
    }
    let before = game.server.sessions[0].player.health();
    game.server
        .damage_player(0, 1, DamageSource::Fall, None, &mut ev);
    assert_eq!(
        before - game.server.sessions[0].player.health(),
        1,
        "damage is never reduced below one half-heart"
    );

    let (disabled, _, _) = game.mods_for_test().probe(1);
    assert!(!disabled, "the farming mod never trapped");
}

#[test]
fn farming_trough_bucket_swap_via_wasm() {
    let Some(root) =
        crate::modding::tests::stage_mods_fixture("farming-trough", &["kitchen", "farming"])
    else {
        return;
    };
    crate::modding::tests::run_child_test(&root, "game::tests::farming_mod::farming_trough_inner");
}

/// Runs ONLY in the child process spawned above.
#[test]
#[ignore = "spawned by farming_trough_bucket_swap_via_wasm with a fixture pack env"]
fn farming_trough_inner() {
    use crate::block::Block;
    use crate::item::ItemStack;
    use crate::mathh::IVec3;

    let trough = block_by_name("farming:trough");
    let trough_filled = block_by_name("farming:trough_filled");
    let water_bucket = by_key("petramond:water_bucket");
    let wooden_bucket = by_key("petramond:wooden_bucket");

    let mut game =
        super::common::game_with_camera(Camera::new(Vec3::new(8.0, 66.0, 8.0), 16.0 / 9.0));
    super::common::flat_floor_loaded_air(&mut game.server.world, Block::Stone);
    game.server.sessions[0].player.pos = Vec3::new(4.0, 64.0, 4.0);
    game.server.sessions[0].player.vel = Vec3::ZERO;
    game.server.sessions[0].player.on_ground = true;

    // Place the trough on the floor at (5,64,5) — its [2,1,1] footprint covers
    // (5,64,5) and (6,64,5).
    let origin = IVec3::new(5, 64, 5);
    assert!(game.server.world.place_model_block(origin, trough));
    assert_eq!(at(&game, 5, 64, 5), trough);
    assert_eq!(at(&game, 6, 64, 5), trough);

    let mut ev = TickEvents::default();

    // Fill: water bucket → empty bucket + filled trough.
    game.server.sessions[0]
        .player
        .inventory
        .add(ItemStack::new(water_bucket, 1));
    game.server.sessions[0].player.inventory.set_active(0);
    use_click(&mut game, &mut ev, 0, origin, IVec3::Y);
    assert_eq!(
        at(&game, 5, 64, 5),
        trough_filled,
        "the trough block swaps to filled"
    );
    assert_eq!(
        game.server.sessions[0]
            .player
            .inventory
            .selected()
            .map(|s| s.item),
        Some(wooden_bucket),
        "the water bucket emptied in hand"
    );

    // Empty: empty bucket → water bucket + empty trough.
    use_click(&mut game, &mut ev, 0, origin, IVec3::Y);
    assert_eq!(at(&game, 5, 64, 5), trough, "the trough block swaps back to empty");
    assert_eq!(
        game.server.sessions[0]
            .player
            .inventory
            .selected()
            .map(|s| s.item),
        Some(water_bucket),
        "the empty bucket refilled in hand"
    );

    let (disabled, _, _) = game.mods_for_test().probe(1);
    assert!(!disabled, "the farming mod never trapped");
}

fn inventory_count(inventory: &crate::inventory::Inventory, item: crate::item::ItemType) -> u16 {
    inventory
        .raw_slots()
        .iter()
        .flatten()
        .filter(|stack| stack.item == item)
        .map(|stack| u16::from(stack.count))
        .sum()
}

/// Runs ONLY in the child process spawned above: with kitchen absent the
/// dependency cascade disables farming WHOLE — no wasm, no catalogs, no
/// partial load.
#[test]
#[ignore = "spawned by farming_is_dependency_disabled_without_kitchen with a fixture pack env"]
fn farming_nokitchen_inner() {
    assert!(
        !crate::item::ItemType::all()
            .iter()
            .any(|i| i.key().starts_with("farming:")),
        "no farming item registers while the kitchen dependency is missing"
    );
    assert!(
        crate::registry::names()
            .blocks
            .id("farming:farmland_dry")
            .is_none(),
        "no farming block registers either — the pack never half-loads"
    );
}

/// Runs ONLY in the child process spawned above. The wild-patch worldgen
/// feature: patches root on ordinary grass above the waterline, and the
/// whole decision is a pure function of (seed, position) — the same chunk
/// regenerates byte-identically. Densities AND the per-crop biome whitelists
/// are pack balance data (declared inside the mod's own worldgen), so they
/// stay deliberately unpinned; the counts pinned are only "patches exist".
#[test]
#[ignore = "spawned by farming_wild_patches_generate_deterministically_via_wasm with a fixture pack env"]
fn farming_worldgen_inner() {
    use crate::block::Block;
    use crate::chunk::{CHUNK_SX, CHUNK_SY, CHUNK_SZ};

    // A full Game load installs the mod worldgen hooks process-wide.
    let game = super::common::game_with_camera(Camera::new(Vec3::new(0.0, 80.0, 0.0), 16.0 / 9.0));
    assert_eq!(game.mods_for_test().loaded(), 2);
    let wild_wheat = block_by_name("farming:wild_wheat");
    let wild_carrots = block_by_name("farming:wild_carrots");
    let wild_potatoes = block_by_name("farming:wild_potatoes");

    let seed = 42u32;
    // Order candidate chunks straight off the climate graph (no chunk
    // generation): one macro sample per chunk over a +-64-chunk square. The
    // per-crop biome whitelists are the PACK's balance data (mod-internal
    // worldgen code) and deliberately unpinned here, so the biome names below
    // are a scan-ordering HEURISTIC only — chunks in today's crop biomes
    // generate first so the break-early triggers fast, with every remaining
    // chunk queued behind them. A pack biome rebalance shifts scan time,
    // never pass/fail.
    let side = 129usize;
    let map = crate::tooling::worldgen::macro_surface_map(seed, side, 16);
    let half = side as i32 / 2;
    let likely = ["plains", "savanna", "forest", "redwood_forest"];
    let mut candidates: Vec<(i32, i32)> = Vec::new();
    let mut fallback: Vec<(i32, i32)> = Vec::new();
    for gz in 0..side {
        for gx in 0..side {
            let b = crate::biome::Biome::from_id(map.biomes[gz * side + gx]).name();
            let list = if likely.contains(&b) {
                &mut candidates
            } else {
                &mut fallback
            };
            list.push((gx as i32 - half, gz as i32 - half));
        }
    }
    candidates.extend(fallback);

    let (mut wheat_cells, mut carrot_cells, mut potato_cells) = (0u32, 0u32, 0u32);
    let mut first_hit: Option<(i32, i32)> = None;
    for &(cx, cz) in &candidates {
        let chunk = crate::worldgen::generate_chunk(seed, cx, cz);
        for z in 0..CHUNK_SZ {
            for x in 0..CHUNK_SX {
                for y in 1..CHUNK_SY {
                    let b = Block::from_id(chunk.block_raw(x, y, z));
                    let (wheat, carrot, potato) =
                        (b == wild_wheat, b == wild_carrots, b == wild_potatoes);
                    if !wheat && !carrot && !potato {
                        continue;
                    }
                    assert_eq!(
                        Block::from_id(chunk.block_raw(x, y - 1, z)),
                        Block::Grass,
                        "a wild crop roots only on ordinary grass"
                    );
                    assert!(
                        y as i32 - 1 > crate::chunk::SEA_LEVEL,
                        "patches root above the waterline"
                    );
                    // WHICH biomes each crop lands in is the pack's own
                    // declaration — balance data, deliberately not asserted.
                    if wheat {
                        wheat_cells += 1;
                    } else if carrot {
                        carrot_cells += 1;
                    } else {
                        potato_cells += 1;
                    }
                    first_hit.get_or_insert((cx, cz));
                }
            }
        }
        if wheat_cells >= 4 && carrot_cells >= 3 && potato_cells >= 2 {
            break;
        }
    }
    assert!(
        wheat_cells >= 4 && carrot_cells >= 3 && potato_cells >= 2,
        "purposeful scanning finds all wild crops \
         (wheat {wheat_cells}, carrots {carrot_cells}, potatoes {potato_cells})"
    );

    // Pure function of (seed, position): the same chunk regenerates
    // byte-identically, wild patches included.
    let (cx, cz) = first_hit.expect("a crop-bearing chunk was found");
    let a = crate::worldgen::generate_chunk(seed, cx, cz);
    let b = crate::worldgen::generate_chunk(seed, cx, cz);
    for y in 0..CHUNK_SY {
        for z in 0..CHUNK_SZ {
            for x in 0..CHUNK_SX {
                assert_eq!(
                    a.block_raw(x, y, z),
                    b.block_raw(x, y, z),
                    "worldgen with the farming feature is deterministic at ({x},{y},{z})"
                );
            }
        }
    }
}

/// Hand-encode a `weather:field` row (layout owned by weather-core's
/// `FieldRow`; farming is a READER — the test plays the weather mod's role).
/// Storm 2.0 saturates both cloud sheets — rain 1.0 at EVERY column; storm
/// 0.0 is a guaranteed clear sky. Field values stay deliberately unpinned.
fn field_row(storm: f32, clock: u64) -> Vec<u8> {
    // Pinned to `weather_core::FieldRow::encode` (the engine test crate
    // cannot depend on the mods-src crate): LE, clock u64 at 0, then eight
    // 4-byte lanes — storm is lane 2. A weather-core layout change edits
    // these three consts and nothing else.
    const ROW_LEN: usize = 40; // FieldRow::ENCODED_LEN
    const CLOCK_OFFSET: usize = 0;
    const STORM_OFFSET: usize = 8 + 2 * 4;
    let mut b = vec![0u8; ROW_LEN];
    b[CLOCK_OFFSET..CLOCK_OFFSET + 8].copy_from_slice(&clock.to_le_bytes());
    b[STORM_OFFSET..STORM_OFFSET + 4].copy_from_slice(&storm.to_le_bytes());
    b
}

/// Runs ONLY in the child process spawned above. The 2026-07 soil additions
/// end to end: rain hydrates open-sky farmland through the `weather:field`
/// interop row (freshness-gated), compostables fill the barrel to a
/// fertilizer, fertilizer upgrades plain farmland to FERTILE with the skin
/// pair preserved through the wet/dry reconcile, and fertile soil accepts
/// planting and grows under rain alone.
#[test]
#[ignore = "spawned by farming_rain_compost_and_fertile_soil_via_wasm with a fixture pack env"]
fn farming_soil_inner() {
    use crate::block::Block;
    use crate::item::ItemStack;
    use crate::mathh::IVec3;

    let hoe = by_key("farming:iron_hoe");
    let seeds = by_key("farming:wheat_seeds");
    let carrot = by_key("farming:carrot");
    let potato = by_key("farming:potato");
    let fertilizer = by_key("farming:fertilizer");
    let barrel_item = by_key("farming:compost_barrel");
    let farmland_dry = block_by_name("farming:farmland_dry");
    let farmland_wet = block_by_name("farming:farmland_wet");
    let fertile_dry = block_by_name("farming:farmland_fertile_dry");
    let fertile_wet = block_by_name("farming:farmland_fertile_wet");
    let compost: Vec<Block> = (0..4)
        .map(|i| block_by_name(&format!("farming:compost_{i}")))
        .collect();
    let wheat: Vec<Block> = (0..4)
        .map(|i| block_by_name(&format!("farming:wheat_{i}")))
        .collect();
    let potatoes_0 = block_by_name("farming:potatoes_0");

    // (The potato -> baked-potato pair is a `kitchen:cooking` data row —
    // balance data, deliberately unpinned; the cross-pack process-class seam
    // is covered in `farming_processing_inner`.)

    let mut game = farming_floor_game();
    game.server.world.set_load_target_for_test(0, 4, 0, 4);

    let sess = &mut game.server.sessions[0];
    sess.player.inventory.add(ItemStack::new(hoe, 1)); // slot 0
    sess.player.inventory.add(ItemStack::new(seeds, 8)); // slot 1
    sess.player.inventory.add(ItemStack::new(carrot, 8)); // slot 2
    sess.player.inventory.add(ItemStack::new(barrel_item, 1)); // slot 3
    sess.player.inventory.add(ItemStack::new(potato, 2)); // slot 4

    let mut ev = TickEvents::default();
    // One tick so core day/night publishes `petramond:clock` — the row's
    // freshness stamp must track it.
    game.server.game_tick_step(&mut ev);
    let set_weather = |game: &mut super::common::TestGame, storm: f32, lag: u64| {
        let clock = game
            .server
            .world
            .mod_kv_get("petramond:clock")
            .and_then(|b| b.try_into().ok().map(u64::from_le_bytes))
            .expect("core day/night publishes petramond:clock");
        game.server
            .world
            .mod_kv_set("weather:field".to_owned(), field_row(storm, clock - lag));
    };

    // --- Rain hydration. NO water exists anywhere on this map: under a
    // fresh saturated rain row, open-sky grass tills straight to WET.
    set_weather(&mut game, 2.0, 0);
    use_click(&mut game, &mut ev, 0, IVec3::new(5, 63, 8), IVec3::Y);
    assert_eq!(
        at(&game, 5, 63, 8),
        farmland_wet,
        "rain overhead hydrates tilled soil with no ground water at all"
    );

    // A STALE row (stamp trailing the clock beyond the tolerance) reads as
    // no weather — the same rain field tills DRY.
    set_weather(&mut game, 2.0, 1000);
    use_click(&mut game, &mut ev, 0, IVec3::new(7, 63, 8), IVec3::Y);
    assert_eq!(
        at(&game, 7, 63, 8),
        farmland_dry,
        "a stale weather row is a clear sky (uninstalled-mod leftovers can't rain)"
    );

    // --- Compost. Place the barrel, feed it three compostables (carrots),
    // and watch the fill stage rise — stage identity is the block id.
    use_click(&mut game, &mut ev, 3, IVec3::new(11, 63, 8), IVec3::Y);
    let barrel = IVec3::new(11, 64, 8);
    assert_eq!(at(&game, 11, 64, 8), compost[0], "the barrel places empty");
    for fill in 1..=3u8 {
        use_click(&mut game, &mut ev, 2, barrel, IVec3::Y);
        assert_eq!(
            at(&game, 11, 64, 8),
            compost[fill as usize],
            "one compostable advances one fill stage"
        );
    }
    assert_eq!(
        game.server.sessions[0]
            .player
            .inventory
            .slot(2)
            .map(|s| s.count),
        Some(5),
        "three carrots went into the barrel"
    );

    // Any click on the FULL barrel pops one fertilizer and resets it; the
    // player on the barrel top vacuums the pop.
    use_click(&mut game, &mut ev, 7, barrel, IVec3::Y); // empty hand slot
    assert_eq!(
        at(&game, 11, 64, 8),
        compost[0],
        "collecting resets the barrel to empty"
    );
    game.server.sessions[0].player.pos = Vec3::new(11.5, 65.05, 8.5);
    game.server.sessions[0].player.on_ground = true;
    let mut got_fertilizer = false;
    for _ in 0..80 {
        game.server.game_tick_step(&mut ev);
        if (0..36).any(|i| {
            game.server.sessions[0]
                .player
                .inventory
                .slot(i)
                .is_some_and(|s| s.item == fertilizer)
        }) {
            got_fertilizer = true;
            break;
        }
    }
    assert!(got_fertilizer, "the popped fertilizer was picked up");
    let fert_slot = (0..9)
        .find(|&i| {
            game.server.sessions[0]
                .player
                .inventory
                .slot(i)
                .is_some_and(|s| s.item == fertilizer)
        })
        .expect("fertilizer on the hotbar") as u8;

    // --- Fertile soil. Fertilizer upgrades the DRY plain farmland in
    // place, consuming one unit; seeds then plant on it like any farmland.
    use_click(
        &mut game,
        &mut ev,
        fert_slot,
        IVec3::new(7, 63, 8),
        IVec3::Y,
    );
    assert_eq!(
        at(&game, 7, 63, 8),
        fertile_dry,
        "fertilizer upgrades plain dry farmland to fertile dry"
    );
    assert!(
        !(0..9).any(|i| {
            game.server.sessions[0]
                .player
                .inventory
                .slot(i)
                .is_some_and(|s| s.item == fertilizer)
        }),
        "the applied fertilizer was consumed"
    );
    use_click(&mut game, &mut ev, 1, IVec3::new(7, 63, 8), IVec3::Y);
    assert_eq!(
        at(&game, 7, 64, 8),
        wheat[0],
        "seeds plant on fertile farmland"
    );

    // A potato is contextual placeable food on farmland, like the carrot.
    set_weather(&mut game, 2.0, 0);
    use_click(&mut game, &mut ev, 0, IVec3::new(9, 63, 8), IVec3::Y);
    use_click(&mut game, &mut ev, 4, IVec3::new(9, 63, 8), IVec3::Y);
    assert_eq!(
        at(&game, 9, 64, 8),
        potatoes_0,
        "a potato plants on farmland"
    );

    // --- Under sustained rain (row re-published every tick, like the real
    // weather mod), the fertile cell's look reconciles to fertile WET — the
    // wet/dry swap preserves the soil grade — and the planted wheat GROWS on
    // rain alone (growth probes real hydration, which now includes rain).
    let mut ok = false;
    for _ in 0..90 {
        for _ in 0..200 {
            set_weather(&mut game, 2.0, 0);
            game.server.game_tick_step(&mut ev);
        }
        let soil_wet = at(&game, 7, 63, 8) == fertile_wet;
        let grown = wheat
            .iter()
            .position(|s| *s == at(&game, 7, 64, 8))
            .is_some_and(|stage| stage >= 1);
        if soil_wet && grown {
            ok = true;
            break;
        }
    }
    assert!(
        ok,
        "rain-fed fertile soil turns fertile-wet and its crop grows; soil {:?}, crop {:?}",
        at(&game, 7, 63, 8),
        at(&game, 7, 64, 8)
    );

    let (disabled, _, _) = game.mods_for_test().probe(1);
    assert!(!disabled, "the farming mod never trapped");
}

/// Runs ONLY in the child process spawned above. The wheat lure end to end
/// through the whole new seam chain: the pack's `brain_extensions` row on the
/// ENGINE sheep, the scripted AI node dispatch, and the extended AiNodeCtx
/// facts (held item + player foothold). A sheep beyond the OLD lure range
/// walks to a wheat-holding player and STOPS at the stand-off ring instead
/// of crowding them; the player straying past the follow radius breaks the
/// follow and the sheep refuses to re-follow until its 200–300-tick sulk
/// expires. Radii are balance data — the assertions band around them
/// loosely (reached the ring, never pressed in, refused while sulking).
#[test]
#[ignore = "spawned by farming_sheep_follow_the_wheat_lure_via_wasm with a fixture pack env"]
fn farming_lure_inner() {
    use crate::item::ItemStack;

    let wheat_item = by_key("farming:wheat");
    let mut game = farming_floor_game();
    assert_eq!(game.mods_for_test().loaded(), 2, "kitchen + farming loaded");

    let sess = &mut game.server.sessions[0];
    sess.player.pos = Vec3::new(8.5, 64.0, 8.5);
    sess.player.inventory.add(ItemStack::new(wheat_item, 8)); // slot 0
    sess.player.inventory.set_active(0);

    // 6.5 blocks out: beyond the pre-2026-07-17 5-block lure, inside the
    // 8-block one.
    assert!(game.server.world.mobs_mut().spawn(
        crate::mob::Mob::Sheep,
        Vec3::new(15.0, 64.0, 8.5),
        0.0
    ));
    let id = game.server.world.mobs().instances()[0].id();
    let sheep_pos = |game: &super::common::TestGame| -> Vec3 {
        game.server
            .world
            .mobs()
            .instances()
            .iter()
            .find(|m| m.id() == id)
            .map(|m| m.pos)
            .expect("the sheep lives")
    };
    let dist = |game: &super::common::TestGame| -> f32 {
        let p = game.server.sessions[0].player.pos;
        let s = sheep_pos(game);
        Vec3::new(s.x - p.x, 0.0, s.z - p.z).length()
    };

    // --- Lured: wheat in hand, 6.5 blocks away — the sheep closes in but
    // STOPS at the stand-off ring rather than pressing into the player.
    let mut ev = TickEvents::default();
    let mut closest = f32::MAX;
    for _ in 0..400 {
        game.server.game_tick_step(&mut ev);
        closest = closest.min(dist(&game));
    }
    assert!(
        closest < 3.5,
        "a wheat-holding player lures the sheep to the stand-off ring, closest {closest}"
    );
    assert!(
        closest > 1.5,
        "the sheep stops short instead of crowding the player, closest {closest}"
    );
    let settled = dist(&game);
    assert!(
        settled < 4.0,
        "the stopped sheep stays at the lure, settled {settled}"
    );

    // --- Broken: the player strides far past the follow radius. The follow
    // breaks and arms the sulk.
    game.server.sessions[0].player.pos = Vec3::new(30.5, 64.0, 8.5);
    for _ in 0..20 {
        game.server.game_tick_step(&mut ev);
    }

    // --- Sulking: wheat still in hand, five blocks away (inside the lure,
    // outside the stand-off ring), but inside the refusal window (< 200
    // ticks) the sheep will not walk in.
    let s = sheep_pos(&game);
    game.server.sessions[0].player.pos = Vec3::new(s.x + 5.0, 64.0, s.z);
    let mut closest = f32::MAX;
    for _ in 0..150 {
        game.server.game_tick_step(&mut ev);
        closest = closest.min(dist(&game));
    }
    assert!(
        closest > 3.5,
        "a freshly sulking sheep refuses the lure, closest {closest}"
    );

    // --- Forgiven: past the 200–300-tick sulk the same lure works again —
    // the sheep closes from five blocks back to the stand-off ring. Keep the
    // player pinned near the sheep's current spot and wait it out.
    let mut lured_again = false;
    for _ in 0..8 {
        let s = sheep_pos(&game);
        game.server.sessions[0].player.pos = Vec3::new(s.x + 5.0, 64.0, s.z);
        for _ in 0..100 {
            game.server.game_tick_step(&mut ev);
        }
        if dist(&game) < 3.5 {
            lured_again = true;
            break;
        }
    }
    assert!(lured_again, "the sulk expires and the lure works again");

    let (disabled, _, _) = game.mods_for_test().probe(1);
    assert!(!disabled, "the farming mod never trapped");
}

/// Runs ONLY in the child process spawned above. Fertilizer's landscaping
/// uses end to end: fertilizer flips grass to the fertilized block (one unit
/// consumed), a flower rooted on it spreads a copy to nearby grass over
/// random ticks, the fertility is SPENT after a bounded number of random
/// ticks (back to plain grass), the hoe still tills the fertilized block —
/// into PLAIN farmland — and fertilizer on a sapling jumps it to its last
/// growth stage exactly once: the second click falls through and keeps the
/// unit. Chances, radii, and tick counts are balance data — the assertions
/// pin behavior (spread happens, revert happens, one charge per boost).
#[test]
#[ignore = "spawned by farming_fertilized_grass_and_sapling_boost_via_wasm with a fixture pack env"]
fn farming_landscape_inner() {
    use crate::block::Block;
    use crate::item::ItemStack;
    use crate::mathh::IVec3;

    let hoe = by_key("farming:iron_hoe");
    let fertilizer = by_key("farming:fertilizer");
    let poppy_item = by_key("petramond:poppy");
    let sapling_item = by_key("petramond:oak_sapling");
    let grass_fertilized = block_by_name("farming:grass_fertilized");
    let farmland_dry = block_by_name("farming:farmland_dry");

    let mut game = farming_floor_game();
    game.server.world.set_load_target_for_test(0, 4, 0, 4);

    let sess = &mut game.server.sessions[0];
    sess.player.inventory.add(ItemStack::new(hoe, 1)); // slot 0
    sess.player.inventory.add(ItemStack::new(fertilizer, 8)); // slot 1
    sess.player.inventory.add(ItemStack::new(poppy_item, 1)); // slot 2
    sess.player.inventory.add(ItemStack::new(sapling_item, 1)); // slot 3

    let mut ev = TickEvents::default();
    fn fert_count(game: &super::common::TestGame) -> u8 {
        game.server.sessions[0]
            .player
            .inventory
            .slot(1)
            .map_or(0, |s| s.count)
    }

    // --- Fertilized grass. A poppy rooted on grass, fertilizer on the soil
    // under it: the block flips to the fertilized variant, one unit spent.
    use_click(&mut game, &mut ev, 2, IVec3::new(8, 63, 8), IVec3::Y);
    assert_eq!(at(&game, 8, 64, 8), Block::Poppy, "the poppy roots on grass");
    use_click(&mut game, &mut ev, 1, IVec3::new(8, 63, 8), IVec3::Y);
    assert_eq!(
        at(&game, 8, 63, 8),
        grass_fertilized,
        "fertilizer flips grass to the fertilized block"
    );
    assert_eq!(fert_count(&game), 7, "one unit fertilized the grass");

    // Over random ticks the rooted poppy spreads a copy onto nearby grass
    // (the flat floor offers valid targets in every direction).
    fn spread_poppies(game: &super::common::TestGame) -> usize {
        let mut n = 0;
        for dz in -7..=7 {
            for dx in -7..=7 {
                if (dx, dz) != (0, 0) && at(game, 8 + dx, 64, 8 + dz) == Block::Poppy {
                    n += 1;
                }
            }
        }
        n
    }
    // Crank the heartbeat: a natural draw hits one given cell ~3 times per
    // 4096 world ticks, which would tune these loops to tens of thousands of
    // full game ticks against unstated balance odds. Instead enqueue the SAME
    // hook the world's random draw would (the block's own behavior seam —
    // `WasmBehavior::random_tick` queues, the next `game_tick_step` drains it
    // to the mod), one per step, so the mod's spread roll and spend count run
    // unmodified at a testable rate. The bounds assume only "a handful of
    // boosted ticks per roll", never the exact chance or spend count.
    let soil = IVec3::new(8, 63, 8);
    let mut spread = false;
    for _ in 0..500 {
        let b = at(&game, 8, 63, 8);
        if b == grass_fertilized {
            b.behavior().random_tick(&mut game.server.world, soil);
        } else if b == Block::Grass {
            // The rare path: the fertility spent all its ticks before any
            // spread roll landed — re-fertilize like a player would rather
            // than flake (counts below are all relative).
            use_click(&mut game, &mut ev, 1, soil, IVec3::Y);
            continue;
        }
        game.server.game_tick_step(&mut ev);
        if spread_poppies(&game) > 0 {
            spread = true;
            break;
        }
    }
    assert!(spread, "a rooted flower spreads to nearby grass");

    // The fertility is a bounded investment: the block relaxes back to plain
    // grass, and the spread stops with it.
    let mut spent = false;
    for _ in 0..500 {
        let b = at(&game, 8, 63, 8);
        if b == Block::Grass {
            spent = true;
            break;
        }
        if b == grass_fertilized {
            b.behavior().random_tick(&mut game.server.world, soil);
        }
        game.server.game_tick_step(&mut ev);
    }
    assert!(spent, "spent fertility relaxes back to plain grass");
    // The negative at the same cranked rate: the reverted block's random
    // ticks run the ENGINE grass ecology, not the mod hook — no more spread.
    let settled = spread_poppies(&game);
    for _ in 0..50 {
        at(&game, 8, 63, 8)
            .behavior()
            .random_tick(&mut game.server.world, soil);
        game.server.game_tick_step(&mut ev);
    }
    assert_eq!(
        spread_poppies(&game),
        settled,
        "plain grass spreads nothing further"
    );

    // --- The hoe still tills a fertilized block — into PLAIN farmland (the
    // fertility was the spreading kind, not the soil upgrade).
    use_click(&mut game, &mut ev, 1, IVec3::new(4, 63, 4), IVec3::Y);
    assert_eq!(at(&game, 4, 63, 4), grass_fertilized);
    use_click(&mut game, &mut ev, 0, IVec3::new(4, 63, 4), IVec3::Y);
    assert_eq!(
        at(&game, 4, 63, 4),
        farmland_dry,
        "tilling fertilized grass yields plain farmland"
    );

    // --- Sapling boost: one click jumps the sapling to its last growth
    // stage — observable as a plain block swap to the final stage row —
    // charging exactly one unit; a second click changes nothing and keeps
    // the fertilizer (never waste a unit on an already-final sapling).
    let final_oak = block_by_name("petramond:oak_sapling_2");
    use_click(&mut game, &mut ev, 3, IVec3::new(12, 63, 4), IVec3::Y);
    let sap = IVec3::new(12, 64, 4);
    assert_eq!(at(&game, 12, 64, 4), Block::OakSapling, "the sapling plants");
    let before_count = fert_count(&game);
    use_click(&mut game, &mut ev, 1, sap, IVec3::Y);
    assert_eq!(
        at(&game, 12, 64, 4),
        final_oak,
        "fertilizer jumps the sapling to its final stage row"
    );
    assert_eq!(
        fert_count(&game),
        before_count - 1,
        "the boost charged one unit"
    );
    use_click(&mut game, &mut ev, 1, sap, IVec3::Y);
    assert_eq!(
        fert_count(&game),
        before_count - 1,
        "a second click on the boosted sapling keeps the unit"
    );
    assert_eq!(
        at(&game, 12, 64, 4),
        final_oak,
        "the boost never grows the tree by itself"
    );

    let (disabled, _, _) = game.mods_for_test().probe(1);
    assert!(!disabled, "the farming mod never trapped");
}
