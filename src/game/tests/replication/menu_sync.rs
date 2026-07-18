//! Inventory revision, crafting outputs, and the session-boundary menu
//! contracts: chest viewer events, `menu_sync`, mod GUI state, open-screen.

use super::common::{count_item, filled_inventory, game, game_on_empty_chunk};
use super::pump_one_tick;
use crate::controls::PointerButton;
use crate::entity::DroppedItem;
use crate::game::tick::{TickEvents, TICK_DT};
use crate::gui::MenuSlot;
use crate::item::{ItemStack, ItemType};
use crate::mathh::Vec3;

fn test_crafting_recipe(
    key: &str,
    ingredient: ItemType,
    result: ItemType,
) -> crate::crafting::CraftingRecipe {
    crate::crafting::CraftingRecipe::new(
        key.into(),
        crate::crafting::CraftingStation::Inventory,
        vec![crate::crafting::CraftingIngredient {
            selector: crate::crafting::IngredientSelector::Item(ingredient),
            count: 1,
            use_mode: crate::crafting::IngredientUse::Consume,
        }],
        ItemStack::new(result, 2),
    )
}

/// The inventory revision moves on every mutation class the HUD cares about:
/// pickup, menu click, drop, and craft.
#[test]
fn pickup_menu_click_drop_and_craft_each_bump_the_inventory_revision() {
    let mut game = game_on_empty_chunk();
    let rev = |game: &super::common::TestGame| game.server.sessions[0].player.inventory.revision();

    // Pickup: an eligible drop at the body centre is collected in one tick.
    game.server.sessions[0].player.pos = Vec3::new(8.5, 64.0, 8.5);
    let mut drop = DroppedItem::new(
        game.server.sessions[0].player.body_center(),
        ItemStack::new(ItemType::Dirt, 2),
        1,
    );
    drop.ticks_lived = crate::world::ITEM_PICKUP_DELAY_TICKS;
    game.server.world.spawn_item(drop);
    let before = rev(&game);
    assert!(game.server.item_pickup_tick(0), "the drop was collected");
    assert_ne!(rev(&game), before, "a pickup bumps the revision");
    assert_eq!(count_item(game.inventory(), ItemType::Dirt), 2);

    // Menu click: picking the stack up onto the cursor.
    let before = rev(&game);
    game.menu_click(MenuSlot::Inventory(0), PointerButton::Primary, false, false);
    game.apply_latched_actions_for_test();
    assert!(game.inventory().cursor().is_some(), "stack on the cursor");
    assert_ne!(rev(&game), before, "a menu click bumps the revision");
    // Put it back for the drop below.
    game.menu_click(MenuSlot::Inventory(0), PointerButton::Primary, false, false);
    game.apply_latched_actions_for_test();

    // Drop: Q drops one of the selected stack.
    game.server.sessions[0].player.inventory = filled_inventory();
    let before = rev(&game);
    game.drop_selected_item(false);
    game.apply_latched_actions_for_test();
    assert_eq!(count_item(game.inventory(), ItemType::Dirt), 63);
    assert_ne!(rev(&game), before, "a drop bumps the revision");

    // Craft: the explicit stable-key request consumes inventory into a real
    // output, then the ordinary result-slot click takes it.
    game.server.recipes = crate::crafting::Recipes::new(
        vec![test_crafting_recipe(
            "test:revision",
            ItemType::Coal,
            ItemType::Stick,
        )],
        Vec::new(),
        Vec::new(),
    );
    game.server.sessions[0]
        .player
        .inventory
        .add(ItemStack::new(ItemType::Coal, 1));
    game.server
        .open_crafting_for(0, crate::crafting::CraftingStation::Inventory);
    let before = rev(&game);
    game.game.craft_recipe("test:revision", false);
    game.apply_latched_actions_for_test();
    assert_eq!(
        game.menu_read_model().craft_output.map(|s| s.item),
        Some(ItemType::Stick),
        "the accepted request replicated a real output"
    );
    assert_ne!(rev(&game), before, "crafting consumes inventory");

    let before = rev(&game);
    game.menu_click(MenuSlot::CraftResult, PointerButton::Primary, false, false);
    game.apply_latched_actions_for_test();
    assert_eq!(
        game.inventory().cursor().map(|s| s.item),
        Some(ItemType::Stick),
        "the craft result landed on the cursor"
    );
    assert_ne!(
        rev(&game),
        before,
        "taking a craft result bumps the revision"
    );
}

