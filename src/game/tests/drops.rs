use super::super::tick::TICK_DT;
use super::super::GameInput;
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
    assert!(game.server.world.item_entities().is_empty());
    game.server
        .spawn_drops(IVec3::new(2, 3, 4), Block::Dirt, (17, 0));
    assert_eq!(game.server.world.item_entities().len(), 1);
    let d = &game.server.world.item_entities()[0];
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
    let before = count_item(&game.server.sessions[0].player.inventory, item);
    let centre = game.server.sessions[0].player.body_center();
    let mut drop = DroppedItem::new(centre, ItemStack::new(item, 1), 1);
    drop.ticks_lived = ITEM_PICKUP_DELAY_TICKS; // past the pickup delay
    game.server.world.spawn_item(drop);
    game.server.world.tick_item_lifetime();
    game.server.item_pickup_tick(0);
    let after = count_item(&game.server.sessions[0].player.inventory, item);
    assert_eq!(after, before + 1);
    assert!(game.server.world.item_entities().is_empty());
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
    game.server.sessions[0].player.inventory = inv;

    let centre = game.server.sessions[0].player.body_center();
    let mut drop = DroppedItem::new(centre, ItemStack::new(ItemType::Dirt, 5), 1);
    drop.ticks_lived = ITEM_PICKUP_DELAY_TICKS;
    game.server.world.spawn_item(drop);

    // One tick plans the partial pickup and absorbs the requested split because
    // the stack is already inside the pickup radius.
    game.server.world.tick_item_lifetime();
    game.server.item_pickup_tick(0);

    assert_eq!(
        count_item(&game.server.sessions[0].player.inventory, ItemType::Dirt),
        64,
        "took exactly the one dirt that fit"
    );
    let loose: u32 = game
        .server
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
    game.server.sessions[0].player.inventory = inv;

    let chest = game.server.sessions[0].player.body_center();
    for (seed, offset) in [
        (1, Vec3::new(crate::entity::ATTRACT_RADIUS - 0.1, 0.0, 0.0)),
        (2, Vec3::new(0.0, 0.0, crate::entity::ATTRACT_RADIUS - 0.1)),
    ] {
        let mut drop = DroppedItem::new(chest + offset, ItemStack::new(ItemType::Dirt, 1), seed);
        drop.vel = Vec3::ZERO;
        drop.ticks_lived = ITEM_PICKUP_DELAY_TICKS;
        game.server.world.spawn_item(drop);
    }

    game.server.world.tick_item_lifetime();
    game.server.item_pickup_tick(0);

    assert_eq!(
        count_item(&game.server.sessions[0].player.inventory, ItemType::Dirt),
        63
    );
    let requested: u32 = game
        .server
        .world
        .item_entities()
        .iter()
        .filter(|d| d.pickup_requested.is_some())
        .map(|d| d.stack.count as u32)
        .sum();
    assert_eq!(requested, 1, "only the item that fits is requested");
}

#[test]
fn fresh_dropped_item_waits_out_pickup_delay() {
    let mut game = game();
    let item = crate::item::ItemType::Poppy;
    let centre = game.server.sessions[0].player.body_center();
    // ticks_lived 0: sitting right on the player but still inside the delay.
    game.server
        .world
        .spawn_item(DroppedItem::new(centre, ItemStack::new(item, 1), 1));
    game.server.world.tick_item_lifetime();
    game.server.item_pickup_tick(0);
    assert_eq!(
        game.server.world.item_entities().len(),
        1,
        "delay blocks immediate pickup"
    );
    // Each tick ages it by one; once past the delay it is collected.
    for _ in 0..ITEM_PICKUP_DELAY_TICKS {
        game.server.world.tick_item_lifetime();
        game.server.item_pickup_tick(0);
    }
    assert!(game.server.world.item_entities().is_empty());
}

