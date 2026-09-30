use super::common::game_on_empty_chunk;
use petramond::events::tick::TickEvents;
use petramond_math::math::{IVec3, Vec3};
use petramond_math::world_pos::WorldPos;
use petramond_world::block::Block;
use petramond_world::inventory::Inventory;
use petramond_world::item::{ItemStack, ItemType};

fn holding(item: ItemType) -> Inventory {
    let mut inv = Inventory::new();
    inv.add(ItemStack::new(item, 1));
    inv
}

fn aim_down_at(game: &mut super::common::TestGame, cell: IVec3) {
    set_player_eye(
        game,
        Vec3::new(
            cell.x as f32 + 0.5,
            cell.y as f32 + 3.0,
            cell.z as f32 + 0.5,
        ),
    );
    game.server_player_mut().pitch = -std::f32::consts::FRAC_PI_2;
}

fn set_player_eye(game: &mut super::common::TestGame, eye: Vec3) {
    game.server_player_mut().pos = WorldPos::new(
        f64::from(eye.x),
        f64::from(eye.y - petramond::player::EYE),
        f64::from(eye.z),
    );
}

fn right_click(game: &mut super::common::TestGame) -> TickEvents {
    game.sim_mut().queue_place_click_for_test(0);
    let mut events = TickEvents::default();
    game.sim_mut().tick_place(0, &mut events);
    events
}

fn right_click_at_mob(game: &mut super::common::TestGame, index: usize) -> TickEvents {
    let id = game.server_world().mobs().instances()[index].id();
    super::common::aim_server_at_mob(game, index);
    game.sim_mut().queue_mob_use_click_for_test(0, id);
    let mut events = TickEvents::default();
    game.sim_mut().tick_place(0, &mut events);
    events
}

fn stone_shelf(game: &mut super::common::TestGame, y: i32) {
    for x in 2..=14 {
        for z in 2..=14 {
            game.server_world_mut()
                .set_block_world(x, y, z, Block::Stone);
        }
    }
}

const SHELF_CENTER: IVec3 = IVec3::new(8, 78, 8);

fn run_water_ticks(game: &mut super::common::TestGame, n: u32) {
    for _ in 0..n {
        game.server_world_tick();
    }
}

#[test]
fn filling_the_bucket_scoops_the_source_and_swaps_the_held_item() {
    let mut game = game_on_empty_chunk();
    game.server_player_mut().inventory = holding(ItemType::WoodenBucket);

    let p = IVec3::new(0, 78, 0);
    assert!(game
        .server_world_mut()
        .set_block_world(p.x, p.y, p.z, Block::Water));
    aim_down_at(&mut game, p);

    let events = right_click(&mut game);

    assert!(
        events.player_at(0).used_item,
        "fill should report an item use"
    );
    assert!(events.player_at(0).placed_block.is_none());
    assert_eq!(
        Block::from_id(game.server_world().data().chunk_block(p.x, p.y, p.z)),
        Block::Air,
        "the source should be scooped out of the world"
    );
    assert_eq!(
        game.server_player().inventory.selected().unwrap().item,
        ItemType::WaterBucket
    );
}

#[test]
fn filling_while_aiming_at_flowing_water_does_nothing() {
    let mut game = game_on_empty_chunk();
    game.server_player_mut().inventory = holding(ItemType::WoodenBucket);

    stone_shelf(&mut game, 77);
    let src = SHELF_CENTER;
    game.server_world_mut()
        .set_block_world(src.x, src.y, src.z, Block::Water);
    run_water_ticks(&mut game, 30);
    let flow = src + IVec3::X;
    assert_eq!(
        Block::from_id(
            game.server_world()
                .data()
                .chunk_block(flow.x, flow.y, flow.z)
        ),
        Block::Water,
        "the source should have spread onto the shelf"
    );
    assert!(!game.server_world().is_water_source_world(flow));

    aim_down_at(&mut game, flow);
    let events = right_click(&mut game);

    assert!(
        !events.player_at(0).used_item,
        "flowing water must not fill the bucket"
    );
    assert!(
        game.server_world().is_water_source_world(src),
        "the source elsewhere in the body must be untouched"
    );
    assert_eq!(
        game.server_player().inventory.selected().unwrap().item,
        ItemType::WoodenBucket
    );
}

