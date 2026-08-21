//! The off-hand slot: the two-pass use-click ladder (main hand first, the
//! off hand only when nothing claimed), the F swap through the ordered
//! menu-action stream, the client's mirrored two-pass prediction, and the
//! off-hand GUI cell's click routing.

use super::common::{game, game_on_empty_chunk, hit};
use crate::game::tick::PlacePrediction;
use petramond::events::tick::TickEvents;
use petramond_math::math::IVec3;
use petramond_world::block::Block;
use petramond_world::gui_state::{MenuSlot, PointerButton};
use petramond_world::inventory::Inventory;
use petramond_world::item::{ItemStack, ItemType};

// The stick is an item-only row: no block link, no `use`, no food — inert on
// every rung of the use-click ladder, so a main hand holding one predictably
// claims nothing and the second pass runs.
fn stick() -> ItemType {
    ItemType::Stick
}

/// Session inventory with `main` selected in hotbar slot 0 and `off` in the
/// off-hand slot.
fn hands(main: Option<ItemStack>, off: Option<ItemStack>) -> Inventory {
    let mut inv = Inventory::new();
    if let Some(stack) = main {
        inv.add(stack);
    }
    *inv.off_hand_mut() = off;
    inv
}

#[test]
fn off_hand_places_when_the_main_hand_cannot_act() {
    let mut game = game_on_empty_chunk();
    let floor = IVec3::new(4, 64, 4);
    game.server
        .world
        .set_block_world(floor.x, floor.y, floor.z, Block::Stone);
    game.server.sessions[0].player.inventory = hands(
        Some(ItemStack::new(stick(), 1)),
        Some(ItemStack::new(ItemType::Dirt, 2)),
    );
    game.server.sessions[0].look = Some(hit(floor, IVec3::Y));
    game.server.queue_place_click_for_test(0);

    let mut events = TickEvents::default();
    game.server.tick_place(0, &mut events);

    let above = floor + IVec3::Y;
    assert_eq!(
        Block::from_id(game.server.world.chunk_block(above.x, above.y, above.z)),
        Block::Dirt,
        "the ladder's second pass places the off-hand block"
    );
    let inv = &game.server.sessions[0].player.inventory;
    assert_eq!(
        inv.off_hand().map(|s| s.count),
        Some(1),
        "the OFF hand paid for the placement"
    );
    assert_eq!(
        inv.selected().map(|s| s.item),
        Some(stick()),
        "the main hand is untouched"
    );
    let p = events.player_at(0);
    assert_eq!(p.placed_block, Some(Block::Dirt));
    assert!(
        p.click_off_hand,
        "the one-shots carry the acting hand for presentation"
    );
}

#[test]
fn the_main_hand_wins_when_both_hands_can_place() {
    let mut game = game_on_empty_chunk();
    let floor = IVec3::new(4, 64, 4);
    game.server
        .world
        .set_block_world(floor.x, floor.y, floor.z, Block::Stone);
    game.server.sessions[0].player.inventory = hands(
        Some(ItemStack::new(ItemType::Dirt, 2)),
        Some(ItemStack::new(ItemType::Stone, 2)),
    );
    game.server.sessions[0].look = Some(hit(floor, IVec3::Y));
    game.server.queue_place_click_for_test(0);

    let mut events = TickEvents::default();
    game.server.tick_place(0, &mut events);

    let above = floor + IVec3::Y;
    assert_eq!(
        Block::from_id(game.server.world.chunk_block(above.x, above.y, above.z)),
        Block::Dirt,
        "the main hand acts whenever it can"
    );
    let inv = &game.server.sessions[0].player.inventory;
    assert_eq!(inv.selected().map(|s| s.count), Some(1));
    assert_eq!(
        inv.off_hand().map(|s| s.count),
        Some(2),
        "the off hand never pays when the main hand acted"
    );
    assert!(!events.player_at(0).click_off_hand);
}

#[test]
fn an_empty_off_hand_never_runs_a_second_pass() {
    let mut game = game_on_empty_chunk();
    let floor = IVec3::new(4, 64, 4);
    game.server
        .world
        .set_block_world(floor.x, floor.y, floor.z, Block::Stone);
    game.server.sessions[0].player.inventory = hands(Some(ItemStack::new(stick(), 1)), None);
    game.server.sessions[0].look = Some(hit(floor, IVec3::Y));
    game.server.queue_place_click_for_test(0);

    let mut events = TickEvents::default();
    game.server.tick_place(0, &mut events);

    let p = events.player_at(0);
    assert!(p.placed_block.is_none() && !p.interacted && !p.used_item);
    assert_eq!(
        Block::from_id(game.server.world.chunk_block(floor.x, floor.y + 1, floor.z)),
        Block::Air,
        "an inert click with an empty off-hand does nothing"
    );
}