#[test]
fn dropped_item_magnets_toward_player_then_absorbs() {
    let mut game = game();
    let item = crate::item::ItemType::Poppy;
    let before = count_item(&game.server.sessions[0].player.inventory, item);
    let chest = game.server.sessions[0].player.body_center();
    let start = chest + Vec3::new(0.0, crate::entity::ATTRACT_RADIUS - 0.1, 0.0);
    let mut drop = DroppedItem::new(start, ItemStack::new(item, 1), 1);
    drop.ticks_lived = ITEM_PICKUP_DELAY_TICKS; // skip the delay so the magnet engages now
    game.server.world.spawn_item(drop);
    let d0 = (game.server.world.item_entities()[0].pos - chest).length();
    game.server.world.tick_item_lifetime();
    game.server.item_pickup_tick(0);
    let p0 = game.server.sessions[0].id;
    assert_eq!(
        game.server.world.item_entities()[0].pickup_requested,
        Some(p0)
    );
    let pp = game.server.sessions[0].player.body_center();
    game.server.world.tick_item_physics(TICK_DT, &[(p0, pp)]);
    if !game.server.world.item_entities().is_empty() {
        let d1 = (game.server.world.item_entities()[0].pos - chest).length();
        assert!(d1 < d0);
    }
    // Item physics + pickup both run on the fixed tick now: the magnet flies it in,
    // and the pickup absorbs it once it's in range.
    for _ in 0..60 {
        if game.server.world.item_entities().is_empty() {
            break;
        }
        game.server.world.tick_item_lifetime();
        game.server.item_pickup_tick(0);
        let pp = game.server.sessions[0].player.body_center();
        game.server.world.tick_item_physics(TICK_DT, &[(p0, pp)]);
    }
    assert!(game.server.world.item_entities().is_empty());
    assert_eq!(
        count_item(&game.server.sessions[0].player.inventory, item),
        before + 1
    );
}

#[test]
fn a_dropped_item_enters_the_world_on_the_tick_not_the_frame() {
    let mut game = game();
    game.server.sessions[0].player.inventory = filled_inventory(); // a stack of Dirt
    game.server.sessions[0].player.inventory.set_active(0);
    let before = count_item(&game.server.sessions[0].player.inventory, ItemType::Dirt);

    // Q-drop queues intent only; inventory and world stay unchanged until the tick.
    game.drop_selected_item(false);
    assert_eq!(
        count_item(&game.server.sessions[0].player.inventory, ItemType::Dirt),
        before,
        "inventory mutation waits for the tick"
    );
    assert!(
        game.server.world.item_entities().is_empty(),
        "the drop hasn't entered the world until a tick runs"
    );

    // The tick removes the item and materialises the drop as a world entity.
    let events = apply_drop_actions(&mut game);
    assert!(events.player_at(0).threw_item);
    assert_eq!(
        count_item(&game.server.sessions[0].player.inventory, ItemType::Dirt),
        before - 1
    );
    assert_eq!(
        game.server.world.item_entities().len(),
        1,
        "the drop spawns on the tick"
    );
}

#[test]
fn dropped_item_beyond_one_block_is_not_magnet_picked_up() {
    let mut game = game();
    let item = crate::item::ItemType::Poppy;
    let before = count_item(&game.server.sessions[0].player.inventory, item);
    let chest = game.server.sessions[0].player.body_center();
    let start = chest + Vec3::new(crate::entity::ATTRACT_RADIUS + 0.05, 0.0, 0.0);
    let mut drop = DroppedItem::new(start, ItemStack::new(item, 1), 1);
    drop.vel = Vec3::ZERO;
    drop.ticks_lived = ITEM_PICKUP_DELAY_TICKS; // eligible for pickup, so only range gates it
    game.server.world.spawn_item(drop);

    for _ in 0..60 {
        let pp = game.server.sessions[0].player.body_center();
        let p0 = game.server.sessions[0].id;
        game.server.world.tick_item_physics(TICK_DT, &[(p0, pp)]);
        game.server.world.tick_item_lifetime();
        game.server.item_pickup_tick(0);
    }

    assert_eq!(game.server.world.item_entities().len(), 1);
    assert_eq!(
        count_item(&game.server.sessions[0].player.inventory, item),
        before
    );
}