#[test]
fn crafting_outputs_replicate_per_session_and_remain_independent() {
    use crate::crafting::CraftingStation;
    use crate::net::protocol::{ClientToServer, MenuSlotWire, MenuTargetWire};

    let mut game = game();
    game.server.recipes = crate::crafting::Recipes::new(
        vec![
            test_crafting_recipe("test:local", ItemType::Coal, ItemType::Stick),
            test_crafting_recipe("test:remote", ItemType::Dirt, ItemType::Glass),
        ],
        Vec::new(),
        Vec::new(),
    );
    let remote = game
        .server
        .add_session_for_test(crate::player::Player::new(Vec3::new(2.5, 64.0, 2.5)));
    game.server.sessions[0]
        .player
        .inventory
        .add(ItemStack::new(ItemType::Coal, 1));
    game.server.sessions[remote]
        .player
        .inventory
        .add(ItemStack::new(ItemType::Dirt, 1));
    game.server.open_crafting_for(0, CraftingStation::Inventory);
    game.server
        .open_crafting_for(remote, CraftingStation::Inventory);

    game.server.apply_message(
        0,
        ClientToServer::CraftRecipe {
            recipe: "test:local".into(),
            bulk: false,
            request_id: 41,
        },
    );
    game.server.apply_message(
        remote,
        ClientToServer::CraftRecipe {
            recipe: "test:remote".into(),
            bulk: false,
            request_id: 42,
        },
    );
    let mut events = TickEvents::default();
    game.server.tick_menu(0, &mut events);
    game.server.tick_menu(remote, &mut events);

    let local_sync = game
        .server
        .build_menu_sync(0)
        .expect("local output changed the session view");
    let remote_sync = game
        .server
        .build_menu_sync(remote)
        .expect("remote output changed the session view");
    assert!(matches!(
        local_sync.target,
        MenuTargetWire::Inventory { output: Some(slot) }
            if slot.item_id == ItemType::Stick.0
    ));
    assert!(matches!(
        remote_sync.target,
        MenuTargetWire::Inventory { output: Some(slot) }
            if slot.item_id == ItemType::Glass.0
    ));

    game.server.apply_message(
        0,
        ClientToServer::MenuClick {
            slot: MenuSlotWire::from_menu_slot(&MenuSlot::CraftResult),
            button: 0,
            shift: false,
            gather: false,
            request_id: 43,
        },
    );
    game.server.tick_menu(0, &mut events);
    assert!(game.server.sessions[0].menu.craft_output().is_none());
    assert_eq!(
        game.server.sessions[remote]
            .menu
            .craft_output()
            .map(|s| s.item),
        Some(ItemType::Glass),
        "taking one session's output cannot mutate another session"
    );
}

/// The full inventory rides a `SelfState` only when the revision moved —
/// always on the first update after join, then only after a change.
#[test]
fn self_state_ships_the_inventory_only_when_the_revision_moved() {
    let mut game = game_on_empty_chunk();

    let up1 = pump_one_tick(&mut game);
    let s1 = up1.self_state.as_ref().expect("self state every batch");
    assert!(
        s1.inventory.is_some(),
        "the first update after join always carries the inventory"
    );
    assert_eq!(
        s1.inventory.as_ref().map(|v| v.len()),
        Some(crate::inventory::TOTAL_SLOTS + 1),
        "36 slots + the cursor"
    );

    let up2 = pump_one_tick(&mut game);
    let s2 = up2.self_state.as_ref().expect("self state every batch");
    assert!(
        s2.inventory.is_none(),
        "an unchanged revision ships no inventory body"
    );

    game.server.sessions[0]
        .player
        .inventory
        .add(ItemStack::new(ItemType::Stone, 5));
    let up3 = pump_one_tick(&mut game);
    let s3 = up3.self_state.as_ref().expect("self state every batch");
    let slots = s3
        .inventory
        .as_ref()
        .expect("a mutation re-ships the inventory");
    assert_eq!(
        slots[0].map(|w| (w.item_id, w.count)),
        Some((ItemType::Stone.0, 5))
    );
}

// ---- Session-boundary contracts: events + menu sync on the wire ----