#[test]
fn fill_ray_reads_through_flowing_water_to_the_source_behind_it() {
    let mut game = game_on_empty_chunk();
    game.server_player_mut().inventory = holding(ItemType::WoodenBucket);

    // A spread sheet or thin film renders exactly like still water, so if it
    // stopped the fill ray it would invisibly shadow the source
    // the player is aiming at. Aim at the source through its own flowing ring
    // at a shallow angle — the ray must pass the ring cells and scoop the source.
    stone_shelf(&mut game, 77);
    let src = SHELF_CENTER;
    game.server_world_mut()
        .set_block_world(src.x, src.y, src.z, Block::Water);
    run_water_ticks(&mut game, 30);
    assert!(!game.server_world().is_water_source_world(src + IVec3::X));
    assert!(!game
        .server_world()
        .is_water_source_world(src + IVec3::X * 2));

    set_player_eye(
        &mut game,
        Vec3::new(src.x as f32 + 3.3, 79.5, src.z as f32 + 0.5),
    );
    let target =
        petramond_math::world_pos::WorldPos::new(src.x as f64 + 0.5, 78.4, src.z as f64 + 0.5);
    let dir = target - game.server_player().eye();
    game.server_player_mut().yaw = dir.x.atan2(dir.z);
    game.server_player_mut().pitch = (dir.y / dir.length()).asin();

    let events = right_click(&mut game);

    assert!(
        events.player_at(0).used_item,
        "the source behind the flow must be scooped"
    );
    assert_eq!(
        Block::from_id(game.server_world().data().chunk_block(src.x, src.y, src.z)),
        Block::Air
    );
    assert_eq!(
        game.server_player().inventory.selected().unwrap().item,
        ItemType::WaterBucket
    );
}

#[test]
fn filling_needs_a_source_within_reach() {
    let mut game = game_on_empty_chunk();
    game.server_player_mut().inventory = holding(ItemType::WoodenBucket);

    let p = IVec3::new(0, 70, 0);
    assert!(game
        .server_world_mut()
        .set_block_world(p.x, p.y, p.z, Block::Water));
    set_player_eye(&mut game, Vec3::new(0.5, 80.0, 0.5));
    game.server_player_mut().pitch = -std::f32::consts::FRAC_PI_2;

    let events = right_click(&mut game);

    assert!(!events.player_at(0).used_item);
    assert_eq!(
        Block::from_id(game.server_world().data().chunk_block(p.x, p.y, p.z)),
        Block::Water,
        "out-of-reach water must stay"
    );
    assert_eq!(
        game.server_player().inventory.selected().unwrap().item,
        ItemType::WoodenBucket
    );
}

#[test]
fn pouring_places_a_source_against_the_clicked_face_and_empties_the_bucket() {
    let mut game = game_on_empty_chunk();
    game.server_player_mut().inventory = holding(ItemType::WaterBucket);

    let floor = IVec3::new(3, 64, 3);
    game.server_world_mut()
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
        game.server_world().is_water_source_world(cell),
        "the clicked face's cell should hold a still source"
    );
    assert_eq!(
        game.server_player().inventory.selected().unwrap().item,
        ItemType::WoodenBucket,
        "the emptied bucket returns to the hand"
    );
}

#[test]
fn pouring_onto_flowing_water_firms_it_into_a_source() {
    let mut game = game_on_empty_chunk();
    game.server_player_mut().inventory = holding(ItemType::WaterBucket);

    stone_shelf(&mut game, 77);
    let src = SHELF_CENTER;
    game.server_world_mut()
        .set_block_world(src.x, src.y, src.z, Block::Water);
    run_water_ticks(&mut game, 30);
    let flow = src + IVec3::X;
    assert!(!game.server_world().is_water_source_world(flow));

    aim_down_at(&mut game, flow);
    let events = right_click(&mut game);

    assert!(
        events.player_at(0).used_item,
        "pouring into water must work"
    );
    assert!(
        game.server_world().is_water_source_world(flow),
        "the flowing cell firms into a still source"
    );
    assert_eq!(
        game.server_player().inventory.selected().unwrap().item,
        ItemType::WoodenBucket
    );
}

#[test]
fn pouring_onto_a_source_still_empties_the_bucket() {
    let mut game = game_on_empty_chunk();
    game.server_player_mut().inventory = holding(ItemType::WaterBucket);

    let p = IVec3::new(0, 78, 0);
    game.server_world_mut()
        .set_block_world(p.x, p.y, p.z, Block::Water);
    aim_down_at(&mut game, p);

    let events = right_click(&mut game);

    assert!(events.player_at(0).used_item);
    assert!(game.server_world().is_water_source_world(p));
    assert_eq!(
        game.server_player().inventory.selected().unwrap().item,
        ItemType::WoodenBucket
    );
}