#[test]
fn distant_dropped_item_is_not_picked_up() {
    let mut game = game();
    let far = game.server.sessions[0].player.eye() + Vec3::new(50.0, 0.0, 0.0);
    let mut drop = DroppedItem::new(far, ItemStack::new(crate::item::ItemType::Dirt, 1), 2);
    drop.ticks_lived = ITEM_PICKUP_DELAY_TICKS; // eligible, but far out of range
    game.server.world.spawn_item(drop);
    game.server.world.tick_item_lifetime();
    game.server.item_pickup_tick(0);
    assert_eq!(game.server.world.item_entities().len(), 1);
}

#[test]
fn stale_dropped_item_despawns_on_the_lifetime_tick() {
    let mut game = game();
    let far = game.server.sessions[0].player.eye() + Vec3::new(50.0, 0.0, 0.0);
    let mut item = DroppedItem::new(far, ItemStack::new(crate::item::ItemType::Dirt, 1), 3);
    // One tick short of the lifetime limit: the next fixed tick ages it out.
    item.ticks_lived = ITEM_LIFETIME_TICKS - 1;
    game.server.world.spawn_item(item);
    game.server.world.tick_item_lifetime();
    game.server.item_pickup_tick(0);
    assert!(game.server.world.item_entities().is_empty());
}

/// Cursor throws and hotbar drops: each case queues one intent, checks the
/// source stack stays untouched until the tick, then asserts what the tick
/// spawned and what remained at the source. The queued-throw-survives-menu-
/// close variants add ordering assertions and keep their own tests.
#[test]
fn cursor_throw_and_drop_selected_cases() {
    #[derive(Clone, Copy)]
    enum Source {
        Cursor,
        Selected,
    }
    struct Case {
        label: &'static str,
        /// Whether the source holds a stack of `STACK` dirt (false = empty).
        held: bool,
        source: Source,
        /// Throw/drop the whole stack (true) or a single item (false).
        all: bool,
        /// `Some(count)`: exactly one fresh drop of that size spawns on the
        /// tick. `None`: the action is a no-op, nothing spawns.
        dropped: Option<u8>,
        /// What the source holds after the tick (`None` = emptied).
        remainder: Option<u8>,
    }
    const STACK: u8 = 12;
    let cases = [
        Case {
            label: "throwing the cursor stack drops the whole stack",
            held: true,
            source: Source::Cursor,
            all: true,
            dropped: Some(STACK),
            remainder: None,
        },
        Case {
            label: "throwing one from the cursor drops a single item",
            held: true,
            source: Source::Cursor,
            all: false,
            dropped: Some(1),
            remainder: Some(STACK - 1),
        },
        Case {
            label: "throwing the stack with an empty cursor is a noop",
            held: false,
            source: Source::Cursor,
            all: true,
            dropped: None,
            remainder: None,
        },
        Case {
            label: "throwing one with an empty cursor is a noop",
            held: false,
            source: Source::Cursor,
            all: false,
            dropped: None,
            remainder: None,
        },
        Case {
            label: "drop-selected one throws a single held item",
            held: true,
            source: Source::Selected,
            all: false,
            dropped: Some(1),
            remainder: Some(STACK - 1),
        },
        Case {
            label: "drop-selected all throws the whole held stack",
            held: true,
            source: Source::Selected,
            all: true,
            dropped: Some(STACK),
            remainder: None,
        },
        Case {
            label: "drop one with an empty hand is a noop",
            held: false,
            source: Source::Selected,
            all: false,
            dropped: None,
            remainder: None,
        },
        Case {
            label: "drop all with an empty hand is a noop",
            held: false,
            source: Source::Selected,
            all: true,
            dropped: None,
            remainder: None,
        },
    ];

    fn source_count(game: &super::common::TestGame, source: Source) -> Option<u8> {
        let inv = &game.server.sessions[0].player.inventory;
        match source {
            Source::Cursor => inv.cursor().map(|s| s.count),
            Source::Selected => inv.selected().map(|s| s.count),
        }
    }

    for case in cases {
        let mut game = game();
        game.server.sessions[0].player.inventory = match (case.held, case.source) {
            (false, _) => Inventory::new(),
            (true, Source::Cursor) => Inventory::from_parts(
                [None; crate::inventory::TOTAL_SLOTS],
                Some(ItemStack::new(ItemType::Dirt, STACK)),
                0,
            ),
            (true, Source::Selected) => {
                let mut slots = [None; crate::inventory::TOTAL_SLOTS];
                slots[0] = Some(ItemStack::new(ItemType::Dirt, STACK));
                Inventory::from_parts(slots, None, 0)
            }
        };

        match (case.source, case.all) {
            (Source::Cursor, true) => game.throw_cursor_stack(),
            (Source::Cursor, false) => game.throw_cursor_one(),
            (Source::Selected, all) => game.drop_selected_item(all),
        }
        assert_eq!(
            source_count(&game, case.source),
            case.held.then_some(STACK),
            "[{}] the source stack must not mutate until the tick",
            case.label
        );

        let events = apply_drop_actions(&mut game);
        assert_eq!(
            events.player_at(0).threw_item,
            case.dropped.is_some(),
            "[{}] the hand throw animation fires iff something dropped",
            case.label
        );
        match case.dropped {
            Some(count) => {
                assert_eq!(
                    game.server.world.item_entities().len(),
                    1,
                    "[{}] exactly one drop spawns on the tick",
                    case.label
                );
                let drop = &game.server.world.item_entities()[0];
                assert_eq!(
                    drop.stack,
                    ItemStack::new(ItemType::Dirt, count),
                    "[{}] the dropped stack",
                    case.label
                );
                assert_eq!(
                    drop.ticks_lived, 0,
                    "[{}] a thrown item starts the pickup delay",
                    case.label
                );
            }
            None => assert!(
                game.server.world.item_entities().is_empty(),
                "[{}] a noop throw must spawn nothing",
                case.label
            ),
        }
        assert_eq!(
            source_count(&game, case.source),
            case.remainder,
            "[{}] the remainder at the source",
            case.label
        );
    }
}

