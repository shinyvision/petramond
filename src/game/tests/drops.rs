use super::super::tick::TICK_DT;
use super::super::{Game, GameInput};
use super::common::{apply_drop_actions, count_item, filled_inventory, game};
use crate::block::Block;
use crate::entity::DroppedItem;
use crate::inventory::Inventory;
use crate::item::{ItemStack, ItemType};
use crate::mathh::{IVec3, Vec3};
use crate::world::{ITEM_LIFETIME_TICKS, ITEM_PICKUP_DELAY_TICKS};

#[test]
fn spawn_drops_dirt_yields_one_drop() {
    let mut game = game();
    assert!(game.world.item_entities().is_empty());
    game.spawn_drops(IVec3::new(2, 3, 4), Block::Dirt, 17);
    assert_eq!(game.world.item_entities().len(), 1);
    let d = &game.world.item_entities()[0];
    assert_eq!(d.stack.item, crate::item::ItemType::Dirt);
    assert_eq!(d.stack.count, 1);
    assert_eq!(d.skylight, 17);
    assert!((d.pos.x - 2.5).abs() < 1e-5);
    assert!((d.pos.y - 3.5).abs() < 1e-5);
    assert!((d.pos.z - 4.5).abs() < 1e-5);
}

#[test]
fn dropped_item_is_picked_up_near_player() {
    let mut game = game();
    let item = crate::item::ItemType::Poppy;
    let before = count_item(&game.player.inventory, item);
    let centre = game.player.body_center();
    let mut drop = DroppedItem::new(centre, ItemStack::new(item, 1), 1);
    drop.ticks_lived = ITEM_PICKUP_DELAY_TICKS; // past the pickup delay
    game.world.spawn_item(drop);
    game.item_pickup_tick();
    let after = count_item(&game.player.inventory, item);
    assert_eq!(after, before + 1);
    assert!(game.world.item_entities().is_empty());
}

#[test]
fn partial_pickup_takes_what_fits_and_leaves_the_rest() {
    let mut game = game();
    // Room for exactly one more dirt: 63 dirt in one slot, every other slot
    // full of a different item.
    let mut inv = Inventory::new();
    inv.add(ItemStack::new(ItemType::Dirt, 63));
    for _ in 0..(crate::inventory::TOTAL_SLOTS - 1) {
        inv.add(ItemStack::new(ItemType::Stone, 64));
    }
    game.player.inventory = inv;

    let centre = game.player.body_center();
    let mut drop = DroppedItem::new(centre, ItemStack::new(ItemType::Dirt, 5), 1);
    drop.ticks_lived = ITEM_PICKUP_DELAY_TICKS;
    game.world.spawn_item(drop);

    // One tick plans the partial pickup and absorbs the requested split because
    // the stack is already inside the pickup radius.
    game.item_pickup_tick();

    assert_eq!(
        count_item(&game.player.inventory, ItemType::Dirt),
        64,
        "took exactly the one dirt that fit"
    );
    let loose: u32 = game
        .world
        .item_entities()
        .iter()
        .filter(|d| d.stack.item == ItemType::Dirt)
        .map(|d| d.stack.count as u32)
        .sum();
    assert_eq!(
        loose, 4,
        "the four that didn't fit stay in the world, not discarded"
    );
}

#[test]
fn pickup_planning_reserves_capacity_before_magnetizing() {
    let mut game = game();
    // Room for exactly one dirt, but two dirt drops are inside the attract
    // radius. Planning should request only one of them.
    let mut inv = Inventory::new();
    inv.add(ItemStack::new(ItemType::Dirt, 63));
    for _ in 0..(crate::inventory::TOTAL_SLOTS - 1) {
        inv.add(ItemStack::new(ItemType::Stone, 64));
    }
    game.player.inventory = inv;

    let chest = game.player.body_center();
    for (seed, offset) in [
        (1, Vec3::new(crate::entity::ATTRACT_RADIUS - 0.1, 0.0, 0.0)),
        (2, Vec3::new(0.0, 0.0, crate::entity::ATTRACT_RADIUS - 0.1)),
    ] {
        let mut drop = DroppedItem::new(chest + offset, ItemStack::new(ItemType::Dirt, 1), seed);
        drop.vel = Vec3::ZERO;
        drop.ticks_lived = ITEM_PICKUP_DELAY_TICKS;
        game.world.spawn_item(drop);
    }

    game.item_pickup_tick();

    assert_eq!(count_item(&game.player.inventory, ItemType::Dirt), 63);
    let requested: u32 = game
        .world
        .item_entities()
        .iter()
        .filter(|d| d.pickup_requested)
        .map(|d| d.stack.count as u32)
        .sum();
    assert_eq!(requested, 1, "only the item that fits is requested");
}

