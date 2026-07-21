use super::super::tick::TickEvents;
use super::common::game_on_empty_chunk;
use crate::block::Block;
use crate::inventory::Inventory;
use crate::item::{ItemStack, ItemType};
use crate::mathh::{IVec3, Vec3};

fn holding(item: ItemType) -> Inventory {
    let mut inv = Inventory::new();
    inv.add(ItemStack::new(item, 1));
    inv
}

/// Aim the player straight down with the eye two blocks above the top face of
/// `cell` (bucket rays cast from the player's eye along the player's look).
fn aim_down_at(game: &mut super::common::TestGame, cell: IVec3) {
    set_player_eye(
        game,
        Vec3::new(
            cell.x as f32 + 0.5,
            cell.y as f32 + 3.0,
            cell.z as f32 + 0.5,
        ),
    );
    game.server.sessions[0].player.pitch = -std::f32::consts::FRAC_PI_2;
}

/// Place the player so their EYE sits exactly at `eye`.
fn set_player_eye(game: &mut super::common::TestGame, eye: Vec3) {
    game.server.sessions[0].player.pos = Vec3::new(eye.x, eye.y - crate::player::EYE, eye.z);
}

fn right_click(game: &mut super::common::TestGame) -> TickEvents {
    game.server.queue_place_click_for_test(0);
    let mut events = TickEvents::default();
    game.server.tick_place(0, &mut events);
    events
}

/// Latch a use click targeting the mob at `index`, as an
/// `Action(UseClick { mob })` message does — carrying the STABLE id.
fn right_click_at_mob(game: &mut super::common::TestGame, index: usize) -> TickEvents {
    let id = game.server.world.mobs().instances()[index].id();
    super::common::aim_server_at_mob(game, index);
    game.server.queue_mob_use_click_for_test(0, id);
    let mut events = TickEvents::default();
    game.server.tick_place(0, &mut events);
    events
}

/// A stone shelf at `y` so poured water can spread a flowing ring on it. Wide
/// enough (and fully inside the one loaded test chunk) that no shelf edge is
/// within the flow's slope-search range of [`SHELF_CENTER`]: water on it spreads
/// symmetrically instead of chasing the nearest drop-off.
fn stone_shelf(game: &mut super::common::TestGame, y: i32) {
    for x in 2..=14 {
        for z in 2..=14 {
            game.server.world.set_block_world(x, y, z, Block::Stone);
        }
    }
}

/// Where shelf tests put their water source: the middle of [`stone_shelf`].
const SHELF_CENTER: IVec3 = IVec3::new(8, 78, 8);

/// Run enough fixed world ticks for at least one water flow step.
fn run_water_ticks(game: &mut super::common::TestGame, n: u32) {
    for _ in 0..n {
        game.server.world.game_tick(&game.server.recipes);
    }
}

#[test]
fn filling_the_bucket_scoops_the_source_and_swaps_the_held_item() {
    let mut game = game_on_empty_chunk();
    game.server.sessions[0].player.inventory = holding(ItemType::WoodenBucket);

    // A still source right under the eye. The normal look ray sees through
    // water (nothing solid below in the empty chunk), so this exercises the
    // bucket's own water-stopping ray.
    let p = IVec3::new(0, 78, 0);
    assert!(game
        .server
        .world
        .set_block_world(p.x, p.y, p.z, Block::Water));
    aim_down_at(&mut game, p);

    let events = right_click(&mut game);

    assert!(
        events.player_at(0).used_item,
        "fill should report an item use"
    );
    assert!(events.player_at(0).placed_block.is_none());
    assert_eq!(
        Block::from_id(game.server.world.chunk_block(p.x, p.y, p.z)),
        Block::Air,
        "the source should be scooped out of the world"
    );
    assert_eq!(
        game.server.sessions[0]
            .player
            .inventory
            .selected()
            .unwrap()
            .item,
        ItemType::WaterBucket
    );
}