#[test]
fn queued_cursor_stack_throw_survives_menu_close_before_tick() {
    let mut game = game();
    game.server.sessions[0].player.inventory = Inventory::from_parts(
        [None; crate::inventory::TOTAL_SLOTS],
        Some(ItemStack::new(ItemType::Dirt, 12)),
        0,
    );

    game.throw_cursor_stack();
    assert!(
        game.server.sessions[0].player.inventory.cursor().is_some(),
        "throwing does not mutate the cursor until another tick/close action"
    );
    game.server.close_cursor_stack_for(0);

    assert!(
        game.server.sessions[0].player.inventory.cursor().is_none(),
        "the committed cursor throw is not stashed on close"
    );
    assert_eq!(
        count_item(&game.server.sessions[0].player.inventory, ItemType::Dirt),
        0
    );
    assert!(
        game.server.world.item_entities().is_empty(),
        "entity spawn still waits for the tick"
    );

    let events = apply_drop_actions(&mut game);
    assert!(events.player_at(0).threw_item);
    assert_eq!(game.server.world.item_entities().len(), 1);
    assert_eq!(
        game.server.world.item_entities()[0].stack,
        ItemStack::new(ItemType::Dirt, 12)
    );
}

#[test]
fn queued_cursor_one_throw_stashes_only_remainder_on_menu_close() {
    let mut game = game();
    game.server.sessions[0].player.inventory = Inventory::from_parts(
        [None; crate::inventory::TOTAL_SLOTS],
        Some(ItemStack::new(ItemType::Dirt, 12)),
        0,
    );

    game.throw_cursor_one();
    assert_eq!(
        game.server.sessions[0]
            .player
            .inventory
            .cursor()
            .unwrap()
            .count,
        12,
        "throwing one does not mutate the cursor immediately"
    );
    game.server.close_cursor_stack_for(0);

    assert!(game.server.sessions[0].player.inventory.cursor().is_none());
    assert_eq!(
        count_item(&game.server.sessions[0].player.inventory, ItemType::Dirt),
        11,
        "close stashes only the part not committed to the throw"
    );
    let events = apply_drop_actions(&mut game);
    assert!(events.player_at(0).threw_item);
    assert_eq!(game.server.world.item_entities().len(), 1);
    assert_eq!(
        game.server.world.item_entities()[0].stack,
        ItemStack::new(ItemType::Dirt, 1)
    );
    assert_eq!(
        count_item(&game.server.sessions[0].player.inventory, ItemType::Dirt),
        11
    );
}