/// A changed OFF hand between receipt and the Placement stage denies the
/// whole click, exactly like a changed hotbar selection: which hand acts is
/// decided during dispatch, so the guard covers both captures.
#[test]
fn an_off_hand_change_after_receipt_denies_the_click() {
    let mut game = game_on_empty_chunk();
    let floor = IVec3::new(4, 64, 4);
    game.server
        .world
        .set_block_world(floor.x, floor.y, floor.z, Block::Stone);
    game.server.sessions[0].player.inventory = hands(
        Some(ItemStack::new(stick(), 1)),
        Some(ItemStack::new(ItemType::Dirt, 2)),
    );
    game.server.sessions[0].look = Some(hit(floor, IVec3::Y));
    game.server.queue_place_click_for_test(0);

    // The off-hand item changes before the tick consumes the click.
    *game.server.sessions[0].player.inventory.off_hand_mut() =
        Some(ItemStack::new(ItemType::Stone, 2));
    let mut events = TickEvents::default();
    game.server.tick_place(0, &mut events);

    assert_eq!(
        Block::from_id(game.server.world.chunk_block(floor.x, floor.y + 1, floor.z)),
        Block::Air,
        "the superseded click must not act on an item it never aimed"
    );
    assert_eq!(
        game.server.sessions[0]
            .player
            .inventory
            .off_hand()
            .map(|s| s.count),
        Some(2)
    );
}

#[test]
fn swap_off_hand_swaps_the_selected_stack_and_predicts_it() {
    let mut game = game();
    game.server.sessions[0].player.inventory = hands(Some(ItemStack::new(ItemType::Dirt, 5)), None);
    game.sync_self_view_for_test();

    game.game.swap_off_hand();
    // The prediction lands immediately on the replicated view…
    assert_eq!(
        game.self_view.inventory.off_hand().map(|s| s.item),
        Some(ItemType::Dirt)
    );
    assert!(game.self_view.inventory.selected().is_none());
    // …and the authoritative swap applies through the ordered menu stream.
    game.apply_latched_actions_for_test();
    let inv = &game.server.sessions[0].player.inventory;
    assert_eq!(inv.off_hand().map(|s| s.count), Some(5));
    assert!(inv.selected().is_none());

    // F again swaps back (an empty hand takes the off-hand stack out).
    game.game.swap_off_hand();
    game.apply_latched_actions_for_test();
    let inv = &game.server.sessions[0].player.inventory;
    assert!(inv.off_hand().is_none());
    assert_eq!(inv.selected().map(|s| s.count), Some(5));
}

#[test]
fn the_click_verdict_falls_through_to_the_off_hand() {
    let mut game = game_on_empty_chunk();
    game.game.replica.insert_chunk_for_test(
        petramond_world::chunk::ChunkPos::new(0, 0),
        petramond_world::chunk::Chunk::new(0, 0),
    );
    let floor = IVec3::new(8, 63, 8);
    assert!(game
        .game
        .replica
        .set_block_world(floor.x, floor.y, floor.z, Block::Stone));
    // Park the body clear of the place cell so the body gate cannot refuse.
    game.game.player.pos = petramond_math::math::Vec3::new(100.0, 64.0, 100.0);
    game.server.sessions[0].player.inventory = hands(
        Some(ItemStack::new(stick(), 1)),
        Some(ItemStack::new(ItemType::Dirt, 3)),
    );
    game.sync_self_view_for_test();

    let (jabbed, off_hand, place) =
        game.game
            .predict_click_verdict_at_for_test(floor, IVec3::Y, false);
    assert!(jabbed, "the off-hand pass predicts the placement");
    assert!(off_hand, "the verdict names the acting hand");
    assert!(matches!(place, PlacePrediction::Predicted(_)));
    assert_eq!(
        game.self_view.inventory.off_hand().map(|s| s.count),
        Some(2),
        "the predicted decrement pays from the off-hand mirror"
    );
    assert_eq!(
        game.self_view.inventory.selected().map(|s| s.item),
        Some(stick())
    );
    let above = floor + IVec3::Y;
    assert_eq!(
        Block::from_id(game.game.replica.chunk_block(above.x, above.y, above.z)),
        Block::Dirt,
        "the ghost writes the replica like any predicted place"
    );
}

