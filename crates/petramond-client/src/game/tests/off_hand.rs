use super::common::{game, game_on_empty_chunk, hit};
use crate::game::tick::PlacePrediction;
use petramond::events::tick::TickEvents;
use petramond_math::math::IVec3;
use petramond_math::world_pos::WorldPos;
use petramond_world::block::Block;
use petramond_world::gui_state::{MenuSlot, PointerButton};
use petramond_world::inventory::Inventory;
use petramond_world::item::{ItemStack, ItemType};

fn stick() -> ItemType {
    ItemType::Stick
}

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
    game.server_world_mut()
        .set_block_world(floor.x, floor.y, floor.z, Block::Stone);
    game.server_player_mut().inventory = hands(
        Some(ItemStack::new(stick(), 1)),
        Some(ItemStack::new(ItemType::Dirt, 2)),
    );
    game.session_mut().input_mut().look = Some(hit(floor, IVec3::Y));
    game.sim_mut().queue_place_click_for_test(0);

    let mut events = TickEvents::default();
    game.sim_mut().tick_place(0, &mut events);

    let above = floor + IVec3::Y;
    assert_eq!(
        Block::from_id(
            game.server_world()
                .data()
                .chunk_block(above.x, above.y, above.z)
        ),
        Block::Dirt,
        "the ladder's second pass places the off-hand block"
    );
    let inv = &game.server_player().inventory;
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
    game.server_world_mut()
        .set_block_world(floor.x, floor.y, floor.z, Block::Stone);
    game.server_player_mut().inventory = hands(
        Some(ItemStack::new(ItemType::Dirt, 2)),
        Some(ItemStack::new(ItemType::Stone, 2)),
    );
    game.session_mut().input_mut().look = Some(hit(floor, IVec3::Y));
    game.sim_mut().queue_place_click_for_test(0);

    let mut events = TickEvents::default();
    game.sim_mut().tick_place(0, &mut events);

    let above = floor + IVec3::Y;
    assert_eq!(
        Block::from_id(
            game.server_world()
                .data()
                .chunk_block(above.x, above.y, above.z)
        ),
        Block::Dirt,
        "the main hand acts whenever it can"
    );
    let inv = &game.server_player().inventory;
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
    game.server_world_mut()
        .set_block_world(floor.x, floor.y, floor.z, Block::Stone);
    game.server_player_mut().inventory = hands(Some(ItemStack::new(stick(), 1)), None);
    game.session_mut().input_mut().look = Some(hit(floor, IVec3::Y));
    game.sim_mut().queue_place_click_for_test(0);

    let mut events = TickEvents::default();
    game.sim_mut().tick_place(0, &mut events);

    let p = events.player_at(0);
    assert!(p.placed_block.is_none() && !p.interacted && !p.used_item);
    assert_eq!(
        Block::from_id(
            game.server_world()
                .data()
                .chunk_block(floor.x, floor.y + 1, floor.z)
        ),
        Block::Air,
        "an inert click with an empty off-hand does nothing"
    );
}

#[test]
fn an_off_hand_change_after_receipt_denies_the_click() {
    let mut game = game_on_empty_chunk();
    let floor = IVec3::new(4, 64, 4);
    game.server_world_mut()
        .set_block_world(floor.x, floor.y, floor.z, Block::Stone);
    game.server_player_mut().inventory = hands(
        Some(ItemStack::new(stick(), 1)),
        Some(ItemStack::new(ItemType::Dirt, 2)),
    );
    game.session_mut().input_mut().look = Some(hit(floor, IVec3::Y));
    game.sim_mut().queue_place_click_for_test(0);

    *game.server_player_mut().inventory.off_hand_mut() = Some(ItemStack::new(ItemType::Stone, 2));
    let mut events = TickEvents::default();
    game.sim_mut().tick_place(0, &mut events);

    assert_eq!(
        Block::from_id(
            game.server_world()
                .data()
                .chunk_block(floor.x, floor.y + 1, floor.z)
        ),
        Block::Air,
        "the superseded click must not act on an item it never aimed"
    );
    assert_eq!(
        game.server_player().inventory.off_hand().map(|s| s.count),
        Some(2)
    );
}