#[test]
fn queued_q_drop_uses_the_action_time_hotbar_slot() {
    let mut game = game();
    let mut slots = [None; crate::inventory::TOTAL_SLOTS];
    slots[0] = Some(ItemStack::new(ItemType::Dirt, 5));
    slots[1] = Some(ItemStack::new(ItemType::Stone, 7));
    game.server.sessions[0].player.inventory = Inventory::from_parts(slots, None, 0);

    game.drop_selected_item(false);
    game.server.sessions[0].player.inventory.set_active(1);

    let events = apply_drop_actions(&mut game);
    assert!(events.player_at(0).threw_item);
    assert_eq!(
        game.server.sessions[0].player.inventory.slot(0),
        Some(&ItemStack::new(ItemType::Dirt, 4))
    );
    assert_eq!(
        game.server.sessions[0].player.inventory.slot(1),
        Some(&ItemStack::new(ItemType::Stone, 7)),
        "changing selection before the tick must not redirect the drop"
    );
    assert_eq!(game.server.world.item_entities().len(), 1);
    assert_eq!(
        game.server.world.item_entities()[0].stack,
        ItemStack::new(ItemType::Dirt, 1)
    );
}

#[test]
fn applying_a_real_throw_arms_the_hand_place_jab() {
    // The Q drop throws from the active hotbar slot.
    {
        let mut game = game();
        game.server.sessions[0].player.inventory = filled_inventory();
        game.server.sessions[0].player.inventory.set_active(0);
        game.drop_selected_item(false);
        let events = apply_drop_actions(&mut game);
        assert!(
            events.player_at(0).threw_item,
            "Q drop should flick the hand forward"
        );
    }
    // Both inventory drag-outs throw from the cursor-held stack.
    for throw in [
        super::common::TestGame::throw_cursor_stack as fn(&mut super::common::TestGame),
        super::common::TestGame::throw_cursor_one,
    ] {
        let mut game = game();
        game.server.sessions[0].player.inventory = filled_inventory();
        game.server.sessions[0].player.inventory.click_slot(0); // pick the stack onto the cursor
        throw(&mut game);
        let events = apply_drop_actions(&mut game);
        assert!(
            events.player_at(0).threw_item,
            "inventory drag-out should flick the hand forward"
        );
    }
}

#[test]
fn a_noop_throw_does_not_arm_the_place_jab() {
    let mut game = game();
    game.server.sessions[0].player.inventory = crate::inventory::Inventory::new();
    // Nothing in hand or on the cursor: every throw path is a no-op.
    for _ in 0..64 {
        game.server.sessions[0]
            .player
            .inventory
            .decrement_selected();
    }
    game.drop_selected_item(false);
    game.throw_cursor_stack();
    game.throw_cursor_one();
    let events = apply_drop_actions(&mut game);
    assert!(
        !events.player_at(0).threw_item,
        "an empty throw must not animate the hand"
    );
}