#[test]
fn pouring_with_nothing_in_reach_keeps_the_water() {
    let mut game = game_on_empty_chunk();
    game.server_player_mut().inventory = holding(ItemType::WaterBucket);

    set_player_eye(&mut game, Vec3::new(0.5, 80.0, 0.5));
    game.server_player_mut().pitch = -std::f32::consts::FRAC_PI_2;

    let events = right_click(&mut game);

    assert!(!events.player_at(0).used_item);
    assert_eq!(
        game.server_player().inventory.selected().unwrap().item,
        ItemType::WaterBucket,
        "a refused pour must keep the water in the bucket"
    );
}

#[test]
fn shearing_the_targeted_sheep_drops_wool_and_strips_the_coat() {
    let mut game = game_on_empty_chunk();
    game.server_player_mut().inventory = holding(ItemType::Shears);
    assert!(game.server_world_mut().mobs_mut().spawn(
        petramond::mob::Mob::Sheep,
        WorldPos::new(8.0, 64.0, 8.0),
        0.0
    ));
    let events = right_click_at_mob(&mut game, 0);

    assert!(
        events.player_at(0).used_item,
        "shearing reports an item use"
    );
    assert!(
        game.server_world().mobs().instances()[0].is_shorn(),
        "the sheep is shorn"
    );
    let spec = petramond::mob::def(petramond::mob::Mob::Sheep)
        .shear
        .expect("sheep are shearable");
    let wool: Vec<_> = game
        .server_world()
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

    let events = right_click_at_mob(&mut game, 0);
    assert!(
        !events.player_at(0).used_item,
        "no double-shear while shorn"
    );
}

#[test]
fn shearing_needs_the_shears_in_hand() {
    let mut game = game_on_empty_chunk();
    game.server_player_mut().inventory = holding(ItemType::Dirt);
    assert!(game.server_world_mut().mobs_mut().spawn(
        petramond::mob::Mob::Sheep,
        WorldPos::new(8.0, 64.0, 8.0),
        0.0
    ));
    let events = right_click_at_mob(&mut game, 0);

    assert!(!events.player_at(0).used_item);
    assert!(
        !game.server_world().mobs().instances()[0].is_shorn(),
        "a bare right-click leaves the coat alone"
    );
}

fn walled_pool(game: &mut super::common::TestGame, fluid: Block) {
    for x in 4..=12 {
        for z in 4..=12 {
            game.server_world_mut()
                .set_block_world(x, 64, z, Block::Stone);
            let wall = x == 4 || x == 12 || z == 4 || z == 12;
            for y in 65..=66 {
                let b = if wall { Block::Stone } else { fluid };
                assert!(game.server_world_mut().set_block_world(x, y, z, b));
            }
        }
    }
}

const POOL_TOP: IVec3 = IVec3::new(8, 66, 8);

fn block_at(game: &super::common::TestGame, p: IVec3) -> Block {
    Block::from_id(game.server_world().data().chunk_block(p.x, p.y, p.z))
}

#[test]
fn filling_the_empty_bucket_from_a_lava_source_yields_the_lava_bucket() {
    let mut game = game_on_empty_chunk();
    game.server_player_mut().inventory = holding(ItemType::WoodenBucket);

    let p = IVec3::new(0, 78, 0);
    assert!(game
        .server_world_mut()
        .set_block_world(p.x, p.y, p.z, Block::Lava));
    aim_down_at(&mut game, p);

    let events = right_click(&mut game);

    assert!(
        events.player_at(0).used_item,
        "fill should report an item use"
    );
    assert_eq!(block_at(&game, p), Block::Air, "the lava source is scooped");
    assert_eq!(
        game.server_player().inventory.selected().unwrap().item,
        ItemType::by_name("petramond:lava_bucket").unwrap()
    );
}

