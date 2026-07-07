//! Full-loop proof for the kitchen mod AND the mod container-slot API it
//! rides on: a multi-cell mod model block placed through the real placement
//! path, a slot-bearing mod GUI session (container created at the canonical
//! anchor), accepts-filtered shift-routing, wasm-driven cooking through
//! `container_get`/`container_set`/`item_info`/`recipe_result`, gauge publish
//! to the GUI state map, the take-only output, and break-scatter.
//!
//! The fixture stages the kitchen pack PLUS a synthetic content-only
//! `testfood` pack shipping a `kitchen:cookable`-tagged item and a
//! `kitchen:cooking` processing recipe — the composition the recipe-class
//! design exists for: food packs extend the oven with data only. The oven
//! must cook that food and must NOT smelt ore (its class is `kitchen:cooking`,
//! never the furnace's `llama:smelting` table). Pack registration needs the
//! fixture in the registry, so the assertions run in a child process (the
//! established `LLAMACRAFT_MODS` re-spawn pattern).

use super::super::tick::TickEvents;
use super::super::Game;
use crate::camera::Camera;
use crate::mathh::Vec3;

#[test]
fn kitchen_oven_cooks_food_not_ore_through_the_container_api_via_wasm() {
    let Some(root) = crate::modding::tests::stage_mods_fixture("kitchen-oven", &["kitchen"]) else {
        return;
    };
    // A synthetic food pack beside the kitchen: one cookable item pair + the
    // cooking recipe, all data (no wasm) — proving cross-pack extension.
    let food = root.join("mods").join("testfood");
    std::fs::create_dir_all(&food).unwrap();
    std::fs::write(
        food.join("pack.json"),
        r#"{ "name": "Test Food", "id": "testfood", "version": "0.1.0" }"#,
    )
    .unwrap();
    std::fs::write(
        food.join("items.json"),
        r#"{ "items": [
            { "item": "testfood:raw_chop", "key": "testfood:raw_chop", "name": "Raw Chop",
              "max_stack_size": 64, "held_pose": { "pitch": 0, "yaw": 0, "roll": 0 },
              "sprite": "stone", "tags": ["kitchen:cookable"] },
            { "item": "testfood:cooked_chop", "key": "testfood:cooked_chop", "name": "Cooked Chop",
              "max_stack_size": 64, "held_pose": { "pitch": 0, "yaw": 0, "roll": 0 },
              "sprite": "stone", "tags": [] }
        ] }"#,
    )
    .unwrap();
    std::fs::write(
        food.join("recipes.json"),
        r#"{ "recipes": [
            { "type": "processing", "class": "kitchen:cooking",
              "ingredient": "testfood:raw_chop", "result": "testfood:cooked_chop" }
        ] }"#,
    )
    .unwrap();
    crate::modding::tests::run_child_test(&root, "game::tests::kitchen_mod::kitchen_oven_inner");
}

