use super::super::tick::TickEvents;
use super::common::{filled_inventory, game, game_on_empty_chunk, give, hit};
use crate::block::Block;
use crate::block_state::{HeldBlockState, LogAxis, SlabSplit, SlabState, StairHalf, StairState};
use crate::facing::Facing;
use crate::item::{ItemStack, ItemType};
use crate::mathh::{IVec3, Vec3};
use crate::server::placement::facing_from_forward;

#[test]
fn place_with_empty_hand_does_nothing() {
    let mut game = game();
    // The starting inventory is already empty.
    assert!(game.server.sessions[0]
        .player
        .inventory
        .selected()
        .is_none());
    game.server.sessions[0].look = Some(hit(IVec3::new(0, 40, 0), IVec3::Y));
    assert!(!game.server.try_place_for_test());
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
        let mut game = game_on_empty_chunk();
        let pos = IVec3::new(4, 64, 4);
        game.server
            .world
            .set_block_world(pos.x, pos.y, pos.z, block);
        game.server.sessions[0].look = Some(hit(pos, IVec3::Y));
        game.server.queue_place_click_for_test(0);

        let mut events = TickEvents::default();
        game.server.tick_place(0, &mut events);
        game.server.tick_menu(0, &mut events);

        assert!(
            events.player_at(0).placed_block.is_none(),
            "{block:?} should interact, not place"
        );
        // Every consumed interaction reports through the generic flag, so the
        // interact hand jab is the default for ALL interactables — a new
        // interaction kind must not need remembering in the presentation.
        assert!(
            events.player_at(0).interacted,
            "{block:?} should report interacted"
        );
        match expected {
            ExpectedOpen::CraftingTable => {
                assert!(
                    game.server.sessions[0].request_open_table,
                    "{block:?} should open crafting"
                );
            }
            ExpectedOpen::Furnace => {
                assert_eq!(
                    game.server.sessions[0].request_open_furnace,
                    Some(pos),
                    "{block:?}"
                );
            }
            ExpectedOpen::Chest => {
                assert_eq!(
                    game.server.sessions[0].request_open_chest,
                    Some(pos),
                    "{block:?}"
                );
            }
            ExpectedOpen::FurnitureWorkbench => {
                assert!(game.server.sessions[0].request_open_workbench, "{block:?}");
            }
        }
    }
}

#[test]
fn right_clicking_a_door_toggles_it_through_block_interaction() {
    let mut game = game_on_empty_chunk();
    let floor = IVec3::new(5, 63, 5);
    let lower = floor + IVec3::Y;
    game.server
        .world
        .set_block_world(floor.x, floor.y, floor.z, Block::Stone);
    assert!(game
        .server
        .world
        .place_door(lower, Block::OakDoor, Facing::South));
    assert!(
        !game
            .server
            .world
            .door_state_at(lower.x, lower.y, lower.z)
            .unwrap()
            .open
    );

    game.server.sessions[0].look = Some(hit(lower, IVec3::Y));
    game.server.queue_place_click_for_test(0);
    let mut events = TickEvents::default();
    game.server.tick_place(0, &mut events);

    assert!(
        events.player_at(0).placed_block.is_none(),
        "door click should not place"
    );
    assert!(
        events.player_at(0).toggled_door.is_some(),
        "door click should report a toggle event to the toggler"
    );
    assert!(
        game.server
            .world
            .door_state_at(lower.x, lower.y, lower.z)
            .unwrap()
            .open
    );
    let upper = lower + IVec3::Y;
    assert!(
        game.server
            .world
            .door_state_at(upper.x, upper.y, upper.z)
            .unwrap()
            .open
    );
}

