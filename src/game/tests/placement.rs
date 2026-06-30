use super::super::placement::facing_from_forward;
use super::super::tick::TickEvents;
use super::super::Game;
use super::common::{filled_inventory, game, hit, install_empty_chunk};
use crate::block::Block;
use crate::furnace::Facing;
use crate::inventory::Inventory;
use crate::item::{ItemStack, ItemType};
use crate::mathh::{IVec3, Vec3};

#[test]
fn place_with_empty_hand_does_nothing() {
    let mut game = game();
    // The starting inventory is already empty.
    assert!(game.player.inventory.selected().is_none());
    game.look = Some(hit(IVec3::new(0, 40, 0), IVec3::Y));
    assert!(!game.try_place());
}

#[test]
fn right_clicking_interactable_blocks_requests_their_screen() {
    enum ExpectedOpen {
        CraftingTable,
        Furnace,
        Chest,
        FurnitureWorkbench,
    }

    for (block, expected) in [
        (Block::CraftingTable, ExpectedOpen::CraftingTable),
        (Block::Furnace, ExpectedOpen::Furnace),
        (Block::Chest, ExpectedOpen::Chest),
        (Block::FurnitureWorkbench, ExpectedOpen::FurnitureWorkbench),
    ] {
        let mut game = game();
        install_empty_chunk(&mut game);
        let pos = IVec3::new(4, 64, 4);
        game.world.set_block_world(pos.x, pos.y, pos.z, block);
        game.look = Some(hit(pos, IVec3::Y));
        game.pending_place = true;

        let mut events = TickEvents::default();
        game.tick_place(&mut events);

        assert!(
            events.placed_block.is_none(),
            "{block:?} should interact, not place"
        );
        match expected {
            ExpectedOpen::CraftingTable => {
                assert!(game.request_open_table, "{block:?} should open crafting");
            }
            ExpectedOpen::Furnace => {
                assert_eq!(game.request_open_furnace, Some(pos), "{block:?}");
            }
            ExpectedOpen::Chest => {
                assert_eq!(game.request_open_chest, Some(pos), "{block:?}");
            }
            ExpectedOpen::FurnitureWorkbench => {
                assert_eq!(game.request_open_workbench, Some(pos), "{block:?}");
            }
        }
    }
}

#[test]
fn right_clicking_a_door_toggles_it_through_block_interaction() {
    let mut game = game();
    install_empty_chunk(&mut game);
    let floor = IVec3::new(5, 63, 5);
    let lower = floor + IVec3::Y;
    game.world
        .set_block_world(floor.x, floor.y, floor.z, Block::Stone);
    assert!(game.world.place_door(lower, Block::OakDoor, Facing::South));
    assert!(
        !game
            .world
            .door_state_at(lower.x, lower.y, lower.z)
            .unwrap()
            .open
    );

    game.look = Some(hit(lower, IVec3::Y));
    game.pending_place = true;
    let mut events = TickEvents::default();
    game.tick_place(&mut events);

    assert!(events.placed_block.is_none(), "door click should not place");
    assert!(
        game.toggled_door.is_some(),
        "door click should report a toggle event"
    );
    assert!(
        game.world
            .door_state_at(lower.x, lower.y, lower.z)
            .unwrap()
            .open
    );
    let upper = lower + IVec3::Y;
    assert!(
        game.world
            .door_state_at(upper.x, upper.y, upper.z)
            .unwrap()
            .open
    );
}