#[test]
fn filling_while_aiming_at_flowing_water_does_nothing() {
    let mut game = game_on_empty_chunk();
    game.server.sessions[0].player.inventory = holding(ItemType::WoodenBucket);

    // A source spread into a flowing ring on a stone shelf. Flowing water is
    // TRANSPARENT to the fill ray: aiming straight down at a ring cell reads
    // through it to the shelf beneath, so nothing is picked up — the fill never
    // acts on flow, and never searches the body for a source.
    stone_shelf(&mut game, 77);
    let src = SHELF_CENTER;
    game.server
        .world
        .set_block_world(src.x, src.y, src.z, Block::Water);
    run_water_ticks(&mut game, 30);
    let flow = src + IVec3::X;
    assert_eq!(
        Block::from_id(game.server.world.chunk_block(flow.x, flow.y, flow.z)),
        Block::Water,
        "the source should have spread onto the shelf"
    );
    assert!(!game.server.world.is_water_source_world(flow));

    aim_down_at(&mut game, flow);
    let events = right_click(&mut game);

    assert!(
        !events.player_at(0).used_item,
        "flowing water must not fill the bucket"
    );
    assert!(
        game.server.world.is_water_source_world(src),
        "the source elsewhere in the body must be untouched"
    );
    assert_eq!(
        game.server.sessions[0]
            .player
            .inventory
            .selected()
            .unwrap()
            .item,
        ItemType::WoodenBucket
    );
}

#[test]
fn fill_ray_reads_through_flowing_water_to_the_source_behind_it() {
    let mut game = game_on_empty_chunk();
    game.server.sessions[0].player.inventory = holding(ItemType::WoodenBucket);

    // The bug this pins: a spread sheet or thin film renders exactly like still
    // water, so if it STOPPED the fill ray it would invisibly shadow the source
    // the player is aiming at. Aim at the source through its own flowing ring
    // at a shallow angle — the ray must pass the ring cells and scoop the source.
    stone_shelf(&mut game, 77);
    let src = SHELF_CENTER;
    game.server
        .world
        .set_block_world(src.x, src.y, src.z, Block::Water);
    run_water_ticks(&mut game, 30);
    assert!(!game.server.world.is_water_source_world(src + IVec3::X));
    assert!(!game.server.world.is_water_source_world(src + IVec3::X * 2));

    set_player_eye(
        &mut game,
        Vec3::new(src.x as f32 + 3.3, 79.5, src.z as f32 + 0.5),
    );
    let target = Vec3::new(src.x as f32 + 0.5, 78.4, src.z as f32 + 0.5);
    let dir = target - game.server.sessions[0].player.eye();
    game.server.sessions[0].player.yaw = dir.x.atan2(dir.z);
    game.server.sessions[0].player.pitch = (dir.y / dir.length()).asin();

    let events = right_click(&mut game);

    assert!(
        events.player_at(0).used_item,
        "the source behind the flow must be scooped"
    );
    assert_eq!(
        Block::from_id(game.server.world.chunk_block(src.x, src.y, src.z)),
        Block::Air
    );
    assert_eq!(
        game.server.sessions[0]
            .player
            .inventory
            .selected()
            .unwrap()
            .item,
        ItemType::WaterBucket
    );
}

#[test]
fn filling_needs_a_source_within_reach() {
    let mut game = game_on_empty_chunk();
    game.server.sessions[0].player.inventory = holding(ItemType::WoodenBucket);

    // Water well below REACH (eye ~9 blocks above the surface).
    let p = IVec3::new(0, 70, 0);
    assert!(game
        .server
        .world
        .set_block_world(p.x, p.y, p.z, Block::Water));
    set_player_eye(&mut game, Vec3::new(0.5, 80.0, 0.5));
    game.server.sessions[0].player.pitch = -std::f32::consts::FRAC_PI_2;

    let events = right_click(&mut game);

    assert!(!events.player_at(0).used_item);
    assert_eq!(
        Block::from_id(game.server.world.chunk_block(p.x, p.y, p.z)),
        Block::Water,
        "out-of-reach water must stay"
    );
    assert_eq!(
        game.server.sessions[0]
            .player
            .inventory
            .selected()
            .unwrap()
            .item,
        ItemType::WoodenBucket
    );
}