#[test]
fn swap_off_hand_swaps_the_selected_stack_and_predicts_it() {
    let mut game = game();
    game.server_player_mut().inventory = hands(Some(ItemStack::new(ItemType::Dirt, 5)), None);
    game.sync_self_view_for_test();

    game.game.swap_off_hand();
    assert_eq!(
        game.replica.self_view.inventory.off_hand().map(|s| s.item),
        Some(ItemType::Dirt)
    );
    assert!(game.replica.self_view.inventory.selected().is_none());
    game.apply_latched_actions_for_test();
    let inv = &game.server_player().inventory;
    assert_eq!(inv.off_hand().map(|s| s.count), Some(5));
    assert!(inv.selected().is_none());

    game.game.swap_off_hand();
    game.apply_latched_actions_for_test();
    let inv = &game.server_player().inventory;
    assert!(inv.off_hand().is_none());
    assert_eq!(inv.selected().map(|s| s.count), Some(5));
}

#[test]
fn the_click_verdict_falls_through_to_the_off_hand() {
    let mut game = game_on_empty_chunk();
    game.game.replica.world.insert_chunk_for_test(
        petramond_world::chunk::ChunkPos::new(0, 0),
        petramond_world::chunk::Chunk::new(0, 0),
    );
    let floor = IVec3::new(8, 63, 8);
    assert!(game
        .game
        .replica
        .world
        .set_block_world(floor.x, floor.y, floor.z, Block::Stone));
    game.game.local.player.pos = WorldPos::new(100.0, 64.0, 100.0);
    game.server_player_mut().inventory = hands(
        Some(ItemStack::new(stick(), 1)),
        Some(ItemStack::new(ItemType::Dirt, 3)),
    );
    game.sync_self_view_for_test();

    let verdict = game
        .game
        .predict_click_verdict_at_for_test(floor, IVec3::Y, false);
    let (jabbed, off_hand, place) = (verdict.consumed, verdict.off_hand, verdict.place);
    assert!(jabbed, "the off-hand pass predicts the placement");
    assert!(verdict.places, "a predicted placement is a place jab");
    assert!(off_hand, "the verdict names the acting hand");
    assert!(matches!(place, PlacePrediction::Predicted(_)));
    assert_eq!(
        game.replica.self_view.inventory.off_hand().map(|s| s.count),
        Some(2),
        "the predicted decrement pays from the off-hand mirror"
    );
    assert_eq!(
        game.replica.self_view.inventory.selected().map(|s| s.item),
        Some(stick())
    );
    let above = floor + IVec3::Y;
    assert_eq!(
        Block::from_id(
            game.game
                .replica
                .world
                .data()
                .chunk_block(above.x, above.y, above.z)
        ),
        Block::Dirt,
        "the ghost writes the replica like any predicted place"
    );
}

#[test]
fn the_off_hand_menu_cell_clicks_like_a_plain_slot_on_both_mirrors() {
    let mut game = game();
    game.server_player_mut().inventory = hands(None, Some(ItemStack::new(ItemType::Stone, 7)));
    game.sync_self_view_for_test();

    game.menu_click(MenuSlot::OffHand, PointerButton::Primary, false, false);
    assert_eq!(
        game.replica.self_view.inventory.cursor().map(|s| s.count),
        Some(7)
    );
    assert!(game.replica.self_view.inventory.off_hand().is_none());
    game.apply_latched_actions_for_test();
    let inv = &game.server_player().inventory;
    assert_eq!(inv.cursor().map(|s| s.count), Some(7));
    assert!(inv.off_hand().is_none());

    game.menu_click(MenuSlot::OffHand, PointerButton::Primary, false, false);
    game.apply_latched_actions_for_test();
    let inv = &game.server_player().inventory;
    assert_eq!(
        inv.off_hand().map(|s| s.count),
        Some(7),
        "a plain click deposits the cursor stack into the off-hand"
    );
    assert!(inv.cursor().is_none());

    game.menu_click(MenuSlot::OffHand, PointerButton::Primary, true, false);
    game.apply_latched_actions_for_test();
    let inv = &game.server_player().inventory;
    assert!(inv.off_hand().is_none(), "shift ships it into the grid");
    assert_eq!(super::common::count_item(inv, ItemType::Stone), 7);
    assert_eq!(
        game.replica.self_view.inventory.off_hand(),
        None,
        "the predicted mirror agrees"
    );
}