/// Runs ONLY in the child process spawned above (needs `LLAMACRAFT_MODS`
/// pointing at the fixture packs before first registry touch).
#[test]
#[ignore = "spawned by kitchen_oven_cooks_food_not_ore_through_the_container_api_via_wasm with a fixture pack env"]
fn kitchen_oven_inner() {
    use crate::block::Block;
    use crate::chunk::{Chunk, ChunkPos, CHUNK_SX, CHUNK_SZ};
    use crate::controls::PointerButton;
    use crate::gui::{GuiValue, MenuSlot};
    use crate::item::{ItemStack, ItemType};
    use crate::mathh::IVec3;

    let by_key = |key: &str| {
        ItemType::all()
            .iter()
            .copied()
            .find(|i| i.key() == key)
            .unwrap_or_else(|| panic!("{key} registered from the fixture packs"))
    };
    let oven_item = by_key("kitchen:oven");
    let oven_block = oven_item.as_block().expect("oven item links to its block");
    let raw_chop = by_key("testfood:raw_chop");
    let cooked_chop = by_key("testfood:cooked_chop");
    let raw_iron = ItemType::RawIron;

    let mut game = Game::new(Camera::new(Vec3::new(8.0, 66.0, 8.0), 16.0 / 9.0), "", 1, 1);
    assert_eq!(game.mods_for_test().loaded(), 1, "the kitchen wasm loaded");
    game.world.clear_world();
    let cp = ChunkPos::new(0, 0);
    game.world.insert_empty_column_for_test(cp);
    let mut chunk = Chunk::new(0, 0);
    for z in 0..CHUNK_SZ {
        for x in 0..CHUNK_SX {
            chunk.set_block(x, 63, z, Block::Stone);
        }
    }
    game.world.insert_chunk_for_test(cp, chunk);
    game.player.pos = Vec3::new(4.0, 64.0, 4.0);
    game.player.vel = Vec3::ZERO;
    game.player.on_ground = true;

    // Hotbar: the oven to place, food to cook, ore as the must-NOT-route (and
    // must-not-cook) control, coal to burn, stone for the cursor tests.
    game.player.inventory.add(ItemStack::new(oven_item, 1));
    game.player.inventory.add(ItemStack::new(raw_chop, 1));
    game.player.inventory.add(ItemStack::new(raw_iron, 3));
    // Two coal: the relight consumes one the same tick the fuel routes in
    // (Menu stage click, then the mod's WorldScheduled cook step).
    game.player.inventory.add(ItemStack::new(ItemType::Coal, 2));
    game.player.inventory.add(ItemStack::new(ItemType::Stone, 7));

    // Place through the REAL placement path (multi-cell footprint, facing,
    // `block_placed` anchor announcement — the cell the mod records).
    let floor = IVec3::new(10, 63, 8);
    game.look = Some(super::common::hit(floor, IVec3::Y));
    game.pending_place = true;
    let mut ev = TickEvents::default();
    game.game_tick_step(&mut ev);
    assert!(
        ev.placed_block.is_some(),
        "the oven placed from the held item"
    );
    let clicked = floor + IVec3::Y;
    let (_, anchor, cells) = game
        .world
        .model_group(clicked)
        .expect("the placed oven resolves as a model group from any cell");
    assert_eq!(
        cells.len(),
        2 * 3 * 2,
        "the converted bbmodel keeps its declared 2x3x2 footprint"
    );

    // Open the GUI from a NON-anchor footprint cell: the session must
    // canonicalize to the anchor and create the 3-slot container there.
    let far_cell = *cells
        .iter()
        .find(|c| **c != anchor)
        .expect("a 12-cell footprint has non-anchor cells");
    let kind = crate::gui::resolve_kind("kitchen:oven")
        .expect("the pack's open_gui interaction registered the kind");
    game.open_mod_gui_screen(kind, Some(far_cell));
    let slots = |game: &Game| {
        game.world
            .container_at(anchor)
            .expect("the session created the container at the ANCHOR cell")
            .slots
            .clone()
    };
    assert_eq!(slots(&game).len(), 3, "sized by the document's slot count");

    // Shift-clicks route by the document's accepts filters: the cookable food
    // to the input, coal to the fuel slot — and raw IRON to NEITHER (the oven
    // is not a furnace; it falls back to the ordinary inventory shuffle).
    let inv_slot_of = |game: &Game, item: ItemType| -> usize {
        (0..crate::inventory::TOTAL_SLOTS)
            .find(|&i| game.player.inventory.slot(i).is_some_and(|s| s.item == item))
            .expect("item somewhere in the inventory")
    };
    for item in [raw_chop, ItemType::Coal, raw_iron] {
        let i = inv_slot_of(&game, item);
        game.menu_click(MenuSlot::Inventory(i), PointerButton::Primary, true, false);
        game.game_tick_step(&mut ev);
    }
    let s = slots(&game);
    assert_eq!(s[0].map(|s| s.item), Some(raw_chop), "input got the food");
    assert_eq!(
        s[1],
        Some(ItemStack::new(ItemType::Coal, 1)),
        "fuel got the coal, minus the one the same-tick relight consumed"
    );
    assert_eq!(s[2], None, "nothing routed into the take-only output");
    assert!(
        !slots(&game).iter().flatten().any(|s| s.item == raw_iron),
        "raw iron routed NOWHERE into the oven (it is not cookable)"
    );
    assert_eq!(
        game.player
            .inventory
            .slot(crate::inventory::HOTBAR_LEN)
            .map(|s| s.item),
        Some(raw_iron),
        "raw iron fell back to the ordinary hotbar-to-grid shuffle"
    );

    // Cook: the wasm consumes the `kitchen:cooking` class (620 ticks covers
    // the 600-tick cook), publishing the gauges while the session is open.
    for _ in 0..620 {
        game.game_tick_step(&mut ev);
    }
    let s = slots(&game);
    assert_eq!(s[0], None, "the food was consumed");
    assert_eq!(
        s[2],
        Some(ItemStack::new(cooked_chop, 1)),
        "the output holds the cooked food"
    );
    match game.world.gui_state_get("kitchen:burn01") {
        Some(GuiValue::F32(v)) => assert!(*v > 0.0, "coal keeps burning mid-session, got {v}"),
        other => panic!("kitchen:burn01 should be live while open, got {other:?}"),
    }
    assert!(
        game.world
            .cell_kv_get(anchor.x, anchor.y, anchor.z, "kitchen:state")
            .is_some(),
        "burn/cook state persists in the anchor's section cell KV"
    );

    // The output is take-only: a primary click with a stack on the cursor
    // must not deposit it. Then shift-click the food out into the inventory.
    let stone_slot = inv_slot_of(&game, ItemType::Stone);
    game.menu_click(
        MenuSlot::Inventory(stone_slot),
        PointerButton::Primary,
        false,
        false,
    );
    game.game_tick_step(&mut ev); // stone now on the cursor
    game.menu_click(MenuSlot::Container(2), PointerButton::Primary, false, false);
    game.game_tick_step(&mut ev);
    assert_eq!(
        slots(&game)[2],
        Some(ItemStack::new(cooked_chop, 1)),
        "a held stack cannot be deposited into the take-only output"
    );
    game.menu_click(
        MenuSlot::Inventory(stone_slot),
        PointerButton::Primary,
        false,
        false,
    );
    game.game_tick_step(&mut ev); // stone back in the inventory
    game.menu_click(MenuSlot::Container(2), PointerButton::Primary, true, false);
    game.game_tick_step(&mut ev);
    assert!(
        (0..crate::inventory::TOTAL_SLOTS)
            .filter_map(|i| game.player.inventory.slot(i))
            .any(|s| s.item == cooked_chop),
        "the cooked food shift-clicked into the inventory"
    );

    // THE BUG THIS TEST PINS: ore in the oven's input must never cook. The
    // burner is still lit (plenty of coal burn left), so a smelting-table
    // leak would produce an ingot within 600 ticks + margin.
    if let Some(c) = game.world.container_at_mut(anchor) {
        c.slots[0] = Some(ItemStack::new(raw_iron, 3));
    }
    for _ in 0..650 {
        game.game_tick_step(&mut ev);
    }
    let s = slots(&game);
    assert_eq!(
        s[0],
        Some(ItemStack::new(raw_iron, 3)),
        "ore sits in the oven untouched — the oven consumes kitchen:cooking, not llama:smelting"
    );
    assert_eq!(s[2], None, "no smelted product appeared");
    game.close_open_menu();

    // Break from a NON-anchor cell: the whole footprint clears and the
    // container's remaining contents (the untouched ore) scatter.
    let before = game.world.item_entities().len();
    game.finish_player_break(
        crate::mining::BreakEvent {
            pos: far_cell,
            block: oven_block,
            harvested: true,
        },
        &mut ev,
    );
    assert!(
        game.world.container_at(anchor).is_none(),
        "breaking any cell removes the anchored container"
    );
    assert!(
        cells
            .iter()
            .all(|c| game.world.chunk_block(c.x, c.y, c.z) == Block::Air.id()),
        "the whole footprint cleared"
    );
    let scattered: Vec<ItemType> = game.world.item_entities()[before..]
        .iter()
        .map(|e| e.stack.item)
        .collect();
    assert!(
        scattered.contains(&raw_iron),
        "container contents scattered on break, got {scattered:?}"
    );
    assert!(
        scattered.contains(&oven_item),
        "the oven item itself dropped, got {scattered:?}"
    );

    // The mod prunes the broken oven from its tracked list on the next tick.
    game.game_tick_step(&mut ev);
    assert_eq!(
        game.world.mod_kv_get("kitchen:ovens").map(<[u8]>::len),
        Some(0),
        "the wasm-side oven list pruned the broken anchor"
    );
    let (disabled, _, _) = game.mods_for_test().probe(0);
    assert!(!disabled, "the kitchen mod stayed healthy throughout");
}