/// Chest viewer-count transitions emit `ChestOpened`/`ChestClosed` world
/// events ONLY at the 0↔1 boundaries — a second overlapping viewer opens and
/// closes silently.
#[test]
fn chest_viewer_transitions_emit_events_only_at_zero_boundaries() {
    use crate::block::Block;
    use crate::mathh::IVec3;

    let mut game = super::common::game_on_empty_chunk();
    let pos = IVec3::new(3, 64, 3);
    game.server.world.set_block_world(3, 64, 3, Block::Chest);
    game.server
        .world
        .insert_chest(pos, crate::block_model::DEFAULT_MODEL_FACING);
    let s1 = game
        .server
        .add_session_for_test(crate::player::Player::new(Vec3::new(2.5, 64.0, 2.5)));

    let mut ev = TickEvents::default();
    game.server.open_chest_screen_for(0, pos, &mut ev);
    assert_eq!(ev.world.chest_changed, vec![(pos, true)], "0→1 opens");
    game.server.open_chest_screen_for(s1, pos, &mut ev);
    assert_eq!(ev.world.chest_changed.len(), 1, "1→2 emits nothing");
    game.server.close_open_menu_for(0, &mut ev);
    assert_eq!(ev.world.chest_changed.len(), 1, "2→1 emits nothing");
    game.server.close_open_menu_for(s1, &mut ev);
    assert_eq!(
        ev.world.chest_changed,
        vec![(pos, true), (pos, false)],
        "1→0 closes, exactly once"
    );
}

/// A SECOND session's chest interaction (the tick-side open) reaches session
/// 0's replication batch: `open_chests` gains the chest and exactly one
/// `ChestOpened` event rides `events` — while session 0's own `self_events`
/// carries no open-screen (it wasn't the opener).
#[test]
fn a_remote_sessions_chest_open_reaches_the_local_batch_exactly_once() {
    use crate::block::Block;
    use crate::mathh::IVec3;
    use crate::net::protocol::WorldEventMsg;

    let mut game = super::common::game_on_empty_chunk();
    let pos = IVec3::new(3, 64, 3);
    game.server.world.set_block_world(3, 64, 3, Block::Chest);
    game.server
        .world
        .insert_chest(pos, crate::block_model::DEFAULT_MODEL_FACING);
    let s1 = game
        .server
        .add_session_for_test(crate::player::Player::new(Vec3::new(2.5, 64.0, 2.5)));

    // Session 1 right-clicked the chest (latched edge + look, as its
    // PlayerUpdate/UseClick messages would leave them).
    game.server.sessions[s1].look = Some(super::common::hit(pos, IVec3::Y));
    game.server.queue_place_click_for_test(s1);

    let update = pump_one_tick(&mut game);
    assert!(
        update.open_chests.contains(&pos),
        "the other player's open lifts the replicated lid set"
    );
    let opened: Vec<_> = update
        .events
        .iter()
        .filter(|e| matches!(e, WorldEventMsg::ChestOpened { pos: p } if *p == pos))
        .collect();
    assert_eq!(opened.len(), 1, "exactly one ChestOpened event broadcast");
    assert_eq!(
        update.self_events.open_screen, None,
        "the non-opening recipient gets no open-screen one-shot"
    );
}

/// `menu_sync` rides a batch only when the menu view CHANGED: the first batch
/// ships the initial (closed) view, an unchanged menu ships `None`, and a
/// tick-side open ships the new target once.
#[test]
fn menu_sync_ships_on_change_only() {
    use crate::block::Block;
    use crate::mathh::IVec3;
    use crate::net::protocol::MenuTargetWire;

    let mut game = super::common::game_on_empty_chunk();
    let pos = IVec3::new(3, 64, 3);
    game.server.world.set_block_world(3, 64, 3, Block::Chest);
    game.server
        .world
        .insert_chest(pos, crate::block_model::DEFAULT_MODEL_FACING);

    let up1 = pump_one_tick(&mut game);
    let sync = up1
        .menu_sync
        .expect("the first batch ships the initial view");
    assert_eq!(sync.target, MenuTargetWire::None);
    let up2 = pump_one_tick(&mut game);
    assert!(
        up2.menu_sync.is_none(),
        "unchanged (closed) menu ships nothing"
    );

    let mut ev = TickEvents::default();
    game.server.open_chest_screen_for(0, pos, &mut ev);
    let up3 = pump_one_tick(&mut game);
    let sync = up3.menu_sync.expect("the open ships the new view");
    assert!(
        matches!(sync.target, MenuTargetWire::Chest { pos: p, .. } if p == pos),
        "the chest target replicates, got {:?}",
        sync.target
    );
    let up4 = pump_one_tick(&mut game);
    assert!(
        up4.menu_sync.is_none(),
        "a still-open, untouched chest ships nothing"
    );
}

