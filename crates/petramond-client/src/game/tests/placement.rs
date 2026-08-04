use petramond::events::tick::TickEvents;
use super::common::{filled_inventory, game, game_on_empty_chunk, give, hit};
use petramond::block::Block;
use petramond::block_state::{HeldBlockState, LogAxis, SlabSplit, SlabState, StairHalf, StairState};
use petramond::facing::Facing;
use petramond::item::{ItemStack, ItemType};
use petramond::mathh::{IVec3, Vec3};
use petramond::server::placement::facing_from_forward;

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
    use petramond::gui_state::GuiKind;

    for (block, expected_kind) in [
        (Block::CraftingTable, GuiKind::CraftingTable),
        (Block::Furnace, GuiKind::Furnace),
        (Block::Chest, GuiKind::Chest),
        (Block::FurnitureWorkbench, GuiKind::FurnitureWorkbench),
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
        // Every engine container opens through the SAME unified request lane
        // a mod GUI uses: one (kind, pos) shape, no per-kind fields.
        assert_eq!(
            game.server.sessions[0].request_open_gui,
            Some((expected_kind, Some(pos))),
            "{block:?}"
        );
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

#[test]
fn place_into_loaded_air_decrements_selected() {
    let mut game = game();
    game.server.sessions[0].player.inventory = filled_inventory();
    // Player at the surface (section cy 4 ≈ y64): the vertical window streams the surface
    // band, and the y=200 placement below is into open air via materialize-on-write.
    game.server.world.update_load(0, 4, 0);
    // `TestGame` uses an inline job pool — gen finishes inside `poll`.
    let deadline = std::time::Instant::now() + petramond::test_time::TEST_HARD_DEADLINE;
    let mut loaded = false;
    while std::time::Instant::now() < deadline {
        game.server.world.poll();
        if game.server.world.chunk_loaded(0, 0) {
            loaded = true;
            break;
        }
    }
    assert!(
        loaded,
        "chunk (0,0) failed to load within the hard deadline"
    );

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
fn placing_a_replaceable_block_on_itself_is_refused() {
    // Clicking short grass while HOLDING short grass must not "replace" it:
    // the rewrite would be invisible yet still eat one item off the hotbar.
    let mut game = game_on_empty_chunk();
    give(&mut game, ItemType::ShortGrass, 64);
    game.server.sessions[0].player.pos = Vec3::new(100.0, 64.0, 100.0); // park clear of the cell

    let g = IVec3::new(8, 100, 8);
    // Real ground below, so the refusal comes from the same-block rule and
    // not the substrate gate.
    game.server
        .world
        .set_block_world(g.x, g.y - 1, g.z, Block::Grass);
    game.server
        .world
        .set_block_world(g.x, g.y, g.z, Block::ShortGrass);

    game.server.sessions[0].look = Some(hit(g, IVec3::Y));
    assert!(
        !game.server.try_place_for_test(),
        "placing grass on grass is refused"
    );

    assert_eq!(
        Block::from_id(game.server.world.chunk_block(g.x, g.y, g.z)),
        Block::ShortGrass,
        "the clicked grass is untouched"
    );
    assert_eq!(
        game.server.sessions[0]
            .player
            .inventory
            .selected()
            .unwrap()
            .count,
        64,
        "no item was consumed"
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

/// The SHAPE half of the substrate gate, and the reason it cannot be a tag:
/// a TOP slab of stone is the same MATERIAL as stone and presents a complete
/// top face, yet it is not whole matter, so a mushroom must refuse it while
/// still taking the full cube beside it.
///
/// Both sides are asserted because a predictor that says yes where the server
/// says no is its own bug class — the player gets a ghost the next delta
/// deletes, which is exactly the "it breaks right after placing" report.
#[test]
fn a_full_cube_substrate_is_required_by_the_server_and_the_predictor() {
    use crate::game::tick::PlacePrediction;

    let mut game = game_on_empty_chunk();
    game.game.replica.insert_chunk_for_test(
        petramond::chunk::ChunkPos::new(0, 0),
        petramond::chunk::Chunk::new(0, 0),
    );
    game.server.sessions[0].player.pos = Vec3::new(100.0, 64.0, 100.0);
    game.game.player.pos = Vec3::new(100.0, 64.0, 100.0);

    // Column 4: a whole stone cube. Column 6: a stone TOP slab — complete top
    // face, shaped body.
    let whole = IVec3::new(4, 64, 4);
    let slab = IVec3::new(6, 64, 6);
    for world in [&mut game.server.world, &mut game.game.replica] {
        world.set_block_world(whole.x, whole.y, whole.z, Block::Stone);
        world.set_block_world(slab.x, slab.y, slab.z, Block::StoneSlab);
        world
            .section_at_world_mut_for_test(slab.x, slab.y, slab.z)
            .expect("the slab's section is loaded")
            .set_slab_state(
                slab.x.rem_euclid(16) as usize,
                slab.y.rem_euclid(16) as usize,
                slab.z.rem_euclid(16) as usize,
                SlabState::single(SlabSplit::Y, 1, Block::StoneSlab),
            );
    }
    assert_eq!(
        game.server
            .world
            .slab_state_at(slab.x, slab.y, slab.z)
            .layers[1],
        Block::StoneSlab,
        "fixture: the slab really occupies its cell's TOP half"
    );

    for (ground, expect) in [(whole, true), (slab, false)] {
        give(&mut game, ItemType::BrownMushroom, 1);
        game.sync_self_view_for_test();
        game.server.sessions[0].look = Some(hit(ground, IVec3::Y));
        assert_eq!(
            game.server.try_place_for_test(),
            expect,
            "server verdict on {ground:?}"
        );
        let predicted = !matches!(
            game.game.predict_place_at_for_test(ground, IVec3::Y, false),
            PlacePrediction::No
        );
        assert_eq!(
            predicted, expect,
            "the predictor must agree with the server on {ground:?}"
        );
        assert_eq!(
            game.game
                .replica
                .chunk_block(ground.x, ground.y + 1, ground.z)
                != Block::Air.0,
            expect,
            "a refused placement must leave no ghost on {ground:?}"
        );
    }
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
    let parts = game
        .server
        .world
        .cell_parts(p)
        .expect("a slab cell is composed");
    assert_eq!(
        game.server.part_drop_stacks(p, &parts),
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

/// A wall torch mounts only on a FULL support face: a stair's flat back
/// qualifies, its stepped open side does not; a lone slab layer's side does
/// not, a completed two-layer stack's side does.
#[test]
fn torch_support_face_cases() {
    struct Case {
        label: &'static str,
        /// Install the support block at the given cell; false = setup failed.
        setup: fn(&mut super::common::TestGame, IVec3) -> bool,
        /// The clicked face (torch cell = support + normal).
        normal: IVec3,
        expect_place: bool,
        expected_block: Block,
        expected_mount: Option<petramond::torch::TorchPlacement>,
    }
    fn stair(game: &mut super::common::TestGame, support: IVec3) -> bool {
        game.server.world.place_stair(
            support,
            Block::OakStairs,
            StairState::new(Facing::East, StairHalf::Bottom),
        )
    }
    let cases = [
        Case {
            label: "stair flat back supports a wall torch",
            setup: stair,
            normal: -IVec3::X,
            expect_place: true,
            expected_block: Block::Torch,
            expected_mount: Some(petramond::torch::TorchPlacement::West),
        },
        Case {
            label: "stair open side is not a full support face",
            setup: stair,
            normal: IVec3::X,
            expect_place: false,
            expected_block: Block::Air,
            expected_mount: None,
        },
        Case {
            label: "single slab side is not a full support face",
            setup: |game, support| {
                game.server.world.place_slab_layer(
                    support,
                    Block::DirtSlab,
                    petramond::slab::SlabSlot {
                        split: SlabSplit::Y,
                        index: 0,
                    },
                )
            },
            normal: IVec3::X,
            expect_place: false,
            expected_block: Block::Air,
            expected_mount: None,
        },
        Case {
            label: "full slab stack side supports a wall torch",
            setup: |game, support| {
                [(Block::DirtSlab, 0), (Block::CobblestoneSlab, 1)]
                    .into_iter()
                    .all(|(block, index)| {
                        game.server.world.place_slab_layer(
                            support,
                            block,
                            petramond::slab::SlabSlot {
                                split: SlabSplit::Y,
                                index,
                            },
                        )
                    })
            },
            normal: IVec3::X,
            expect_place: true,
            expected_block: Block::Torch,
            expected_mount: None,
        },
    ];

    for case in cases {
        let mut game = game_on_empty_chunk();
        game.server.sessions[0].player.pos = Vec3::new(100.0, 64.0, 100.0);
        let support = IVec3::new(4, 64, 4);
        assert!(
            (case.setup)(&mut game, support),
            "[{}] support setup must succeed",
            case.label
        );

        give(&mut game, ItemType::Torch, 1);
        game.server.sessions[0].look = Some(hit(support, case.normal));
        assert_eq!(
            game.server.try_place_for_test(),
            case.expect_place,
            "[{}] torch placement verdict",
            case.label
        );

        let torch = support + case.normal;
        assert_eq!(
            Block::from_id(game.server.world.chunk_block(torch.x, torch.y, torch.z)),
            case.expected_block,
            "[{}] the clicked face's adjacent cell",
            case.label
        );
        if let Some(mount) = case.expected_mount {
            assert_eq!(
                game.server.world.torch_placement(torch),
                mount,
                "[{}] the recorded wall mount",
                case.label
            );
        }
    }
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

/// Stacking a second slab into a cell must not take the sitting layer's
/// per-cell data with it.
///
/// A block write clears the cell's whole KV map, which is right when the cell
/// is replaced and wrong here: the placement AUGMENTS the cell, so the layer
/// already sitting there keeps its own data (its dye) while the newcomer's
/// lands on the part it filled. Without this a white slab under an orange one
/// came out orange on both halves — and dropped two orange slabs.
#[test]
fn stacking_a_slab_keeps_the_sitting_layers_data() {
    use petramond::block::{part_kv_key, TINT_KV_KEY};

    let mut game = game_on_empty_chunk();
    game.server.sessions[0].player.pos = Vec3::new(100.0, 64.0, 100.0);
    let p = IVec3::new(4, 64, 4);

    give(&mut game, ItemType::WoolSlab, 1);
    game.server.sessions[0].look = Some(hit(p - IVec3::Y, IVec3::Y));
    assert!(game.server.try_place_for_test(), "first slab places");

    // Dye the sitting bottom layer (what the carry courier would have written).
    let white = vec![255u8, 255, 255];
    assert!(game
        .server
        .world
        .cell_kv_set(p.x, p.y, p.z, TINT_KV_KEY.to_owned(), white.clone()));

    give(&mut game, ItemType::WoolSlab, 1);
    game.server.sessions[0].look = Some(hit(p, IVec3::Y));
    assert!(game.server.try_place_for_test(), "second slab stacks");

    let state = game.server.world.slab_state_at(p.x, p.y, p.z);
    assert_eq!(state.layers, [Block::WoolSlab, Block::WoolSlab]);
    assert_eq!(
        game.server
            .world
            .cell_kv_get(p.x, p.y, p.z, TINT_KV_KEY)
            .map(<[u8]>::to_vec),
        Some(white),
        "the bottom layer's data must survive the stacking write"
    );
    // The newcomer carried nothing, so the layer it filled stays plain — the
    // two layers are addressed independently.
    assert_eq!(
        game.server
            .world
            .cell_kv_get(p.x, p.y, p.z, &part_kv_key(TINT_KV_KEY, 1)),
        None,
        "the newcomer's part must not inherit the sitting layer's data"
    );
}

/// The slab family's part numbering has to be ONE numbering: the boxes the
/// mesher tints, the parts the drop courier reads, and the part a placement
/// claims all address the same layer. They are three separate impls, so
/// nothing but a test stops them drifting — and drift here does not crash, it
/// just quietly paints or drops the wrong layer.
#[test]
fn slab_parts_and_boxes_agree_on_the_layer_numbering() {
    use petramond::block::{ShapeCtx, NO_PART_TINT};

    let mut game = game_on_empty_chunk();
    let p = IVec3::new(4, 64, 4);
    let world = &mut game.server.world;
    for (index, block) in [(0, Block::DirtSlab), (1, Block::CobblestoneSlab)] {
        assert!(world.place_slab_layer(
            p,
            block,
            petramond::slab::SlabSlot {
                split: SlabSplit::Y,
                index,
            },
        ));
    }

    let block = Block::from_id(world.chunk_block(p.x, p.y, p.z));
    let k = block.shape_kind_def();
    let parts = world.cell_parts(p).expect("a slab cell is composed");
    assert_eq!(
        parts,
        vec![(0, Block::DirtSlab), (1, Block::CobblestoneSlab)],
        "slot index IS the part number"
    );

    let tint_for = |_: petramond::tile::Tile| [1.0f32; 3];
    let mut boxes = vec![];
    k.render.boxes(
        &ShapeCtx {
            nb: world,
            pos: p,
            block,
            params: &k.params,
            tint_for: &tint_for,
            part_tint: NO_PART_TINT,
        },
        &mut boxes,
    );
    assert_eq!(boxes.len(), parts.len(), "one box per part");
    for (b, &(part, _)) in boxes.iter().zip(parts.iter()) {
        assert_eq!(b.part, part, "box parts must match the family's part list");
        // Part 0 is the lower half of the split axis, part 1 the upper — the
        // ordering the placement plan and the mesher both assume.
        let lower = b.aabb.min[1] < 0.25;
        assert_eq!(lower, part == 0, "part {part} sits in the wrong half");
    }
}