#[test]
fn fresh_dropped_item_waits_out_pickup_delay() {
    let mut game = game();
    let item = crate::item::ItemType::Poppy;
    let centre = game.player.body_center();
    // ticks_lived 0: sitting right on the player but still inside the delay.
    game.world
        .spawn_item(DroppedItem::new(centre, ItemStack::new(item, 1), 1));
    game.item_pickup_tick();
    assert_eq!(
        game.world.item_entities().len(),
        1,
        "delay blocks immediate pickup"
    );
    // Each tick ages it by one; once past the delay it is collected.
    for _ in 0..ITEM_PICKUP_DELAY_TICKS {
        game.item_pickup_tick();
    }
    assert!(game.world.item_entities().is_empty());
}

#[test]
fn dropped_item_magnets_toward_player_then_absorbs() {
    let mut game = game();
    let item = crate::item::ItemType::Poppy;
    let before = count_item(&game.player.inventory, item);
    let chest = game.player.body_center();
    let start = chest + Vec3::new(0.0, crate::entity::ATTRACT_RADIUS - 0.1, 0.0);
    let mut drop = DroppedItem::new(start, ItemStack::new(item, 1), 1);
    drop.ticks_lived = ITEM_PICKUP_DELAY_TICKS; // skip the delay so the magnet engages now
    game.world.spawn_item(drop);
    let d0 = (game.world.item_entities()[0].pos - chest).length();
    game.item_pickup_tick();
    assert!(game.world.item_entities()[0].pickup_requested);
    let pp = game.player.body_center();
    game.world.tick_item_physics(TICK_DT, pp);
    if !game.world.item_entities().is_empty() {
        let d1 = (game.world.item_entities()[0].pos - chest).length();
        assert!(d1 < d0);
    }
    // Item physics + pickup both run on the fixed tick now: the magnet flies it in,
    // and the pickup absorbs it once it's in range.
    for _ in 0..60 {
        if game.world.item_entities().is_empty() {
            break;
        }
        game.item_pickup_tick();
        let pp = game.player.body_center();
        game.world.tick_item_physics(TICK_DT, pp);
    }
    assert!(game.world.item_entities().is_empty());
    assert_eq!(count_item(&game.player.inventory, item), before + 1);
}

#[test]
fn a_dropped_item_enters_the_world_on_the_tick_not_the_frame() {
    let mut game = game();
    game.player.inventory = filled_inventory(); // a stack of Dirt
    game.player.inventory.set_active(0);
    let before = count_item(&game.player.inventory, ItemType::Dirt);

    // Q-drop queues intent only; inventory and world stay unchanged until the tick.
    game.drop_selected_item(false);
    assert_eq!(
        count_item(&game.player.inventory, ItemType::Dirt),
        before,
        "inventory mutation waits for the tick"
    );
    assert!(
        game.world.item_entities().is_empty(),
        "the drop hasn't entered the world until a tick runs"
    );

    // The tick removes the item and materialises the drop as a world entity.
    let events = apply_drop_actions(&mut game);
    assert!(events.threw_item);
    assert_eq!(
        count_item(&game.player.inventory, ItemType::Dirt),
        before - 1
    );
    assert_eq!(
        game.world.item_entities().len(),
        1,
        "the drop spawns on the tick"
    );
}

