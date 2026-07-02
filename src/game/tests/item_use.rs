use super::super::tick::TickEvents;
use super::super::Game;
use super::common::{game, install_empty_chunk};
use crate::block::Block;
use crate::inventory::Inventory;
use crate::item::{ItemStack, ItemType};
use crate::mathh::{IVec3, Vec3};

fn holding(item: ItemType) -> Inventory {
    let mut inv = Inventory::new();
    inv.add(ItemStack::new(item, 1));
    inv
}

/// Aim the camera straight down from two blocks above the top face of `cell`.
fn aim_down_at(game: &mut Game, cell: IVec3) {
    game.cam.pos = Vec3::new(cell.x as f32 + 0.5, cell.y as f32 + 3.0, cell.z as f32 + 0.5);
    game.cam.pitch = -std::f32::consts::FRAC_PI_2;
}

fn right_click(game: &mut Game) -> TickEvents {
    game.pending_place = true;
    let mut events = TickEvents::default();
    game.tick_place(&mut events);
    events
}

/// A stone shelf at `y` so poured water can spread a flowing ring on it. Wide
/// enough (and fully inside the one loaded test chunk) that no shelf edge is
/// within the flow's slope-search range of [`SHELF_CENTER`]: water on it spreads
/// symmetrically instead of chasing the nearest drop-off.
fn stone_shelf(game: &mut Game, y: i32) {
    for x in 2..=14 {
        for z in 2..=14 {
            game.world.set_block_world(x, y, z, Block::Stone);
        }
    }
}

/// Where shelf tests put their water source: the middle of [`stone_shelf`].
const SHELF_CENTER: IVec3 = IVec3::new(8, 78, 8);

/// Run enough fixed world ticks for at least one water flow step.
fn run_water_ticks(game: &mut Game, n: u32) {
    for _ in 0..n {
        game.world.game_tick(&game.recipes);
    }
}

#[test]
fn filling_the_bucket_scoops_the_source_and_swaps_the_held_item() {
    let mut game = game();
    install_empty_chunk(&mut game);
    game.player.inventory = holding(ItemType::WoodenBucket);

    // A still source right under the eye. The normal look ray sees through
    // water (nothing solid below in the empty chunk), so this exercises the
    // bucket's own water-stopping ray.
    let p = IVec3::new(0, 78, 0);
    assert!(game.world.set_block_world(p.x, p.y, p.z, Block::Water));
    aim_down_at(&mut game, p);

    let events = right_click(&mut game);

    assert!(events.used_item, "fill should report an item use");
    assert!(events.placed_block.is_none());
    assert_eq!(
        Block::from_id(game.world.chunk_block(p.x, p.y, p.z)),
        Block::Air,
        "the source should be scooped out of the world"
    );
    assert_eq!(
        game.player.inventory.selected().unwrap().item,
        ItemType::WaterBucket
    );
}

#[test]
fn filling_while_aiming_at_flowing_water_does_nothing() {
    let mut game = game();
    install_empty_chunk(&mut game);
    game.player.inventory = holding(ItemType::WoodenBucket);

    // A source spread into a flowing ring on a stone shelf. Flowing water is
    // TRANSPARENT to the fill ray: aiming straight down at a ring cell reads
    // through it to the shelf beneath, so nothing is picked up — the fill never
    // acts on flow, and never searches the body for a source.
    stone_shelf(&mut game, 77);
    let src = SHELF_CENTER;
    game.world.set_block_world(src.x, src.y, src.z, Block::Water);
    run_water_ticks(&mut game, 30);
    let flow = src + IVec3::X;
    assert_eq!(
        Block::from_id(game.world.chunk_block(flow.x, flow.y, flow.z)),
        Block::Water,
        "the source should have spread onto the shelf"
    );
    assert!(!game.world.is_water_source_world(flow));

    aim_down_at(&mut game, flow);
    let events = right_click(&mut game);

    assert!(!events.used_item, "flowing water must not fill the bucket");
    assert!(
        game.world.is_water_source_world(src),
        "the source elsewhere in the body must be untouched"
    );
    assert_eq!(
        game.player.inventory.selected().unwrap().item,
        ItemType::WoodenBucket
    );
}