#[test]
fn place_into_loaded_air_decrements_selected() {
    let mut game = game();
    game.player.inventory = filled_inventory();
    // Player at the surface (section cy 4 ≈ y64): the vertical window streams the surface
    // band, and the y=200 placement below is into open air via materialize-on-write.
    game.world.update_load(0, 4, 0);
    // Real async generation runs on the shared worldgen pool; under a saturated pool
    // (the full `worldgen-tests` suite on a many-core box) this chunk's job can queue
    // for a while, so wait on a generous wall-clock deadline rather than a fixed poll
    // count to stay robust under load. The common case still returns in well under 1s.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let mut loaded = false;
    while std::time::Instant::now() < deadline {
        game.world.poll();
        if game.world.chunk_loaded(0, 0) {
            loaded = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert!(loaded, "chunk (0,0) failed to load within 30s");

    let p = IVec3::new(0, 200, 0);
    assert!(Block::from_id(game.world.chunk_block(p.x, p.y, p.z)).is_replaceable());
    game.player.inventory.set_active(0);
    let item = game.player.inventory.selected().unwrap().item;
    let block = item.as_block().unwrap();
    let before = game.player.inventory.selected().unwrap().count;

    game.look = Some(hit(IVec3::new(0, 199, 0), IVec3::Y));
    assert!(game.try_place());

    assert_eq!(Block::from_id(game.world.chunk_block(p.x, p.y, p.z)), block);
    assert_eq!(game.player.inventory.selected().unwrap().count, before - 1);
}

#[test]
fn placing_into_replaceable_grass_overwrites_it_with_no_drop() {
    // Right-clicking short grass (a replaceable plant) while holding a block places
    // the block straight INTO the grass cell, overwriting it with no drop — not on
    // top of it.
    let mut game = game();
    install_empty_chunk(&mut game);
    game.player.inventory = filled_inventory(); // a stack of Dirt
    game.player.inventory.set_active(0);
    game.player.pos = Vec3::new(100.0, 64.0, 100.0); // park clear of the cell

    let g = IVec3::new(8, 100, 8);
    game.world.set_block_world(g.x, g.y, g.z, Block::ShortGrass);
    let before = game.player.inventory.selected().unwrap().count;

    // Look straight at the grass and place into it.
    game.look = Some(hit(g, IVec3::Y));
    assert!(game.try_place(), "placing into replaceable grass succeeds");

    assert_eq!(
        Block::from_id(game.world.chunk_block(g.x, g.y, g.z)),
        Block::Dirt,
        "the block replaced the grass in its own cell, not the cell above"
    );
    assert_eq!(
        game.player.inventory.selected().unwrap().count,
        before - 1,
        "one block was consumed"
    );
    assert!(
        game.world.item_entities().is_empty(),
        "the overwritten grass dropped nothing"
    );
}

#[test]
fn rooted_plants_place_only_on_their_required_ground() {
    // The data-driven substrate gate: a flower roots in soil (grass/dirt), a cactus
    // in sand (sand/red sand). Building onto the wrong ground is a no-op; the right
    // ground accepts it. Each case uses its own column so they don't interfere.
    fn place_on(game: &mut Game, ground: Block, item: ItemType, col: i32) -> bool {
        let g = IVec3::new(col, 100, col);
        game.world.set_block_world(g.x, g.y, g.z, ground);
        let mut inv = Inventory::new();
        inv.add(ItemStack::new(item, 1));
        game.player.inventory = inv;
        game.player.inventory.set_active(0);
        game.look = Some(hit(g, IVec3::Y)); // build on TOP of the ground block
        let placed = game.try_place();
        // The return must agree with whether the block actually landed above.
        let above = Block::from_id(game.world.chunk_block(g.x, g.y + 1, g.z));
        assert_eq!(
            placed,
            above == item.as_block().unwrap(),
            "try_place() return must match whether the block landed"
        );
        placed
    }

    let mut game = game();
    install_empty_chunk(&mut game);
    game.player.pos = Vec3::new(100.0, 64.0, 100.0); // park clear of every cell

    // A flower (Dandelion) roots in soil only.
    assert!(
        !place_on(&mut game, Block::Stone, ItemType::Dandelion, 2),
        "no flower on stone"
    );
    assert!(
        place_on(&mut game, Block::Grass, ItemType::Dandelion, 4),
        "flower on grass"
    );
    assert!(
        place_on(&mut game, Block::Dirt, ItemType::Dandelion, 6),
        "flower on dirt"
    );
    assert!(
        !place_on(&mut game, Block::Sand, ItemType::Dandelion, 8),
        "no flower on sand"
    );

    // A cactus roots in sand only.
    assert!(
        !place_on(&mut game, Block::Grass, ItemType::Cactus, 10),
        "no cactus on grass"
    );
    assert!(
        place_on(&mut game, Block::Sand, ItemType::Cactus, 12),
        "cactus on sand"
    );
    assert!(
        place_on(&mut game, Block::RedSand, ItemType::Cactus, 14),
        "cactus on red sand"
    );

    // A mushroom roots in soil OR any stone (its two RootsIn* tags combine).
    assert!(
        place_on(&mut game, Block::Grass, ItemType::BrownMushroom, 1),
        "mushroom on grass"
    );
    assert!(
        place_on(&mut game, Block::Stone, ItemType::BrownMushroom, 3),
        "mushroom on stone"
    );
    assert!(
        place_on(&mut game, Block::Cobblestone, ItemType::BrownMushroom, 5),
        "mushroom on cobblestone"
    );
    assert!(
        !place_on(&mut game, Block::Sand, ItemType::BrownMushroom, 7),
        "no mushroom on sand"
    );
    assert!(
        !place_on(&mut game, Block::OakPlanks, ItemType::BrownMushroom, 9),
        "no mushroom on wood"
    );
}

#[test]
fn furnace_front_faces_the_player_on_placement() {
    // The front points opposite the look direction (back toward the player).
    assert_eq!(facing_from_forward(Vec3::new(0.0, 0.0, 1.0)), Facing::North);
    assert_eq!(
        facing_from_forward(Vec3::new(0.0, 0.0, -1.0)),
        Facing::South
    );
    assert_eq!(facing_from_forward(Vec3::new(1.0, 0.0, 0.0)), Facing::West);
    assert_eq!(facing_from_forward(Vec3::new(-1.0, 0.0, 0.0)), Facing::East);
    // A pitched, mostly-horizontal look snaps to the dominant horizontal axis.
    assert_eq!(
        facing_from_forward(Vec3::new(0.2, -0.9, 0.95)),
        Facing::North
    );
}