#[test]
fn throw_animates_once_at_the_click_and_is_never_echoed_back() {
    let mut game = game();
    game.server.sessions[0].player.inventory = filled_inventory();
    game.server.sessions[0].player.inventory.set_active(0);
    game.sync_self_view_for_test();
    game.drop_selected_item(false);

    // The hand animation is CLIENT-OWNED: it fires on the click frame...
    let events = game.tick(1.0 / 60.0, &GameInput::default());
    assert!(
        events.threw_item,
        "the throw animates at the click frame (P0 prediction)"
    );
    assert!(
        game.server.sessions[0]
            .player
            .inventory
            .selected()
            .is_some(),
        "a frame with no fixed tick must not mutate the inventory"
    );

    // ...and the tick that APPLIES the drop server-side does not replay it
    // (the server never echoes self-initiated one-shots).
    let applied = game.tick(TICK_DT, &GameInput::default());
    assert!(
        !applied.threw_item,
        "the server tick must not echo the throw"
    );
    let next = game.tick(TICK_DT, &GameInput::default());
    assert!(!next.threw_item, "the throw event is one-shot");
}

/// The multiplayer pickup contract: reservations are per-requester, so two
/// players each vacuum THEIR OWN adjacent drop within one tick's session
/// sweep — the second session's planner pass can no longer steal or reset the
/// first's marks.
#[test]
fn two_players_each_collect_their_own_adjacent_drop_in_one_tick() {
    let mut game = game();
    let item = crate::item::ItemType::Poppy;
    game.server.sessions[0].player.pos = Vec3::new(0.5, 64.0, 0.5);
    let other = game
        .server
        .add_session_for_test(crate::player::Player::new(Vec3::new(20.5, 64.0, 0.5)));
    for s in [0, other] {
        let centre = game.server.sessions[s].player.body_center();
        let mut drop = DroppedItem::new(centre, ItemStack::new(item, 1), s as u32 + 1);
        drop.ticks_lived = ITEM_PICKUP_DELAY_TICKS;
        game.server.world.spawn_item(drop);
    }

    // One tick's Pickup stage: each session plans + collects in id order.
    game.server.world.tick_item_lifetime();
    for s in [0, other] {
        assert!(game.server.item_pickup_tick(s), "session {s} collects");
    }

    assert!(
        game.server.world.item_entities().is_empty(),
        "both drops collected"
    );
    for s in [0, other] {
        assert_eq!(
            count_item(&game.server.sessions[s].player.inventory, item),
            1,
            "session {s} got exactly its own drop"
        );
    }
}

/// A single drop reachable by two players goes to exactly ONE of them —
/// first come in session order — never duplicated, never lost.
#[test]
fn a_single_drop_between_two_players_goes_to_exactly_one() {
    let mut game = game();
    let item = crate::item::ItemType::Poppy;
    game.server.sessions[0].player.pos = Vec3::new(0.0, 64.0, 0.5);
    let other = game
        .server
        .add_session_for_test(crate::player::Player::new(Vec3::new(1.0, 64.0, 0.5)));
    // Midway between the two body centres: within the absorb radius of both.
    let mid = (game.server.sessions[0].player.body_center()
        + game.server.sessions[other].player.body_center())
        * 0.5;
    let mut drop = DroppedItem::new(mid, ItemStack::new(item, 1), 1);
    drop.ticks_lived = ITEM_PICKUP_DELAY_TICKS;
    game.server.world.spawn_item(drop);

    game.server.world.tick_item_lifetime();
    let took_0 = game.server.item_pickup_tick(0);
    let took_1 = game.server.item_pickup_tick(other);

    assert!(
        game.server.world.item_entities().is_empty(),
        "the drop is gone"
    );
    assert!(took_0 && !took_1, "first come in session order takes it");
    let total = count_item(&game.server.sessions[0].player.inventory, item)
        + count_item(&game.server.sessions[other].player.inventory, item);
    assert_eq!(total, 1, "no dupe, no vanish");
}