#[test]
fn fill_ray_reads_through_flowing_water_to_the_source_behind_it() {
    let mut game = game();
    install_empty_chunk(&mut game);
    game.player.inventory = holding(ItemType::WoodenBucket);

    // The bug this pins: a spread sheet or thin film renders exactly like still
    // water, so if it STOPPED the fill ray it would invisibly shadow the source
    // the player is aiming at. Aim at the source through its own flowing ring
    // at a shallow angle — the ray must pass the ring cells and scoop the source.
    stone_shelf(&mut game, 77);
    let src = SHELF_CENTER;
    game.world.set_block_world(src.x, src.y, src.z, Block::Water);
    run_water_ticks(&mut game, 30);
    assert!(!game.world.is_water_source_world(src + IVec3::X));
    assert!(!game.world.is_water_source_world(src + IVec3::X * 2));

    game.cam.pos = Vec3::new(src.x as f32 + 3.3, 79.5, src.z as f32 + 0.5);
    let target = Vec3::new(src.x as f32 + 0.5, 78.4, src.z as f32 + 0.5);
    let dir = target - game.cam.pos;
    game.cam.yaw = dir.x.atan2(dir.z);
    game.cam.pitch = (dir.y / dir.length()).asin();

    let events = right_click(&mut game);

    assert!(events.used_item, "the source behind the flow must be scooped");
    assert_eq!(
        Block::from_id(game.world.chunk_block(src.x, src.y, src.z)),
        Block::Air
    );
    assert_eq!(
        game.player.inventory.selected().unwrap().item,
        ItemType::WaterBucket
    );
}

#[test]
fn filling_needs_a_source_within_reach() {
    let mut game = game();
    install_empty_chunk(&mut game);
    game.player.inventory = holding(ItemType::WoodenBucket);

    // Water well below REACH (eye ~9 blocks above the surface).
    let p = IVec3::new(0, 70, 0);
    assert!(game.world.set_block_world(p.x, p.y, p.z, Block::Water));
    game.cam.pos = Vec3::new(0.5, 80.0, 0.5);
    game.cam.pitch = -std::f32::consts::FRAC_PI_2;

    let events = right_click(&mut game);

    assert!(!events.used_item);
    assert_eq!(
        Block::from_id(game.world.chunk_block(p.x, p.y, p.z)),
        Block::Water,
        "out-of-reach water must stay"
    );
    assert_eq!(
        game.player.inventory.selected().unwrap().item,
        ItemType::WoodenBucket
    );
}

#[test]
fn pouring_places_a_source_against_the_clicked_face_and_empties_the_bucket() {
    let mut game = game();
    install_empty_chunk(&mut game);
    game.player.inventory = holding(ItemType::WaterBucket);

    let floor = IVec3::new(3, 64, 3);
    game.world
        .set_block_world(floor.x, floor.y, floor.z, Block::Stone);
    aim_down_at(&mut game, floor);

    let events = right_click(&mut game);

    let cell = floor + IVec3::Y;
    assert!(events.used_item, "pour should report an item use");
    assert!(events.placed_block.is_none());
    assert!(
        game.world.is_water_source_world(cell),
        "the clicked face's cell should hold a still source"
    );
    assert_eq!(
        game.player.inventory.selected().unwrap().item,
        ItemType::WoodenBucket,
        "the emptied bucket returns to the hand"
    );
}

#[test]
fn pouring_onto_flowing_water_firms_it_into_a_source() {
    let mut game = game();
    install_empty_chunk(&mut game);
    game.player.inventory = holding(ItemType::WaterBucket);

    stone_shelf(&mut game, 77);
    let src = SHELF_CENTER;
    game.world.set_block_world(src.x, src.y, src.z, Block::Water);
    run_water_ticks(&mut game, 30);
    let flow = src + IVec3::X;
    assert!(!game.world.is_water_source_world(flow));

    // The pour ray stops at the water surface: the flowing cell itself is what
    // receives the source — not the shelf beneath it.
    aim_down_at(&mut game, flow);
    let events = right_click(&mut game);

    assert!(events.used_item, "pouring into water must work");
    assert!(
        game.world.is_water_source_world(flow),
        "the flowing cell firms into a still source"
    );
    assert_eq!(
        game.player.inventory.selected().unwrap().item,
        ItemType::WoodenBucket
    );
}

#[test]
fn pouring_onto_a_source_still_empties_the_bucket() {
    let mut game = game();
    install_empty_chunk(&mut game);
    game.player.inventory = holding(ItemType::WaterBucket);

    // Pouring into already-still water changes nothing in the world, but the
    // action stays predictable: on water, the bucket always empties.
    let p = IVec3::new(0, 78, 0);
    game.world.set_block_world(p.x, p.y, p.z, Block::Water);
    aim_down_at(&mut game, p);

    let events = right_click(&mut game);

    assert!(events.used_item);
    assert!(game.world.is_water_source_world(p));
    assert_eq!(
        game.player.inventory.selected().unwrap().item,
        ItemType::WoodenBucket
    );
}

#[test]
fn pouring_with_nothing_in_reach_keeps_the_water() {
    let mut game = game();
    install_empty_chunk(&mut game);
    game.player.inventory = holding(ItemType::WaterBucket);

    // Nothing but air below the eye: the pour ray finds no cell to fill.
    game.cam.pos = Vec3::new(0.5, 80.0, 0.5);
    game.cam.pitch = -std::f32::consts::FRAC_PI_2;

    let events = right_click(&mut game);

    assert!(!events.used_item);
    assert_eq!(
        game.player.inventory.selected().unwrap().item,
        ItemType::WaterBucket,
        "a refused pour must keep the water in the bucket"
    );
}