#[test]
fn pickup_refills_a_matching_off_hand_before_the_grid() {
    use petramond::entity::DroppedItem;
    use petramond::world::ITEM_PICKUP_DELAY_TICKS;

    let mut game = game();
    game.server_player_mut().inventory = hands(None, Some(ItemStack::new(ItemType::Dirt, 60)));
    let centre = game.server_player().body_center();
    let mut drop = DroppedItem::new(centre, ItemStack::new(ItemType::Dirt, 10), 1);
    drop.ticks_lived = ITEM_PICKUP_DELAY_TICKS;
    game.server_world_mut().spawn_item(drop);
    game.server_world_mut().tick_item_lifetime();
    game.sim_mut().item_pickup_tick(0);
    let inv = &game.server_player().inventory;
    assert_eq!(
        inv.off_hand().map(|s| s.count),
        Some(64),
        "the matching off-hand stack fills first"
    );
    assert_eq!(super::common::count_item(inv, ItemType::Dirt), 6);

    let mut game = super::common::game();
    game.server_player_mut().inventory = hands(None, Some(ItemStack::new(ItemType::Stone, 1)));
    let centre = game.server_player().body_center();
    let mut drop = DroppedItem::new(centre, ItemStack::new(ItemType::Dirt, 3), 1);
    drop.ticks_lived = ITEM_PICKUP_DELAY_TICKS;
    game.server_world_mut().spawn_item(drop);
    game.server_world_mut().tick_item_lifetime();
    game.sim_mut().item_pickup_tick(0);
    let inv = &game.server_player().inventory;
    assert_eq!(
        inv.off_hand().map(|s| (s.item, s.count)),
        Some((ItemType::Stone, 1)),
        "a foreign stack never enters the off-hand"
    );
    assert_eq!(super::common::count_item(inv, ItemType::Dirt), 3);
}

#[test]
fn menu_f_swaps_the_hovered_inventory_slot_on_both_mirrors() {
    let mut game = game();
    let mut inv = Inventory::new();
    *inv.slot_mut(12).expect("in range") = Some(ItemStack::new(ItemType::Stone, 7));
    game.server_player_mut().inventory = inv;
    game.sync_self_view_for_test();

    game.game.menu_swap_off_hand(MenuSlot::Inventory(12));
    assert_eq!(
        game.replica.self_view.inventory.off_hand().map(|s| s.count),
        Some(7),
        "the prediction lands immediately"
    );
    assert!(game.replica.self_view.inventory.slot(12).is_none());
    game.apply_latched_actions_for_test();
    let inv = &game.server_player().inventory;
    assert_eq!(inv.off_hand().map(|s| s.count), Some(7));
    assert!(inv.slot(12).is_none());
}