#[test]
fn pouring_lava_at_a_pond_surface_acts_at_the_surface() {
    let mut game = game_on_empty_chunk();
    game.server_player_mut().inventory =
        holding(ItemType::by_name("petramond:lava_bucket").unwrap());
    walled_pool(&mut game, Block::Water);
    let below = POOL_TOP - IVec3::Y;
    aim_down_at(&mut game, POOL_TOP);

    let events = right_click(&mut game);

    assert!(
        events.player_at(0).used_item,
        "pouring onto water must work"
    );
    assert_eq!(
        block_at(&game, POOL_TOP),
        Block::Lava,
        "lava lands in the surface cell"
    );
    assert!(
        game.server_world().is_water_source_world(below),
        "the cell beneath stays water"
    );
    assert_eq!(
        game.server_player().inventory.selected().unwrap().item,
        ItemType::WoodenBucket
    );

    run_water_ticks(&mut game, 1);
    assert_eq!(
        block_at(&game, POOL_TOP),
        Block::Stone,
        "the poured lava cools at the surface"
    );
    assert!(
        game.server_world().is_water_source_world(below),
        "the pond floor is still water"
    );
}

#[test]
fn pouring_water_at_a_lava_sea_surface_cools_the_surface_not_the_floor() {
    let mut game = game_on_empty_chunk();
    game.server_player_mut().inventory = holding(ItemType::WaterBucket);
    walled_pool(&mut game, Block::Lava);
    let below = POOL_TOP - IVec3::Y;
    aim_down_at(&mut game, POOL_TOP);

    let events = right_click(&mut game);

    assert!(events.player_at(0).used_item, "pouring onto lava must work");
    assert!(
        game.server_world().is_water_source_world(POOL_TOP),
        "water lands in the surface cell"
    );
    assert_eq!(
        block_at(&game, below),
        Block::Lava,
        "the lava beneath is not the target"
    );

    run_water_ticks(&mut game, 1);
    assert!(
        game.server_world().is_water_source_world(POOL_TOP),
        "the water survives the contact"
    );
    assert_eq!(
        block_at(&game, below),
        Block::Stone,
        "the lava source under the water quenches to stone"
    );
    assert_eq!(
        block_at(&game, POOL_TOP + IVec3::X),
        Block::Stone,
        "so does the lava beside it"
    );
    let far = IVec3::new(10, 65, 10);
    assert_eq!(
        block_at(&game, far),
        Block::Lava,
        "lava out of contact stays lava"
    );
}

/// Server and client both get this bucket click, straight down onto `aim`. If the server uses
/// the item, the client should have predicted it, and if not, not.
fn bucket_click_agrees(item: ItemType, aim: IVec3, stage: &[(IVec3, Block)]) -> bool {
    let mut game = game_on_empty_chunk();
    game.game.replica.world.insert_chunk_for_test(
        petramond_world::chunk::ChunkPos::new(0, 0),
        petramond_world::chunk::Chunk::new(0, 0),
    );
    for &(p, b) in stage {
        assert!(game.server_world_mut().set_block_world(p.x, p.y, p.z, b));
        assert!(game.game.replica.world.set_block_world(p.x, p.y, p.z, b));
    }
    game.server_player_mut().inventory = holding(item);
    game.sync_self_view_for_test();
    aim_down_at(&mut game, aim);
    game.game.local.cam.pos = WorldPos::new(
        f64::from(aim.x) + 0.5,
        f64::from(aim.y) + 3.0,
        f64::from(aim.z) + 0.5,
    );
    game.game.local.cam.pitch = -std::f32::consts::FRAC_PI_2;

    let predicted = game
        .game
        .predict_click_verdict_at_for_test(aim, IVec3::Y, false);
    let events = right_click(&mut game);
    assert_eq!(
        predicted.consumed,
        events.player_at(0).used_item,
        "{item:?} at {aim:?}: the prediction and the authority disagree"
    );
    assert!(!predicted.places, "a bucket click is never a place ghost");
    predicted.consumed
}

#[test]
fn bucket_prediction_matches_the_authoritative_fill() {
    let src = IVec3::new(4, 70, 4);
    assert!(
        bucket_click_agrees(ItemType::WoodenBucket, src, &[(src, Block::Water)]),
        "a source under the eye fills on both mirrors"
    );
    assert!(
        !bucket_click_agrees(ItemType::WoodenBucket, src, &[(src, Block::Stone)]),
        "stone is nothing to scoop on either mirror"
    );
}

#[test]
fn bucket_prediction_matches_the_authoritative_pour() {
    let floor = IVec3::new(4, 70, 4);
    assert!(
        bucket_click_agrees(ItemType::WaterBucket, floor, &[(floor, Block::Stone)]),
        "a pour against the floor lands on both mirrors"
    );
    assert!(
        !bucket_click_agrees(ItemType::WaterBucket, IVec3::new(4, 20, 4), &[]),
        "a pour with nothing in reach lands on neither"
    );
}