#[test]
fn dropped_item_beyond_one_block_is_not_magnet_picked_up() {
    let mut game = game();
    let item = crate::item::ItemType::Poppy;
    let before = count_item(&game.player.inventory, item);
    let chest = game.player.body_center();
    let start = chest + Vec3::new(crate::entity::ATTRACT_RADIUS + 0.05, 0.0, 0.0);
    let mut drop = DroppedItem::new(start, ItemStack::new(item, 1), 1);
    drop.vel = Vec3::ZERO;
    drop.ticks_lived = ITEM_PICKUP_DELAY_TICKS; // eligible for pickup, so only range gates it
    game.world.spawn_item(drop);

    for _ in 0..60 {
        let pp = game.player.body_center();
        game.world.tick_item_physics(TICK_DT, pp);
        game.item_pickup_tick();
    }

    assert_eq!(game.world.item_entities().len(), 1);
    assert_eq!(count_item(&game.player.inventory, item), before);
}

#[test]
fn distant_dropped_item_is_not_picked_up() {
    let mut game = game();
    let far = game.player.eye() + Vec3::new(50.0, 0.0, 0.0);
    let mut drop = DroppedItem::new(far, ItemStack::new(crate::item::ItemType::Dirt, 1), 2);
    drop.ticks_lived = ITEM_PICKUP_DELAY_TICKS; // eligible, but far out of range
    game.world.spawn_item(drop);
    game.item_pickup_tick();
    assert_eq!(game.world.item_entities().len(), 1);
}