/// Hold-to-interact: a HELD use button re-runs the WHOLE use-click ladder on
/// a fixed cadence — a held click on a door keeps toggling it (interact
/// rung), a held block keeps placing (placement rung; the item-use rung is
/// pinned end-to-end by the farming hold-to-till assertions) — never faster
/// than the repeat interval, and stopping on release. A held button over
/// nothing actionable attempts silently and does nothing, exactly like
/// today's single click. Consumed repeats echo the initiator's hand jab
/// (`used_unpredicted`): there was no client click to animate it.
#[test]
fn holding_use_repeats_the_use_ladder_but_stays_a_noop_on_plain_blocks() {
    use crate::server::placement::USE_REPEAT_TICKS;

    let mut game = game_on_empty_chunk();
    let floor = IVec3::new(5, 63, 5);
    let lower = floor + IVec3::Y;
    game.server
        .world
        .set_block_world(floor.x, floor.y, floor.z, Block::Stone);
    assert!(game
        .server
        .world
        .place_door(lower, Block::OakDoor, Facing::South));
    let door_open = |game: &super::common::TestGame| {
        game.server
            .world
            .door_state_at(lower.x, lower.y, lower.z)
            .unwrap()
            .open
    };

    game.server.sessions[0].look = Some(hit(lower, IVec3::Y));
    game.server.sessions[0].intent_gameplay = true;
    game.server.sessions[0].intent_use_held = true;
    game.server.queue_place_click_for_test(0);
    game.server.tick_place(0, &mut TickEvents::default());
    assert!(door_open(&game), "the click itself toggles once");

    // Held: paced — nothing re-fires inside the repeat interval...
    for _ in 0..USE_REPEAT_TICKS - 1 {
        game.server.tick_place(0, &mut TickEvents::default());
        assert!(door_open(&game), "held repeats never fire per-tick");
    }
    // ...then the interval elapses and the repeat toggles again, echoing
    // the jab the client never played.
    let mut repeat_ev = TickEvents::default();
    game.server.tick_place(0, &mut repeat_ev);
    assert!(!door_open(&game), "the held repeat toggled the door again");
    assert!(
        repeat_ev.player_at(0).used_unpredicted && repeat_ev.player_at(0).interacted,
        "a consumed repeat animates: observer row + initiator jab echo"
    );

    // Release: no further repeats, however long the look lingers.
    game.server.sessions[0].intent_use_held = false;
    for _ in 0..3 * USE_REPEAT_TICKS {
        game.server.tick_place(0, &mut TickEvents::default());
    }
    assert!(!door_open(&game), "release stops the repeats");

    // A held button over a NON-interactable block: attempts keep pacing but
    // consume nothing — no toggle, no jab echo, no observer animation.
    game.server.sessions[0].look = Some(hit(floor, IVec3::Y));
    game.server.sessions[0].intent_use_held = true;
    let mut ev = TickEvents::default();
    for _ in 0..3 * USE_REPEAT_TICKS {
        game.server.tick_place(0, &mut ev);
    }
    assert!(
        !ev.player_at(0).used_unpredicted && !ev.player_at(0).interacted,
        "a non-interactable repeat stays silent, like today's single click"
    );
    assert!(
        !door_open(&game),
        "and the door two cells away is untouched"
    );

    // The FULL ladder repeats — a held block keeps placing: give the hand
    // dirt and hold over a fresh stone shelf; the paced repeat places with
    // no click at all, echoing the jab the client never latched.
    let shelf = IVec3::new(8, 63, 8);
    game.server
        .world
        .set_block_world(shelf.x, shelf.y, shelf.z, Block::Stone);
    game.server.sessions[0].player.inventory = filled_inventory(); // Dirt in slot 0
    game.server.sessions[0].look = Some(hit(shelf, IVec3::Y));
    let mut place_ev = TickEvents::default();
    for _ in 0..USE_REPEAT_TICKS {
        game.server.tick_place(0, &mut place_ev);
    }
    assert_eq!(
        Block::from_id(game.server.world.chunk_block(shelf.x, shelf.y + 1, shelf.z)),
        Block::Dirt,
        "a held repeat runs the placement rung too"
    );
    assert!(
        place_ev.player_at(0).used_unpredicted,
        "the un-clicked placement echoes the jab"
    );
}