#[test]
fn pouring_places_a_source_against_the_clicked_face_and_empties_the_bucket() {
    let mut game = game_on_empty_chunk();
    game.server.sessions[0].player.inventory = holding(ItemType::WaterBucket);

    let floor = IVec3::new(3, 64, 3);
    game.server
        .world
        .set_block_world(floor.x, floor.y, floor.z, Block::Stone);
    aim_down_at(&mut game, floor);

    let events = right_click(&mut game);

    let cell = floor + IVec3::Y;
    assert!(
        events.player_at(0).used_item,
        "pour should report an item use"
    );
    assert!(events.player_at(0).placed_block.is_none());
    assert!(
        game.server.world.is_water_source_world(cell),
        "the clicked face's cell should hold a still source"
    );
    assert_eq!(
        game.server.sessions[0]
            .player
            .inventory
            .selected()
            .unwrap()
            .item,
        ItemType::WoodenBucket,
        "the emptied bucket returns to the hand"
    );
}

#[test]
fn pouring_onto_flowing_water_firms_it_into_a_source() {
    let mut game = game_on_empty_chunk();
    game.server.sessions[0].player.inventory = holding(ItemType::WaterBucket);

    stone_shelf(&mut game, 77);
    let src = SHELF_CENTER;
    game.server
        .world
        .set_block_world(src.x, src.y, src.z, Block::Water);
    run_water_ticks(&mut game, 30);
    let flow = src + IVec3::X;
    assert!(!game.server.world.is_water_source_world(flow));

    // The pour ray stops at the water surface: the flowing cell itself is what
    // receives the source — not the shelf beneath it.
    aim_down_at(&mut game, flow);
    let events = right_click(&mut game);

    assert!(
        events.player_at(0).used_item,
        "pouring into water must work"
    );
    assert!(
        game.server.world.is_water_source_world(flow),
        "the flowing cell firms into a still source"
    );
    assert_eq!(
        game.server.sessions[0]
            .player
            .inventory
            .selected()
            .unwrap()
            .item,
        ItemType::WoodenBucket
    );
}

#[test]
fn pouring_onto_a_source_still_empties_the_bucket() {
    let mut game = game_on_empty_chunk();
    game.server.sessions[0].player.inventory = holding(ItemType::WaterBucket);

    // Pouring into already-still water changes nothing in the world, but the
    // action stays predictable: on water, the bucket always empties.
    let p = IVec3::new(0, 78, 0);
    game.server
        .world
        .set_block_world(p.x, p.y, p.z, Block::Water);
    aim_down_at(&mut game, p);

    let events = right_click(&mut game);

    assert!(events.player_at(0).used_item);
    assert!(game.server.world.is_water_source_world(p));
    assert_eq!(
        game.server.sessions[0]
            .player
            .inventory
            .selected()
            .unwrap()
            .item,
        ItemType::WoodenBucket
    );
}

#[test]
fn pouring_with_nothing_in_reach_keeps_the_water() {
    let mut game = game_on_empty_chunk();
    game.server.sessions[0].player.inventory = holding(ItemType::WaterBucket);

    // Nothing but air below the eye: the pour ray finds no cell to fill.
    set_player_eye(&mut game, Vec3::new(0.5, 80.0, 0.5));
    game.server.sessions[0].player.pitch = -std::f32::consts::FRAC_PI_2;

    let events = right_click(&mut game);

    assert!(!events.player_at(0).used_item);
    assert_eq!(
        game.server.sessions[0]
            .player
            .inventory
            .selected()
            .unwrap()
            .item,
        ItemType::WaterBucket,
        "a refused pour must keep the water in the bucket"
    );
}