#[test]
fn the_off_hand_menu_cell_clicks_like_a_plain_slot_on_both_mirrors() {
    let mut game = game();
    game.server.sessions[0].player.inventory =
        hands(None, Some(ItemStack::new(ItemType::Stone, 7)));
    game.sync_self_view_for_test();

    game.menu_click(MenuSlot::OffHand, PointerButton::Primary, false, false);
    // Predicted pickup onto the cursor…
    assert_eq!(game.self_view.inventory.cursor().map(|s| s.count), Some(7));
    assert!(game.self_view.inventory.off_hand().is_none());
    // …and the authoritative decode agrees.
    game.apply_latched_actions_for_test();
    let inv = &game.server.sessions[0].player.inventory;
    assert_eq!(inv.cursor().map(|s| s.count), Some(7));
    assert!(inv.off_hand().is_none());

    // A second plain click deposits the cursor stack back into the cell.
    game.menu_click(MenuSlot::OffHand, PointerButton::Primary, false, false);
    game.apply_latched_actions_for_test();
    let inv = &game.server.sessions[0].player.inventory;
    assert_eq!(
        inv.off_hand().map(|s| s.count),
        Some(7),
        "a plain click deposits the cursor stack into the off-hand"
    );
    assert!(inv.cursor().is_none());

    // A shift-click ships the stack into the grid on both mirrors.
    game.menu_click(MenuSlot::OffHand, PointerButton::Primary, true, false);
    game.apply_latched_actions_for_test();
    let inv = &game.server.sessions[0].player.inventory;
    assert!(inv.off_hand().is_none(), "shift ships it into the grid");
    assert_eq!(super::common::count_item(inv, ItemType::Stone), 7);
    assert_eq!(
        game.self_view.inventory.off_hand(),
        None,
        "the predicted mirror agrees"
    );
}

/// World pickup tops up a SAME-item off-hand first; a different item never
/// touches it — driven through the real planner + collector
/// (`item_pickup_tick`), not just the `Inventory` routing.
#[test]
fn pickup_refills_a_matching_off_hand_before_the_grid() {
    use petramond::entity::DroppedItem;
    use petramond::world::ITEM_PICKUP_DELAY_TICKS;

    let mut game = game();
    game.server.sessions[0].player.inventory =
        hands(None, Some(ItemStack::new(ItemType::Dirt, 60)));
    let centre = game.server.sessions[0].player.body_center();
    let mut drop = DroppedItem::new(centre, ItemStack::new(ItemType::Dirt, 10), 1);
    drop.ticks_lived = ITEM_PICKUP_DELAY_TICKS;
    game.server.world.spawn_item(drop);
    game.server.world.tick_item_lifetime();
    game.server.item_pickup_tick(0);
    let inv = &game.server.sessions[0].player.inventory;
    assert_eq!(
        inv.off_hand().map(|s| s.count),
        Some(64),
        "the matching off-hand stack fills first"
    );
    assert_eq!(super::common::count_item(inv, ItemType::Dirt), 6);

    // A different item routes past the off-hand entirely.
    let mut game = super::common::game();
    game.server.sessions[0].player.inventory =
        hands(None, Some(ItemStack::new(ItemType::Stone, 1)));
    let centre = game.server.sessions[0].player.body_center();
    let mut drop = DroppedItem::new(centre, ItemStack::new(ItemType::Dirt, 3), 1);
    drop.ticks_lived = ITEM_PICKUP_DELAY_TICKS;
    game.server.world.spawn_item(drop);
    game.server.world.tick_item_lifetime();
    game.server.item_pickup_tick(0);
    let inv = &game.server.sessions[0].player.inventory;
    assert_eq!(
        inv.off_hand().map(|s| (s.item, s.count)),
        Some((ItemType::Stone, 1)),
        "a foreign stack never enters the off-hand"
    );
    assert_eq!(super::common::count_item(inv, ItemType::Dirt), 3);
}

/// The menu F: hovering a slot swaps it with the off-hand, predicted on the
/// mirrors with the same primitives the server's decode runs.
#[test]
fn menu_f_swaps_the_hovered_inventory_slot_on_both_mirrors() {
    let mut game = game();
    let mut inv = Inventory::new();
    *inv.slot_mut(12).expect("in range") = Some(ItemStack::new(ItemType::Stone, 7));
    game.server.sessions[0].player.inventory = inv;
    game.sync_self_view_for_test();

    game.game.menu_swap_off_hand(MenuSlot::Inventory(12));
    assert_eq!(
        game.self_view.inventory.off_hand().map(|s| s.count),
        Some(7),
        "the prediction lands immediately"
    );
    assert!(game.self_view.inventory.slot(12).is_none());
    game.apply_latched_actions_for_test();
    let inv = &game.server.sessions[0].player.inventory;
    assert_eq!(inv.off_hand().map(|s| s.count), Some(7));
    assert!(inv.slot(12).is_none());
}