#[test]
fn stationary_dropped_item_resamples_after_chunk_light_bake_installs() {
    let mut game = game();
    game.world.clear_world();

    let pos = crate::chunk::ChunkPos::new(0, 0);
    game.world
        .insert_chunk_for_test(pos, crate::chunk::Chunk::new(0, 0));
    game.dropped_light_revision = game.world.lighting_revision();

    let mut drop = DroppedItem::new(
        Vec3::new(1.5, 5.5, 1.5),
        ItemStack::new(crate::item::ItemType::Dirt, 1),
        4,
    );
    drop.vel = Vec3::ZERO;
    drop.skylight = 0;
    game.world.spawn_item(drop);

    let before = game.world.lighting_revision();
    for _ in 0..200 {
        game.world.tick_mesh_budget(1);
        game.refresh_dropped_item_lights_after_world_light_update();
        if game.world.lighting_revision() != before {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }

    assert_ne!(game.world.lighting_revision(), before);
    assert_eq!(game.world.item_entities()[0].skylight, 63);
}

#[test]
fn stale_dropped_item_despawns_on_the_lifetime_tick() {
    let mut game = game();
    let far = game.player.eye() + Vec3::new(50.0, 0.0, 0.0);
    let mut item = DroppedItem::new(far, ItemStack::new(crate::item::ItemType::Dirt, 1), 3);
    // One tick short of the lifetime limit: the next fixed tick ages it out.
    item.ticks_lived = ITEM_LIFETIME_TICKS - 1;
    game.world.spawn_item(item);
    game.item_pickup_tick();
    assert!(game.world.item_entities().is_empty());
}

#[test]
fn throwing_cursor_stack_spawns_a_dropped_item() {
    let mut game = game();
    game.player.inventory = filled_inventory();
    // Drag a stack onto the cursor first.
    game.player.inventory.click_slot(0);
    let held = game
        .player
        .inventory
        .cursor()
        .expect("cursor holds a stack")
        .count;
    assert!(game.world.item_entities().is_empty());
    game.throw_cursor_stack();
    assert!(
        game.player.inventory.cursor().is_some(),
        "cursor is not mutated until the tick"
    );
    let events = apply_drop_actions(&mut game);
    assert!(events.threw_item);
    assert!(game.player.inventory.cursor().is_none(), "cursor emptied");
    assert_eq!(game.world.item_entities().len(), 1);
    assert_eq!(game.world.item_entities()[0].stack.count, held);
    assert_eq!(
        game.world.item_entities()[0].ticks_lived,
        0,
        "thrown item starts the pickup delay"
    );
}

#[test]
fn queued_cursor_stack_throw_survives_menu_close_before_tick() {
    let mut game = game();
    game.player.inventory = Inventory::from_parts(
        [None; crate::inventory::TOTAL_SLOTS],
        Some(ItemStack::new(ItemType::Dirt, 12)),
        0,
    );

    game.throw_cursor_stack();
    assert!(
        game.player.inventory.cursor().is_some(),
        "throwing does not mutate the cursor until another tick/close action"
    );
    game.close_cursor_stack();

    assert!(
        game.player.inventory.cursor().is_none(),
        "the committed cursor throw is not stashed on close"
    );
    assert_eq!(count_item(&game.player.inventory, ItemType::Dirt), 0);
    assert!(
        game.world.item_entities().is_empty(),
        "entity spawn still waits for the tick"
    );

    let events = apply_drop_actions(&mut game);
    assert!(events.threw_item);
    assert_eq!(game.world.item_entities().len(), 1);
    assert_eq!(
        game.world.item_entities()[0].stack,
        ItemStack::new(ItemType::Dirt, 12)
    );
}

#[test]
fn throwing_one_from_cursor_drops_a_single_item() {
    let mut game = game();
    game.player.inventory = filled_inventory();
    game.player.inventory.click_slot(0);
    let held = game.player.inventory.cursor().unwrap().count;
    game.throw_cursor_one();
    assert_eq!(
        game.player.inventory.cursor().unwrap().count,
        held,
        "cursor count is unchanged until the tick"
    );
    let events = apply_drop_actions(&mut game);
    assert!(events.threw_item);
    assert_eq!(game.world.item_entities().len(), 1);
    assert_eq!(game.world.item_entities()[0].stack.count, 1);
    assert_eq!(game.player.inventory.cursor().unwrap().count, held - 1);
}

#[test]
fn queued_cursor_one_throw_stashes_only_remainder_on_menu_close() {
    let mut game = game();
    game.player.inventory = Inventory::from_parts(
        [None; crate::inventory::TOTAL_SLOTS],
        Some(ItemStack::new(ItemType::Dirt, 12)),
        0,
    );

    game.throw_cursor_one();
    assert_eq!(
        game.player.inventory.cursor().unwrap().count,
        12,
        "throwing one does not mutate the cursor immediately"
    );
    game.close_cursor_stack();

    assert!(game.player.inventory.cursor().is_none());
    assert_eq!(
        count_item(&game.player.inventory, ItemType::Dirt),
        11,
        "close stashes only the part not committed to the throw"
    );
    let events = apply_drop_actions(&mut game);
    assert!(events.threw_item);
    assert_eq!(game.world.item_entities().len(), 1);
    assert_eq!(
        game.world.item_entities()[0].stack,
        ItemStack::new(ItemType::Dirt, 1)
    );
    assert_eq!(count_item(&game.player.inventory, ItemType::Dirt), 11);
}

#[test]
fn throwing_with_empty_cursor_is_a_noop() {
    let mut game = game();
    game.player.inventory = crate::inventory::Inventory::new();
    assert!(game.player.inventory.cursor().is_none());
    game.throw_cursor_stack();
    game.throw_cursor_one();
    assert!(game.world.item_entities().is_empty());
}

#[test]
fn drop_selected_one_throws_a_single_held_item() {
    let mut game = game();
    game.player.inventory = filled_inventory();
    game.player.inventory.set_active(0);
    let before = game.player.inventory.selected().unwrap().count;
    game.drop_selected_item(false);
    assert_eq!(
        game.player.inventory.selected().unwrap().count,
        before,
        "selected stack is not mutated until the tick"
    );
    let events = apply_drop_actions(&mut game);
    assert!(events.threw_item);
    assert_eq!(game.world.item_entities().len(), 1);
    assert_eq!(game.world.item_entities()[0].stack.count, 1);
    assert_eq!(
        game.world.item_entities()[0].ticks_lived,
        0,
        "dropped item starts the pickup delay"
    );
    assert_eq!(game.player.inventory.selected().unwrap().count, before - 1);
}

#[test]
fn queued_q_drop_uses_the_action_time_hotbar_slot() {
    let mut game = game();
    let mut slots = [None; crate::inventory::TOTAL_SLOTS];
    slots[0] = Some(ItemStack::new(ItemType::Dirt, 5));
    slots[1] = Some(ItemStack::new(ItemType::Stone, 7));
    game.player.inventory = Inventory::from_parts(slots, None, 0);

    game.drop_selected_item(false);
    game.player.inventory.set_active(1);

    let events = apply_drop_actions(&mut game);
    assert!(events.threw_item);
    assert_eq!(
        game.player.inventory.slot(0),
        Some(&ItemStack::new(ItemType::Dirt, 4))
    );
    assert_eq!(
        game.player.inventory.slot(1),
        Some(&ItemStack::new(ItemType::Stone, 7)),
        "changing selection before the tick must not redirect the drop"
    );
    assert_eq!(game.world.item_entities().len(), 1);
    assert_eq!(
        game.world.item_entities()[0].stack,
        ItemStack::new(ItemType::Dirt, 1)
    );
}

#[test]
fn drop_selected_all_throws_the_whole_held_stack() {
    let mut game = game();
    game.player.inventory = filled_inventory();
    game.player.inventory.set_active(0);
    let before = game.player.inventory.selected().unwrap().count;
    game.drop_selected_item(true);
    assert_eq!(
        game.player.inventory.selected().unwrap().count,
        before,
        "selected stack is not mutated until the tick"
    );
    let events = apply_drop_actions(&mut game);
    assert!(events.threw_item);
    assert_eq!(game.world.item_entities().len(), 1);
    assert_eq!(game.world.item_entities()[0].stack.count, before);
    assert!(
        game.player.inventory.selected().is_none(),
        "held slot emptied"
    );
}

#[test]
fn drop_with_empty_hand_is_a_noop() {
    let mut game = game();
    game.player.inventory = crate::inventory::Inventory::new();
    game.player.inventory.set_active(0);
    assert!(game.player.inventory.selected().is_none());
    game.drop_selected_item(false);
    game.drop_selected_item(true);
    assert!(game.world.item_entities().is_empty());
}

#[test]
fn applying_a_real_throw_arms_the_hand_place_jab() {
    // The Q drop throws from the active hotbar slot.
    {
        let mut game = game();
        game.player.inventory = filled_inventory();
        game.player.inventory.set_active(0);
        game.drop_selected_item(false);
        let events = apply_drop_actions(&mut game);
        assert!(events.threw_item, "Q drop should flick the hand forward");
    }
    // Both inventory drag-outs throw from the cursor-held stack.
    for throw in [
        Game::throw_cursor_stack as fn(&mut Game),
        Game::throw_cursor_one,
    ] {
        let mut game = game();
        game.player.inventory = filled_inventory();
        game.player.inventory.click_slot(0); // pick the stack onto the cursor
        throw(&mut game);
        let events = apply_drop_actions(&mut game);
        assert!(
            events.threw_item,
            "inventory drag-out should flick the hand forward"
        );
    }
}

#[test]
fn a_noop_throw_does_not_arm_the_place_jab() {
    let mut game = game();
    game.player.inventory = crate::inventory::Inventory::new();
    // Nothing in hand or on the cursor: every throw path is a no-op.
    for _ in 0..64 {
        game.player.inventory.decrement_selected();
    }
    game.drop_selected_item(false);
    game.throw_cursor_stack();
    game.throw_cursor_one();
    let events = apply_drop_actions(&mut game);
    assert!(
        !events.threw_item,
        "an empty throw must not animate the hand"
    );
}

#[test]
fn tick_reports_throw_event_only_when_the_drop_is_applied() {
    let mut game = game();
    game.player.inventory = filled_inventory();
    game.player.inventory.set_active(0);
    game.drop_selected_item(false);

    let events = game.tick(1.0 / 60.0, &GameInput::default());
    assert!(
        !events.threw_item,
        "a frame with no fixed tick must not report a queued throw"
    );
    assert!(
        game.player.inventory.selected().is_some(),
        "a frame with no fixed tick must not mutate the inventory"
    );

    let applied = game.tick(TICK_DT, &GameInput::default());
    assert!(applied.threw_item, "the applying tick reports the throw");
    let next = game.tick(TICK_DT, &GameInput::default());
    assert!(!next.threw_item, "the throw event is one-shot");
}