#[test]
fn shearing_the_targeted_sheep_drops_wool_and_strips_the_coat() {
    let mut game = game_on_empty_chunk();
    game.server.sessions[0].player.inventory = holding(ItemType::Shears);
    assert!(game.server.world.mobs_mut().spawn(
        crate::mob::Mob::Sheep,
        Vec3::new(8.0, 64.0, 8.0),
        0.0
    ));
    let events = right_click_at_mob(&mut game, 0);

    assert!(
        events.player_at(0).used_item,
        "shearing reports an item use"
    );
    assert!(
        game.server.world.mobs().instances()[0].is_shorn(),
        "the sheep is shorn"
    );
    let spec = crate::mob::def(crate::mob::Mob::Sheep)
        .shear
        .expect("sheep are shearable");
    let wool: Vec<_> = game
        .server
        .world
        .item_entities()
        .iter()
        .filter(|d| d.stack.item == ItemType::Wool)
        .collect();
    assert_eq!(wool.len(), 1, "one wool stack pops at the sheep");
    assert!(
        (spec.min..=spec.max).contains(&wool[0].stack.count),
        "count is rolled from the spec range: {}",
        wool[0].stack.count
    );

    // A shorn sheep refuses a second shear until the coat regrows.
    let events = right_click_at_mob(&mut game, 0);
    assert!(
        !events.player_at(0).used_item,
        "no double-shear while shorn"
    );
}

#[test]
fn shearing_needs_the_shears_in_hand() {
    let mut game = game_on_empty_chunk();
    game.server.sessions[0].player.inventory = holding(ItemType::Dirt);
    assert!(game.server.world.mobs_mut().spawn(
        crate::mob::Mob::Sheep,
        Vec3::new(8.0, 64.0, 8.0),
        0.0
    ));
    let events = right_click_at_mob(&mut game, 0);

    assert!(!events.player_at(0).used_item);
    assert!(
        !game.server.world.mobs().instances()[0].is_shorn(),
        "a bare right-click leaves the coat alone"
    );
}

#[test]
fn a_forged_out_of_reach_mob_id_cannot_interact_or_shear() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    let mut game = game_on_empty_chunk();
    game.server.sessions[0].player.inventory = holding(ItemType::Shears);
    assert!(game.server.world.mobs_mut().spawn(
        crate::mob::Mob::Sheep,
        Vec3::new(8.0, 200.0, 8.0),
        0.0
    ));
    let mob = &game.server.world.mobs().instances()[0];
    let size = crate::mob::def(mob.kind).size;
    let target = mob.pos + Vec3::Y * (size.height * 0.5);
    let half_length = size.half_length.unwrap_or(size.half_width);
    super::common::set_server_view(
        &mut game,
        target - Vec3::Z * (crate::player::REACH + half_length + 2.0),
        Vec3::Z,
    );

    let dispatched = Arc::new(AtomicBool::new(false));
    let observed = dispatched.clone();
    game.server.bus.on_interact_attempt(0, move |_, ev| {
        // The forged id must be scrubbed BEFORE mods observe the attempt —
        // and with no block target either, the dispatch never fires at all.
        observed.store(ev.mob.is_some(), Ordering::Relaxed);
        crate::events::Outcome::Cancel
    });
    let id = game.server.world.mobs().instances()[0].id();
    game.server.queue_mob_use_click_for_test(0, id);
    let mut events = TickEvents::default();
    game.server.tick_place(0, &mut events);

    assert!(
        !dispatched.load(Ordering::Relaxed),
        "authority rejects the forged id before the mod attempt dispatch"
    );
    assert!(!events.player_at(0).interacted);
    assert!(
        !game.server.world.mobs().instances()[0].is_shorn(),
        "the same validator protects the engine mob-use fallback"
    );
}