/// The mod-GUI state map rides `menu_sync` only when its `Arc` changed: once
/// at open (the cleared map), once per tick-side write (copy-on-write forces
/// a fresh allocation), never in between.
#[test]
fn gui_state_ships_in_menu_sync_only_on_arc_change() {
    use crate::gui::GuiValue;
    use crate::net::protocol::{GuiValueWire, MenuTargetWire};

    let mut game = super::common::game();
    game.set_mods_for_test(crate::modding::ModHost::test_unit_guest_host("modtest"));
    let kind = crate::gui::intern_kind("modtest:panel").expect("mod kind registers");
    game.server.open_mod_gui_screen_for(0, kind, None);

    let up = pump_one_tick(&mut game);
    let MenuTargetWire::ModGui { gui_state, .. } = up.menu_sync.expect("the open ships").target
    else {
        panic!("expected a ModGui target");
    };
    assert_eq!(
        gui_state,
        Some(Vec::new()),
        "a fresh session ships its cleared (empty) map"
    );

    let up = pump_one_tick(&mut game);
    assert!(up.menu_sync.is_none(), "no writes → no sync");

    // What a mod's GuiStateSet HostCall does on the tick: a copy-on-write
    // write against the session's map.
    crate::gui::gui_state_set(
        &mut game.server.sessions[0].gui_state,
        "modtest:v".into(),
        GuiValue::I32(7),
    );
    let up = pump_one_tick(&mut game);
    let MenuTargetWire::ModGui { gui_state, .. } =
        up.menu_sync.expect("the write ships a sync").target
    else {
        panic!("expected a ModGui target");
    };
    assert_eq!(
        gui_state,
        Some(vec![("modtest:v".into(), GuiValueWire::I32(7))]),
        "the changed map rides whole"
    );

    let up = pump_one_tick(&mut game);
    assert!(up.menu_sync.is_none(), "same Arc → nothing ships");
}

#[test]
fn host_written_mod_gui_state_syncs_to_matching_remote_session() {
    use crate::gui::GuiValue;
    use crate::net::protocol::{GuiValueWire, MenuTargetWire};

    let mut game = super::common::game();
    game.set_mods_for_test(crate::modding::ModHost::test_unit_guest_host("kitchen"));
    let remote = game
        .server
        .add_session_for_test(crate::player::Player::new(Vec3::new(2.5, 64.0, 2.5)));
    let kind = crate::gui::intern_kind("kitchen:oven").expect("mod kind registers");
    let pos = crate::mathh::IVec3::new(4, 64, 4);

    game.server.open_mod_gui_screen_for(remote, kind, Some(pos));
    crate::gui::gui_state_set(
        &mut game.server.sessions[0].gui_state,
        "kitchen:cook01".into(),
        GuiValue::F32(0.5),
    );

    let MenuTargetWire::ModGui { gui_state, .. } = game
        .server
        .build_menu_sync(remote)
        .expect("remote menu sync includes the shared mod GUI state")
        .target
    else {
        panic!("expected a ModGui target");
    };
    assert_eq!(
        gui_state,
        Some(vec![("kitchen:cook01".into(), GuiValueWire::F32(0.5))]),
        "a single-instance mod machine's gauge publish reaches the remote opener"
    );
}

/// The screen-open request queued at the tick's interaction site arrives as
/// `SelfEvents.open_screen` and `Game::tick` maps it onto the app-facing
/// `GameEvents` field unchanged-consumer-side.
#[test]
fn open_screen_one_shot_maps_back_onto_game_events() {
    use crate::game::GameInput;
    use crate::mathh::IVec3;

    let mut game = super::common::game_on_empty_chunk();
    let pos = IVec3::new(3, 64, 3);
    // The tick's request site (interaction arm) writes this outbox field;
    // seed it directly to isolate the SelfEvents → GameEvents pipe.
    game.server.sessions[0].request_open_gui = Some((crate::gui::GuiKind::Chest, Some(pos)));

    let events = game.tick(TICK_DT, &GameInput::default());
    assert_eq!(
        events.open_gui,
        Some((crate::gui::GuiKind::Chest, Some(pos))),
        "the one-shot rode SelfEvents.open_screen into GameEvents"
    );
    assert!(
        game.server.sessions[0].request_open_gui.is_none(),
        "the request outbox is consumed by the batch"
    );

    let events = game.tick(TICK_DT, &GameInput::default());
    assert_eq!(events.open_gui, None, "one-shots don't repeat");
}