#[test]
fn menu_f_swap_respects_container_slot_specs() {
    use petramond::events::tick::TickEvents;
    use petramond_math::math::IVec3;

    let mut game = game_on_empty_chunk();
    let pos = IVec3::new(3, 64, 3);
    game.server_world_mut()
        .set_block_world(3, 64, 3, Block::Chest);
    game.server_world_mut()
        .insert_chest(pos, petramond_world::block_model::DEFAULT_MODEL_FACING);
    game.server_player_mut().inventory = hands(None, Some(ItemStack::new(ItemType::Dirt, 5)));
    let mut ev = TickEvents::default();
    game.sim_mut().open_chest_screen_for(0, pos, &mut ev);
    game.sync_self_view_for_test();
    game.sync_menu_view_for_test();

    game.game.menu_swap_off_hand(MenuSlot::Container(0));
    assert!(game.replica.self_view.inventory.off_hand().is_none());
    assert_eq!(
        game.replica.menu_view.container.as_ref().unwrap().slots[0].map(|s| s.count),
        Some(5),
        "the predicted mirror holds the deposited stack"
    );
    game.apply_latched_actions_for_test();
    assert_eq!(
        game.server_world()
            .container_at(pos)
            .and_then(|c| c.slots[0])
            .map(|s| s.count),
        Some(5),
        "the authoritative chest cell agrees"
    );
    assert!(game.server_player().inventory.off_hand().is_none());

    let mut game = game_on_empty_chunk();
    let pos = IVec3::new(3, 64, 3);
    game.server_world_mut()
        .set_block_world(3, 64, 3, Block::Furnace);
    game.server_world_mut()
        .insert_furnace(pos, petramond_world::block_model::DEFAULT_MODEL_FACING);
    game.server_player_mut().inventory = hands(None, Some(ItemStack::new(ItemType::Dirt, 5)));
    game.sim_mut().open_furnace_screen_for(0, pos);
    game.sync_self_view_for_test();
    game.sync_menu_view_for_test();

    for cell in [
        petramond_world::furnace::SLOT_FUEL,
        petramond_world::furnace::SLOT_OUTPUT,
    ] {
        game.game.menu_swap_off_hand(MenuSlot::Container(cell));
        assert_eq!(
            game.replica.self_view.inventory.off_hand().map(|s| s.count),
            Some(5),
            "cell {cell}: the refusing spec swaps nothing on the mirror"
        );
        game.apply_latched_actions_for_test();
        assert_eq!(
            game.server_player().inventory.off_hand().map(|s| s.count),
            Some(5),
            "cell {cell}: the authority refuses identically"
        );
        assert!(game
            .server_world()
            .container_at(pos)
            .and_then(|c| c.slots[cell])
            .is_none());
    }
}

#[test]
fn death_spills_the_off_hand_with_the_rest() {
    let mut game = game_on_empty_chunk();
    game.server_player_mut().inventory = hands(
        Some(ItemStack::new(ItemType::Dirt, 3)),
        Some(ItemStack::new(ItemType::Stone, 4)),
    );
    let mut events = TickEvents::default();
    assert!(game.sim_mut().damage_player(
        0,
        petramond::player::MAX_HEALTH,
        petramond::events::DamageSource::Fall,
        None,
        &mut events,
    ));
    assert!(game.server_player().inventory.off_hand().is_none());
    let spilled: u32 = game
        .server_world()
        .item_entities()
        .iter()
        .map(|it| it.stack.count as u32)
        .sum();
    assert_eq!(spilled, 7, "both hands' stacks land in the corpse pile");
}

#[test]
fn a_food_click_jabs_for_the_block_that_claims_it_and_otherwise_eats_without_one() {
    let food = ItemType::all()
        .iter()
        .copied()
        .find(|i| i.food().is_some() && i.as_block().is_none())
        .expect("a registered non-block food");
    let mut game = game_on_empty_chunk();
    game.game.replica.world.insert_chunk_for_test(
        petramond_world::chunk::ChunkPos::new(0, 0),
        petramond_world::chunk::Chunk::new(0, 0),
    );
    let chest = IVec3::new(8, 64, 8);
    let floor = IVec3::new(4, 63, 4);
    assert!(game
        .game
        .replica
        .world
        .set_block_world(chest.x, chest.y, chest.z, Block::Chest));
    assert!(game
        .game
        .replica
        .world
        .set_block_world(floor.x, floor.y, floor.z, Block::Stone));
    game.game.local.player.pos = WorldPos::new(100.0, 64.0, 100.0);
    game.server_player_mut().inventory = hands(
        Some(ItemStack::new(food, 3)),
        Some(ItemStack::new(ItemType::Dirt, 3)),
    );
    game.sync_self_view_for_test();

    let at_chest = game
        .game
        .predict_click_verdict_at_for_test(chest, IVec3::Y, false);
    assert!(
        at_chest.consumed && !at_chest.presents_itself && !at_chest.places && !at_chest.off_hand,
        "the chest claims the click before the eat: the hand jabs"
    );
    let at_floor = game
        .game
        .predict_click_verdict_at_for_test(floor, IVec3::Y, false);
    assert!(
        at_floor.consumed && at_floor.presents_itself,
        "the eat claims the click: its raise presents it, not a jab"
    );
    assert!(
        !at_floor.off_hand && matches!(at_floor.place, PlacePrediction::No),
        "a consumed click never reaches the off hand's dirt"
    );
}