/// The hovered-slot swap against an open CONTAINER cell: a chest cell swaps
/// plainly; the furnace's take-only output and its fuel filter refuse the
/// whole gesture (all-or-nothing — a half-executed swap reads as item loss),
/// identically on the prediction and the authority.
#[test]
fn menu_f_swap_respects_container_slot_specs() {
    use petramond::events::tick::TickEvents;
    use petramond_math::math::IVec3;

    // Chest: a plain cell swaps whole, on both mirrors.
    let mut game = game_on_empty_chunk();
    let pos = IVec3::new(3, 64, 3);
    game.server.world.set_block_world(3, 64, 3, Block::Chest);
    game.server
        .world
        .insert_chest(pos, petramond_world::block_model::DEFAULT_MODEL_FACING);
    game.server.sessions[0].player.inventory = hands(None, Some(ItemStack::new(ItemType::Dirt, 5)));
    let mut ev = TickEvents::default();
    game.server.open_chest_screen_for(0, pos, &mut ev);
    game.sync_self_view_for_test();
    game.sync_menu_view_for_test();

    game.game.menu_swap_off_hand(MenuSlot::Container(0));
    assert!(game.self_view.inventory.off_hand().is_none());
    assert_eq!(
        game.menu_view.container.as_ref().unwrap().slots[0].map(|s| s.count),
        Some(5),
        "the predicted mirror holds the deposited stack"
    );
    game.apply_latched_actions_for_test();
    assert_eq!(
        game.server
            .world
            .container_at(pos)
            .and_then(|c| c.slots[0])
            .map(|s| s.count),
        Some(5),
        "the authoritative chest cell agrees"
    );
    assert!(game.server.sessions[0]
        .player
        .inventory
        .off_hand()
        .is_none());

    // Furnace: the fuel filter and the take-only output refuse the deposit
    // half, so the WHOLE swap refuses.
    let mut game = game_on_empty_chunk();
    let pos = IVec3::new(3, 64, 3);
    game.server.world.set_block_world(3, 64, 3, Block::Furnace);
    game.server
        .world
        .insert_furnace(pos, petramond_world::block_model::DEFAULT_MODEL_FACING);
    game.server.sessions[0].player.inventory = hands(None, Some(ItemStack::new(ItemType::Dirt, 5)));
    game.server.open_furnace_screen_for(0, pos);
    game.sync_self_view_for_test();
    game.sync_menu_view_for_test();

    for cell in [
        petramond_world::furnace::SLOT_FUEL,
        petramond_world::furnace::SLOT_OUTPUT,
    ] {
        game.game.menu_swap_off_hand(MenuSlot::Container(cell));
        assert_eq!(
            game.self_view.inventory.off_hand().map(|s| s.count),
            Some(5),
            "cell {cell}: the refusing spec swaps nothing on the mirror"
        );
        game.apply_latched_actions_for_test();
        assert_eq!(
            game.server.sessions[0]
                .player
                .inventory
                .off_hand()
                .map(|s| s.count),
            Some(5),
            "cell {cell}: the authority refuses identically"
        );
        assert!(game
            .server
            .world
            .container_at(pos)
            .and_then(|c| c.slots[cell])
            .is_none());
    }
}

#[test]
fn death_spills_the_off_hand_with_the_rest() {
    let mut game = game_on_empty_chunk();
    game.server.sessions[0].player.inventory = hands(
        Some(ItemStack::new(ItemType::Dirt, 3)),
        Some(ItemStack::new(ItemType::Stone, 4)),
    );
    let mut events = TickEvents::default();
    assert!(game.server.damage_player(
        0,
        petramond::player::MAX_HEALTH,
        petramond::events::DamageSource::Fall,
        None,
        &mut events,
    ));
    assert!(game.server.sessions[0]
        .player
        .inventory
        .off_hand()
        .is_none());
    let spilled: u32 = game
        .server
        .world
        .item_entities()
        .iter()
        .map(|it| it.stack.count as u32)
        .sum();
    assert_eq!(spilled, 7, "both hands' stacks land in the corpse pile");
}