#[test]
fn place_into_loaded_air_decrements_selected() {
    let mut game = game();
    game.server.sessions[0].player.inventory = filled_inventory();
    // Player at the surface (section cy 4 ≈ y64): the vertical window streams the surface
    // band, and the y=200 placement below is into open air via materialize-on-write.
    game.server.world.update_load(0, 4, 0);
    // Real async generation runs on the shared worldgen pool; under a saturated pool
    // (the full `worldgen-tests` suite on a many-core box) this chunk's job can queue
    // for a while, so wait on a generous wall-clock deadline rather than a fixed poll
    // count to stay robust under load. The common case still returns in well under 1s.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let mut loaded = false;
    while std::time::Instant::now() < deadline {
        game.server.world.poll();
        if game.server.world.chunk_loaded(0, 0) {
            loaded = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert!(loaded, "chunk (0,0) failed to load within 30s");

    let p = IVec3::new(0, 200, 0);
    assert!(Block::from_id(game.server.world.chunk_block(p.x, p.y, p.z)).is_replaceable());
    game.server.sessions[0].player.inventory.set_active(0);
    let item = game.server.sessions[0]
        .player
        .inventory
        .selected()
        .unwrap()
        .item;
    let block = item.as_block().unwrap();
    let before = game.server.sessions[0]
        .player
        .inventory
        .selected()
        .unwrap()
        .count;

    game.server.sessions[0].look = Some(hit(IVec3::new(0, 199, 0), IVec3::Y));
    assert!(game.server.try_place_for_test());

    assert_eq!(
        Block::from_id(game.server.world.chunk_block(p.x, p.y, p.z)),
        block
    );
    assert_eq!(
        game.server.sessions[0]
            .player
            .inventory
            .selected()
            .unwrap()
            .count,
        before - 1
    );
}

#[test]
fn placing_into_replaceable_grass_overwrites_it_with_no_drop() {
    // Right-clicking short grass (a replaceable plant) while holding a block places
    // the block straight INTO the grass cell, overwriting it with no drop — not on
    // top of it.
    let mut game = game_on_empty_chunk();
    game.server.sessions[0].player.inventory = filled_inventory(); // a stack of Dirt
    game.server.sessions[0].player.inventory.set_active(0);
    game.server.sessions[0].player.pos = Vec3::new(100.0, 64.0, 100.0); // park clear of the cell

    let g = IVec3::new(8, 100, 8);
    game.server
        .world
        .set_block_world(g.x, g.y, g.z, Block::ShortGrass);
    let before = game.server.sessions[0]
        .player
        .inventory
        .selected()
        .unwrap()
        .count;

    // Look straight at the grass and place into it.
    game.server.sessions[0].look = Some(hit(g, IVec3::Y));
    assert!(
        game.server.try_place_for_test(),
        "placing into replaceable grass succeeds"
    );

    assert_eq!(
        Block::from_id(game.server.world.chunk_block(g.x, g.y, g.z)),
        Block::Dirt,
        "the block replaced the grass in its own cell, not the cell above"
    );
    assert_eq!(
        game.server.sessions[0]
            .player
            .inventory
            .selected()
            .unwrap()
            .count,
        before - 1,
        "one block was consumed"
    );
    assert!(
        game.server.world.item_entities().is_empty(),
        "the overwritten grass dropped nothing"
    );
}

#[test]
fn rooted_plants_place_only_on_their_required_ground() {
    // The data-driven substrate gate: a flower roots in soil (grass/dirt), a cactus
    // in sand (sand/red sand). Building onto the wrong ground is a no-op; the right
    // ground accepts it. Each case uses its own column so they don't interfere.
    fn place_on(
        game: &mut super::common::TestGame,
        ground: Block,
        item: ItemType,
        col: i32,
    ) -> bool {
        let g = IVec3::new(col, 100, col);
        game.server.world.set_block_world(g.x, g.y, g.z, ground);
        give(game, item, 1);
        game.server.sessions[0].look = Some(hit(g, IVec3::Y)); // build on TOP of the ground block
        let placed = game.server.try_place_for_test();
        // The return must agree with whether the block actually landed above.
        let above = Block::from_id(game.server.world.chunk_block(g.x, g.y + 1, g.z));
        assert_eq!(
            placed,
            above == item.as_block().unwrap(),
            "try_place() return must match whether the block landed"
        );
        placed
    }

    let mut game = game_on_empty_chunk();
    game.server.sessions[0].player.pos = Vec3::new(100.0, 64.0, 100.0); // park clear of every cell

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
fn rotating_held_stair_places_top_half() {
    let mut game = game_on_empty_chunk();
    game.server.sessions[0].player.pos = Vec3::new(100.0, 64.0, 100.0);
    give(&mut game, ItemType::OakStairs, 1);
    game.toggle_held_block_rotation();

    let p = IVec3::new(4, 64, 4);
    game.server.sessions[0].look = Some(hit(p - IVec3::Y, IVec3::Y));
    assert!(game.server.try_place_for_test());

    assert_eq!(
        game.server.world.stair_state_at(p.x, p.y, p.z),
        StairState::new(Facing::North, StairHalf::Top)
    );
}

#[test]
fn slabs_stack_horizontally_with_mixed_materials() {
    let mut game = game_on_empty_chunk();
    game.server.sessions[0].player.pos = Vec3::new(100.0, 64.0, 100.0);
    let p = IVec3::new(4, 64, 4);

    give(&mut game, ItemType::DirtSlab, 1);
    game.server.sessions[0].look = Some(hit(p - IVec3::Y, IVec3::Y));
    assert!(game.server.try_place_for_test(), "first slab places");
    assert_eq!(
        game.server.world.slab_state_at(p.x, p.y, p.z),
        SlabState::single(SlabSplit::Y, 0, Block::DirtSlab)
    );

    give(&mut game, ItemType::CobblestoneSlab, 1);
    game.server.sessions[0].look = Some(hit(p, IVec3::Y));
    assert!(
        game.server.try_place_for_test(),
        "second slab stacks in the hit cell"
    );

    let state = game.server.world.slab_state_at(p.x, p.y, p.z);
    assert_eq!(state.split, SlabSplit::Y);
    assert_eq!(state.layers, [Block::DirtSlab, Block::CobblestoneSlab]);
    assert_eq!(
        game.server.world.slab_drop_stacks_at(p),
        vec![
            ItemStack::new(ItemType::DirtSlab, 1),
            ItemStack::new(ItemType::CobblestoneSlab, 1),
        ],
        "breaking a mixed stack must recover every slab layer"
    );
}

#[test]
fn slabs_stack_vertically_with_mixed_materials() {
    let mut game = game_on_empty_chunk();
    game.server.sessions[0].player.pos = Vec3::new(100.0, 64.0, 100.0);
    let support = IVec3::new(3, 64, 4);
    let p = support + IVec3::X;
    game.server
        .world
        .set_block_world(support.x, support.y, support.z, Block::Stone);

    give(&mut game, ItemType::StoneSlab, 1);
    game.toggle_held_block_rotation();
    game.toggle_held_block_rotation();
    game.server.sessions[0].look = Some(hit(support, IVec3::X));
    assert!(
        game.server.try_place_for_test(),
        "vertical slab places against support"
    );

    give(&mut game, ItemType::DirtSlab, 1);
    game.toggle_held_block_rotation();
    game.toggle_held_block_rotation();
    game.server.sessions[0].look = Some(hit(p, IVec3::X));
    assert!(
        game.server.try_place_for_test(),
        "second vertical slab stacks in the open half"
    );

    let state = game.server.world.slab_state_at(p.x, p.y, p.z);
    assert_eq!(state.split, SlabSplit::X);
    assert_eq!(state.layers, [Block::StoneSlab, Block::DirtSlab]);
}

#[test]
fn torch_places_on_the_flat_side_of_a_stair() {
    let mut game = game_on_empty_chunk();
    game.server.sessions[0].player.pos = Vec3::new(100.0, 64.0, 100.0);
    let stair = IVec3::new(4, 64, 4);
    assert!(game.server.world.place_stair(
        stair,
        Block::OakStairs,
        StairState::new(Facing::East, StairHalf::Bottom)
    ));

    give(&mut game, ItemType::Torch, 1);
    game.server.sessions[0].look = Some(hit(stair, -IVec3::X));
    assert!(
        game.server.try_place_for_test(),
        "torch places on stair back"
    );

    let torch = stair - IVec3::X;
    assert_eq!(
        Block::from_id(game.server.world.chunk_block(torch.x, torch.y, torch.z)),
        Block::Torch
    );
    assert_eq!(
        game.server.world.torch_placement(torch),
        crate::torch::TorchPlacement::West
    );
}

#[test]
fn torch_does_not_place_on_the_open_side_of_a_stair() {
    let mut game = game_on_empty_chunk();
    game.server.sessions[0].player.pos = Vec3::new(100.0, 64.0, 100.0);
    let stair = IVec3::new(4, 64, 4);
    assert!(game.server.world.place_stair(
        stair,
        Block::OakStairs,
        StairState::new(Facing::East, StairHalf::Bottom)
    ));

    give(&mut game, ItemType::Torch, 1);
    game.server.sessions[0].look = Some(hit(stair, IVec3::X));
    assert!(
        !game.server.try_place_for_test(),
        "stair open side is not a full torch support face"
    );

    let torch = stair + IVec3::X;
    assert_eq!(
        Block::from_id(game.server.world.chunk_block(torch.x, torch.y, torch.z)),
        Block::Air
    );
}

#[test]
fn torch_does_not_place_on_the_side_of_a_single_slab() {
    let mut game = game_on_empty_chunk();
    game.server.sessions[0].player.pos = Vec3::new(100.0, 64.0, 100.0);
    let slab = IVec3::new(4, 64, 4);
    assert!(game.server.world.place_slab_layer(
        slab,
        Block::DirtSlab,
        crate::slab::SlabSlot {
            split: SlabSplit::Y,
            index: 0,
        }
    ));

    give(&mut game, ItemType::Torch, 1);
    game.server.sessions[0].look = Some(hit(slab, IVec3::X));
    assert!(
        !game.server.try_place_for_test(),
        "single slab side is not a full torch support face"
    );

    let torch = slab + IVec3::X;
    assert_eq!(
        Block::from_id(game.server.world.chunk_block(torch.x, torch.y, torch.z)),
        Block::Air
    );
}

#[test]
fn torch_places_on_the_side_of_a_full_slab_stack() {
    let mut game = game_on_empty_chunk();
    game.server.sessions[0].player.pos = Vec3::new(100.0, 64.0, 100.0);
    let slab = IVec3::new(4, 64, 4);
    for (block, index) in [(Block::DirtSlab, 0), (Block::CobblestoneSlab, 1)] {
        assert!(game.server.world.place_slab_layer(
            slab,
            block,
            crate::slab::SlabSlot {
                split: SlabSplit::Y,
                index,
            }
        ));
    }

    give(&mut game, ItemType::Torch, 1);
    game.server.sessions[0].look = Some(hit(slab, IVec3::X));
    assert!(
        game.server.try_place_for_test(),
        "full slab stack supports wall torch"
    );

    let torch = slab + IVec3::X;
    assert_eq!(
        Block::from_id(game.server.world.chunk_block(torch.x, torch.y, torch.z)),
        Block::Torch
    );
}

#[test]
fn slab_side_clicks_build_into_the_adjacent_cell_not_the_hit_cell() {
    let mut game = game_on_empty_chunk();
    game.server.sessions[0].player.pos = Vec3::new(100.0, 64.0, 100.0);
    let p = IVec3::new(4, 64, 4);

    give(&mut game, ItemType::DirtSlab, 2);
    game.server.sessions[0].look = Some(hit(p - IVec3::Y, IVec3::Y));
    assert!(game.server.try_place_for_test(), "bottom slab places");

    // Hold TOP rotation and click the bottom slab's SIDE face: the hit cell's
    // empty top half must not swallow the click — only a face looking along
    // the split axis stacks. The top slab builds in the adjacent cell.
    game.toggle_held_block_rotation();
    game.server.sessions[0].look = Some(hit(p, IVec3::X));
    assert!(game.server.try_place_for_test(), "side click places");
    assert_eq!(
        game.server.world.slab_state_at(p.x, p.y, p.z),
        SlabState::single(SlabSplit::Y, 0, Block::DirtSlab),
        "the hit cell keeps its lone bottom layer"
    );
    assert_eq!(
        game.server.world.slab_state_at(p.x + 1, p.y, p.z),
        SlabState::single(SlabSplit::Y, 1, Block::DirtSlab),
        "the top slab lands in the adjacent cell"
    );
}

#[test]
fn held_rotation_does_not_leak_across_item_swaps() {
    let mut game = game_on_empty_chunk();
    game.server.sessions[0].player.pos = Vec3::new(100.0, 64.0, 100.0);
    let p = IVec3::new(4, 64, 4);

    // Rotate a held stair, then swap the ACTIVE SLOT's content to a slab (an
    // inventory-GUI style swap — no hotbar switch, so nothing clears the
    // latched rotation). The stale stair rotation must not orient the slab.
    give(&mut game, ItemType::DirtStairs, 1);
    game.toggle_held_block_rotation();

    give(&mut game, ItemType::DirtSlab, 1);
    game.server.sessions[0].look = Some(hit(p - IVec3::Y, IVec3::Y));
    assert!(game.server.try_place_for_test(), "slab places");
    assert_eq!(
        game.server.world.slab_state_at(p.x, p.y, p.z),
        SlabState::single(SlabSplit::Y, 0, Block::DirtSlab),
        "an un-rotated slab places as a bottom slab"
    );
}

#[test]
fn rotating_held_log_places_horizontal_axis() {
    let mut game = game_on_empty_chunk();
    game.server.sessions[0].player.pos = Vec3::new(100.0, 64.0, 100.0);
    give(&mut game, ItemType::OakLog, 1);

    let vertical = IVec3::new(4, 64, 4);
    game.server.sessions[0].look = Some(hit(vertical - IVec3::Y, IVec3::Y));
    assert!(game.server.try_place_for_test());
    assert_eq!(
        game.server
            .world
            .log_axis_at(vertical.x, vertical.y, vertical.z),
        LogAxis::Y
    );

    give(&mut game, ItemType::OakLog, 1);
    game.toggle_held_block_rotation();

    let horizontal = IVec3::new(6, 64, 4);
    game.server.sessions[0].look = Some(hit(horizontal - IVec3::Y, IVec3::Y));
    assert!(game.server.try_place_for_test());
    assert_eq!(
        game.server
            .world
            .log_axis_at(horizontal.x, horizontal.y, horizontal.z),
        LogAxis::Z
    );
}

#[test]
fn held_rotation_state_toggles_only_for_rotatable_blocks() {
    let mut game = game();
    give(&mut game, ItemType::OakLog, 1);
    game.sync_self_view_for_test(); // held_block_state reads the replicated view

    assert_eq!(game.held_block_state(), HeldBlockState::Log(LogAxis::Y));
    game.toggle_held_block_rotation();
    assert_eq!(game.held_block_state(), HeldBlockState::Log(LogAxis::X));
    game.toggle_held_block_rotation();
    assert_eq!(game.held_block_state(), HeldBlockState::Log(LogAxis::Y));

    give(&mut game, ItemType::StonePickaxe, 1);
    game.sync_self_view_for_test();
    game.toggle_held_block_rotation();
    assert_eq!(game.held_block_state(), HeldBlockState::None);
}

/// A model block's data row picks how it turns to meet the player: LeftToRight spans
/// the authored X axis across the view (workbench), FrontToBack runs it away from the
/// player with the clicked cell at the near end (bed: foot first, headboard far).
#[test]
fn model_placement_orientation_spans_across_or_away() {
    // The default camera (yaw 0) looks south (+Z).
    let place = |item: ItemType, target: IVec3| -> super::common::TestGame {
        let mut game = game_on_empty_chunk();
        game.server.sessions[0].player.pos = Vec3::new(100.0, 64.0, 100.0); // park clear of every cell
        give(&mut game, item, 1);
        game.server.sessions[0].look = Some(hit(target - IVec3::new(0, 1, 0), IVec3::Y));
        assert!(game.server.try_place_for_test(), "{item:?} should place");
        game
    };
    let at = |game: &super::common::TestGame, p: IVec3| {
        Block::from_id(game.server.world.chunk_block(p.x, p.y, p.z))
    };

    // FrontToBack: the bed occupies the clicked cell and the cell BEYOND it (south,
    // away from the player) — never the cells beside it.
    let p = IVec3::new(4, 64, 4);
    let bed = place(ItemType::Bed, p);
    assert_eq!(
        at(&bed, p),
        Block::Bed,
        "near (foot) end at the clicked cell"
    );
    assert_eq!(
        at(&bed, p + IVec3::new(0, 0, 1)),
        Block::Bed,
        "far (head) end grows away from the player"
    );
    assert_eq!(at(&bed, p + IVec3::new(1, 0, 0)), Block::Air);
    assert_eq!(at(&bed, p - IVec3::new(1, 0, 0)), Block::Air);

    // LeftToRight: the workbench spans sideways (east-west) across the same view.
    let wb = place(ItemType::FurnitureWorkbench, p);
    assert_eq!(at(&wb, p), Block::FurnitureWorkbench);
    assert_eq!(
        at(&wb, p - IVec3::new(1, 0, 0)),
        Block::FurnitureWorkbench,
        "second column beside the clicked cell"
    );
    assert_eq!(at(&wb, p + IVec3::new(0, 0, 1)), Block::Air);
    assert_eq!(at(&wb, p - IVec3::new(0, 0, 1)), Block::Air);
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
