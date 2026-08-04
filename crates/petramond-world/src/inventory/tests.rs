use super::*;
use crate::item::ItemType;

fn item(t: ItemType, n: u8) -> ItemStack {
    ItemStack::new(t, n)
}

#[test]
fn new_is_empty() {
    let inv = Inventory::new();
    assert_eq!(inv.active_slot(), 0);
    assert!(inv.cursor().is_none());
    // Every slot starts empty: the player gathers items from the world.
    for i in 0..TOTAL_SLOTS {
        assert!(inv.slot(i).is_none(), "slot {i} should be empty");
    }
}

#[test]
fn selected_follows_active() {
    let mut inv = empty_inv();
    inv.slots[0] = Some(item(ItemType::Grass, 1));
    inv.slots[2] = Some(item(ItemType::Stone, 1));
    assert_eq!(inv.selected().unwrap().item, ItemType::Grass);
    inv.set_active(2);
    assert_eq!(inv.selected().unwrap().item, ItemType::Stone);
}

#[test]
fn replace_selected_one_swaps_in_place_and_splits_stacks() {
    // A single-count stack swaps type in its own slot (keeps hotbar position).
    let mut inv = empty_inv();
    inv.slots[0] = Some(item(ItemType::WoodenBucket, 1));
    assert!(inv.replace_selected_one(item(ItemType::WaterBucket, 1)));
    assert_eq!(inv.selected().unwrap().item, ItemType::WaterBucket);
    assert_eq!(inv.selected().unwrap().count, 1);

    // One of a larger stack converts; the rest stays selected and the
    // replacement lands in another slot.
    let mut inv = empty_inv();
    inv.slots[0] = Some(item(ItemType::WoodenBucket, 3));
    assert!(inv.replace_selected_one(item(ItemType::WaterBucket, 1)));
    assert_eq!(inv.selected().unwrap().item, ItemType::WoodenBucket);
    assert_eq!(inv.selected().unwrap().count, 2);
    let water: u32 = (0..TOTAL_SLOTS)
        .filter_map(|i| inv.slot(i))
        .filter(|s| s.item == ItemType::WaterBucket)
        .map(|s| s.count as u32)
        .sum();
    assert_eq!(water, 1);
}

#[test]
fn replace_selected_one_refuses_when_replacement_has_no_room() {
    // Every other slot full and a >1 selected stack: the conversion has
    // nowhere to put the replacement, so NOTHING may change.
    let mut inv = empty_inv();
    for i in 1..TOTAL_SLOTS {
        inv.slots[i] = Some(item(ItemType::Stone, 64));
    }
    inv.slots[0] = Some(item(ItemType::WoodenBucket, 2));
    assert!(!inv.replace_selected_one(item(ItemType::WaterBucket, 1)));
    assert_eq!(inv.selected().unwrap().item, ItemType::WoodenBucket);
    assert_eq!(inv.selected().unwrap().count, 2);

    // An empty hand has nothing to swap.
    let mut inv = empty_inv();
    assert!(!inv.replace_selected_one(item(ItemType::WaterBucket, 1)));
    assert!(inv.selected().is_none());
}

#[test]
fn set_active_clamps() {
    let mut inv = Inventory::new();
    inv.set_active(200);
    assert_eq!(inv.active_slot(), (HOTBAR_LEN - 1) as u8);
    inv.set_active(3);
    assert_eq!(inv.active_slot(), 3);
}

#[test]
fn scroll_active_wraps() {
    let mut inv = Inventory::new();
    inv.set_active(0);
    inv.scroll_active(-1);
    assert_eq!(inv.active_slot(), (HOTBAR_LEN - 1) as u8); // wrap to 8
    inv.scroll_active(1);
    assert_eq!(inv.active_slot(), 0); // wrap back to 0
                                      // Large magnitudes reduce modulo HOTBAR_LEN.
    inv.set_active(0);
    inv.scroll_active(10); // 10 % 9 == 1
    assert_eq!(inv.active_slot(), 1);
    inv.scroll_active(-11); // (1 - 11) rem_euclid 9 == 8
    assert_eq!(inv.active_slot(), 8);
}

#[test]
fn add_merges_into_existing_then_overflows() {
    let mut inv = Inventory::new();
    // Drain hotbar/main of dirt first: start empty for clarity.
    let mut inv = {
        // build an empty inventory by clearing slots
        for i in 0..TOTAL_SLOTS {
            inv.slots[i] = None;
        }
        inv
    };
    // Seed slot 0 with 60 dirt.
    inv.slots[0] = Some(item(ItemType::Dirt, 60));
    // Adding 10 should top slot 0 to 64 (max) and put 6 into the next empty slot.
    let leftover = inv.add(item(ItemType::Dirt, 10));
    assert!(leftover.is_none());
    assert_eq!(inv.slot(0).unwrap().count, 64);
    // First empty slot after slot 0 is slot 1.
    assert_eq!(inv.slot(1).unwrap().item, ItemType::Dirt);
    assert_eq!(inv.slot(1).unwrap().count, 6);
}

#[test]
fn add_splits_large_stack_across_empty_slots() {
    let mut inv = Inventory::new();
    for i in 0..TOTAL_SLOTS {
        inv.slots[i] = None;
    }
    // 64 is the max, so adding a fresh 64 lands in exactly one slot.
    let leftover = inv.add(item(ItemType::Stone, 64));
    assert!(leftover.is_none());
    assert_eq!(inv.slot(0).unwrap().count, 64);
    assert!(inv.slot(1).is_none());
}

#[test]
fn add_returns_leftover_when_full() {
    // A one-slot-only inventory: fill every slot with a different full stack
    // so neither merge nor empty-slot placement can absorb more dirt.
    let mut inv = Inventory::new();
    for i in 0..TOTAL_SLOTS {
        inv.slots[i] = Some(item(ItemType::Stone, 64));
    }
    let leftover = inv.add(item(ItemType::Dirt, 5));
    assert_eq!(leftover, Some(item(ItemType::Dirt, 5)));

    // Partial absorption: one matching slot with 2 spaces left.
    let mut inv = Inventory::new();
    for i in 0..TOTAL_SLOTS {
        inv.slots[i] = Some(item(ItemType::Stone, 64));
    }
    inv.slots[5] = Some(item(ItemType::Dirt, 62)); // 2 spaces
    let leftover = inv.add(item(ItemType::Dirt, 5));
    assert_eq!(inv.slot(5).unwrap().count, 64);
    assert_eq!(leftover, Some(item(ItemType::Dirt, 3)));
}

#[test]
fn add_empty_is_noop() {
    let mut inv = Inventory::new();
    assert!(inv.add(item(ItemType::Air, 0)).is_none());
    assert!(inv.add(item(ItemType::Dirt, 0)).is_none());
}

#[test]
fn decrement_selected_clears_at_zero() {
    let mut inv = Inventory::new();
    for i in 0..TOTAL_SLOTS {
        inv.slots[i] = None;
    }
    inv.slots[0] = Some(item(ItemType::Dirt, 2));
    inv.set_active(0);
    inv.decrement_selected();
    assert_eq!(inv.selected().unwrap().count, 1);
    inv.decrement_selected();
    assert!(inv.selected().is_none());
    // No-op on empty.
    inv.decrement_selected();
    assert!(inv.selected().is_none());
}

#[test]
fn click_slot_pick_and_drop() {
    let mut inv = Inventory::new();
    for i in 0..TOTAL_SLOTS {
        inv.slots[i] = None;
    }
    inv.slots[0] = Some(item(ItemType::Stone, 10));

    // cursor empty, slot full -> pick whole stack.
    inv.click_slot(0);
    assert!(inv.slot(0).is_none());
    assert_eq!(inv.cursor(), Some(&item(ItemType::Stone, 10)));

    // cursor full, slot empty -> drop into slot.
    inv.click_slot(5);
    assert!(inv.cursor().is_none());
    assert_eq!(inv.slot(5), Some(&item(ItemType::Stone, 10)));
}

#[test]
fn stash_cursor_merges_matching_stacks_before_empty_slots() {
    let mut inv = Inventory::new();
    for i in 0..TOTAL_SLOTS {
        inv.slots[i] = Some(item(ItemType::Stone, 64));
    }
    inv.slots[3] = Some(item(ItemType::Dirt, 60));
    inv.slots[5] = None;
    inv.cursor = Some(item(ItemType::Dirt, 4));

    assert_eq!(inv.stash_cursor_in_inventory(), None);
    assert!(inv.cursor().is_none());
    assert_eq!(inv.slot(3), Some(&item(ItemType::Dirt, 64)));
    assert!(inv.slot(5).is_none(), "matching partial stack filled first");
}

#[test]
fn stash_cursor_uses_empty_slot_after_matching_partials() {
    let mut inv = Inventory::new();
    for i in 0..TOTAL_SLOTS {
        inv.slots[i] = Some(item(ItemType::Stone, 64));
    }
    inv.slots[3] = Some(item(ItemType::Dirt, 60));
    inv.slots[5] = None;
    inv.cursor = Some(item(ItemType::Dirt, 8));

    assert_eq!(inv.stash_cursor_in_inventory(), None);
    assert!(inv.cursor().is_none());
    assert_eq!(inv.slot(3), Some(&item(ItemType::Dirt, 64)));
    assert_eq!(inv.slot(5), Some(&item(ItemType::Dirt, 4)));
}

#[test]
fn stash_cursor_returns_only_unabsorbed_leftover() {
    let mut inv = Inventory::new();
    for i in 0..TOTAL_SLOTS {
        inv.slots[i] = Some(item(ItemType::Stone, 64));
    }
    inv.slots[3] = Some(item(ItemType::Dirt, 60));
    inv.cursor = Some(item(ItemType::Dirt, 10));

    assert_eq!(
        inv.stash_cursor_in_inventory(),
        Some(item(ItemType::Dirt, 6))
    );
    assert!(inv.cursor().is_none());
    assert_eq!(inv.slot(3), Some(&item(ItemType::Dirt, 64)));
}

#[test]
fn stash_cursor_returns_stack_when_no_free_slot() {
    let mut inv = Inventory::new();
    for i in 0..TOTAL_SLOTS {
        inv.slots[i] = Some(item(ItemType::Stone, 64));
    }
    inv.cursor = Some(item(ItemType::Dirt, 4));

    assert_eq!(
        inv.stash_cursor_in_inventory(),
        Some(item(ItemType::Dirt, 4))
    );
    assert!(inv.cursor().is_none());
}

#[test]
fn click_slot_merge_same_item() {
    let mut inv = Inventory::new();
    for i in 0..TOTAL_SLOTS {
        inv.slots[i] = None;
    }
    inv.slots[0] = Some(item(ItemType::Dirt, 60));
    inv.cursor = Some(item(ItemType::Dirt, 10));

    // both full, same item -> merge up to max (64), remainder (6) on cursor.
    inv.click_slot(0);
    assert_eq!(inv.slot(0).unwrap().count, 64);
    assert_eq!(inv.cursor(), Some(&item(ItemType::Dirt, 6)));
}

#[test]
fn click_slot_merge_fully_clears_cursor() {
    let mut inv = Inventory::new();
    for i in 0..TOTAL_SLOTS {
        inv.slots[i] = None;
    }
    inv.slots[0] = Some(item(ItemType::Dirt, 60));
    inv.cursor = Some(item(ItemType::Dirt, 4));
    inv.click_slot(0);
    assert_eq!(inv.slot(0).unwrap().count, 64);
    assert!(inv.cursor().is_none());
}

#[test]
fn click_slot_swap_different_item() {
    let mut inv = Inventory::new();
    for i in 0..TOTAL_SLOTS {
        inv.slots[i] = None;
    }
    inv.slots[0] = Some(item(ItemType::Stone, 5));
    inv.cursor = Some(item(ItemType::Dirt, 7));

    // both full, different item -> swap.
    inv.click_slot(0);
    assert_eq!(inv.slot(0), Some(&item(ItemType::Dirt, 7)));
    assert_eq!(inv.cursor(), Some(&item(ItemType::Stone, 5)));
}

#[test]
fn click_slot_swap_when_slot_full_same_item() {
    let mut inv = Inventory::new();
    for i in 0..TOTAL_SLOTS {
        inv.slots[i] = None;
    }
    // Slot already at max -> no room to merge, so swap even if same item.
    inv.slots[0] = Some(item(ItemType::Dirt, 64));
    inv.cursor = Some(item(ItemType::Dirt, 7));
    inv.click_slot(0);
    assert_eq!(inv.slot(0), Some(&item(ItemType::Dirt, 7)));
    assert_eq!(inv.cursor(), Some(&item(ItemType::Dirt, 64)));
}

#[test]
fn click_slot_out_of_range_is_noop() {
    let mut inv = Inventory::new();
    inv.cursor = Some(item(ItemType::Dirt, 1));
    inv.click_slot(TOTAL_SLOTS); // out of range
    assert_eq!(inv.cursor(), Some(&item(ItemType::Dirt, 1)));
}

fn empty_inv() -> Inventory {
    let mut inv = Inventory::new();
    for i in 0..TOTAL_SLOTS {
        inv.slots[i] = None;
    }
    inv
}

#[test]
fn right_click_splits_odd_stack_dragging_larger_half() {
    let mut inv = empty_inv();
    inv.slots[0] = Some(item(ItemType::Stone, 5));
    // 5 -> drag ceil(5/2)=3, leave 2 behind.
    inv.right_click_slot(0);
    assert_eq!(inv.cursor(), Some(&item(ItemType::Stone, 3)));
    assert_eq!(inv.slot(0), Some(&item(ItemType::Stone, 2)));
}

#[test]
fn right_click_splits_even_stack_in_half() {
    let mut inv = empty_inv();
    inv.slots[0] = Some(item(ItemType::Stone, 8));
    inv.right_click_slot(0);
    assert_eq!(inv.cursor(), Some(&item(ItemType::Stone, 4)));
    assert_eq!(inv.slot(0), Some(&item(ItemType::Stone, 4)));
}

#[test]
fn right_click_single_item_picks_it_up() {
    let mut inv = empty_inv();
    inv.slots[0] = Some(item(ItemType::Stone, 1));
    inv.right_click_slot(0);
    assert_eq!(inv.cursor(), Some(&item(ItemType::Stone, 1)));
    assert!(inv.slot(0).is_none());
}

#[test]
fn right_click_places_one_into_empty_slot() {
    let mut inv = empty_inv();
    inv.cursor = Some(item(ItemType::Dirt, 4));
    inv.right_click_slot(3);
    assert_eq!(inv.slot(3), Some(&item(ItemType::Dirt, 1)));
    assert_eq!(inv.cursor(), Some(&item(ItemType::Dirt, 3)));
}

#[test]
fn right_click_adds_one_to_matching_slot() {
    let mut inv = empty_inv();
    inv.slots[3] = Some(item(ItemType::Dirt, 10));
    inv.cursor = Some(item(ItemType::Dirt, 4));
    inv.right_click_slot(3);
    assert_eq!(inv.slot(3), Some(&item(ItemType::Dirt, 11)));
    assert_eq!(inv.cursor(), Some(&item(ItemType::Dirt, 3)));
}

#[test]
fn right_click_last_held_item_clears_cursor() {
    let mut inv = empty_inv();
    inv.cursor = Some(item(ItemType::Dirt, 1));
    inv.right_click_slot(3);
    assert_eq!(inv.slot(3), Some(&item(ItemType::Dirt, 1)));
    assert!(inv.cursor().is_none());
}

#[test]
fn right_click_different_item_or_full_is_noop() {
    // Different item: leave both untouched.
    let mut inv = empty_inv();
    inv.slots[3] = Some(item(ItemType::Stone, 5));
    inv.cursor = Some(item(ItemType::Dirt, 4));
    inv.right_click_slot(3);
    assert_eq!(inv.slot(3), Some(&item(ItemType::Stone, 5)));
    assert_eq!(inv.cursor(), Some(&item(ItemType::Dirt, 4)));
    // Same item but slot already full: no room, leave both untouched.
    let mut inv = empty_inv();
    inv.slots[3] = Some(item(ItemType::Dirt, 64));
    inv.cursor = Some(item(ItemType::Dirt, 4));
    inv.right_click_slot(3);
    assert_eq!(inv.slot(3), Some(&item(ItemType::Dirt, 64)));
    assert_eq!(inv.cursor(), Some(&item(ItemType::Dirt, 4)));
}

#[test]
fn collect_to_cursor_gathers_matching_until_full() {
    let mut inv = empty_inv();
    inv.cursor = Some(item(ItemType::Dirt, 5));
    inv.slots[0] = Some(item(ItemType::Dirt, 10)); // hotbar
    inv.slots[3] = Some(item(ItemType::Dirt, 20)); // hotbar
    inv.slots[HOTBAR_LEN] = Some(item(ItemType::Dirt, 40)); // main grid
    inv.slots[2] = Some(item(ItemType::Stone, 30)); // different item: untouched

    inv.collect_to_cursor();

    // 5 + 10 + 20 + 40 = 75, capped at 64, with 11 dirt left behind.
    assert_eq!(inv.cursor(), Some(&item(ItemType::Dirt, 64)));
    assert!(inv.slot(0).is_none(), "first partial fully drained");
    assert!(inv.slot(3).is_none(), "second partial fully drained");
    assert_eq!(
        inv.slot(HOTBAR_LEN).unwrap().count,
        11,
        "last source keeps the remainder"
    );
    assert_eq!(
        inv.slot(2),
        Some(&item(ItemType::Stone, 30)),
        "other items untouched"
    );
}

#[test]
fn collect_to_cursor_drains_partials_before_breaking_full_stacks() {
    let mut inv = empty_inv();
    inv.cursor = Some(item(ItemType::Dirt, 1));
    inv.slots[0] = Some(item(ItemType::Dirt, 64)); // full stack, before the partial
    inv.slots[1] = Some(item(ItemType::Dirt, 5)); // partial

    inv.collect_to_cursor();

    // Partial taken first (1 + 5 = 6); the cursor then pulls the remaining 58
    // from the full stack, leaving it with 6 rather than splitting it first.
    assert_eq!(inv.cursor(), Some(&item(ItemType::Dirt, 64)));
    assert!(inv.slot(1).is_none(), "partial consumed");
    assert_eq!(
        inv.slot(0).unwrap().count,
        6,
        "full stack broken only for the remainder"
    );
}

#[test]
fn collect_to_cursor_leaves_full_stacks_intact_when_partials_suffice() {
    let mut inv = empty_inv();
    inv.cursor = Some(item(ItemType::Dirt, 60));
    inv.slots[0] = Some(item(ItemType::Dirt, 64)); // full
    inv.slots[1] = Some(item(ItemType::Dirt, 4)); // exactly tops the cursor off

    inv.collect_to_cursor();

    assert_eq!(inv.cursor(), Some(&item(ItemType::Dirt, 64)));
    assert!(inv.slot(1).is_none(), "partial consumed to fill the cursor");
    assert_eq!(inv.slot(0).unwrap().count, 64, "full stack never touched");
}

#[test]
fn collect_to_cursor_is_noop_when_cursor_empty_or_full() {
    // Empty cursor: nothing to fill, slots untouched.
    let mut inv = empty_inv();
    inv.slots[0] = Some(item(ItemType::Dirt, 10));
    inv.collect_to_cursor();
    assert!(inv.cursor().is_none());
    assert_eq!(inv.slot(0), Some(&item(ItemType::Dirt, 10)));

    // Full cursor: no room, slots untouched.
    let mut inv = empty_inv();
    inv.cursor = Some(item(ItemType::Dirt, 64));
    inv.slots[0] = Some(item(ItemType::Dirt, 10));
    inv.collect_to_cursor();
    assert_eq!(inv.cursor(), Some(&item(ItemType::Dirt, 64)));
    assert_eq!(inv.slot(0), Some(&item(ItemType::Dirt, 10)));
}

#[test]
fn collect_to_cursor_ignores_non_matching_items() {
    let mut inv = empty_inv();
    inv.cursor = Some(item(ItemType::Dirt, 5));
    inv.slots[0] = Some(item(ItemType::Stone, 64));
    inv.slots[1] = Some(item(ItemType::Sand, 30));
    inv.collect_to_cursor();
    assert_eq!(
        inv.cursor(),
        Some(&item(ItemType::Dirt, 5)),
        "nothing to gather"
    );
    assert_eq!(inv.slot(0), Some(&item(ItemType::Stone, 64)));
    assert_eq!(inv.slot(1), Some(&item(ItemType::Sand, 30)));
}

#[test]
fn take_cursor_takes_the_whole_stack() {
    let mut inv = empty_inv();
    inv.cursor = Some(item(ItemType::Dirt, 3));
    assert_eq!(inv.take_cursor(), Some(item(ItemType::Dirt, 3)));
    assert!(inv.cursor().is_none());
    // Empty cursor: None.
    assert!(inv.take_cursor().is_none());
}

#[test]
fn shift_move_hotbar_to_main_grid_uses_first_empty() {
    let mut inv = empty_inv();
    inv.slots[2] = Some(item(ItemType::Stone, 20)); // hotbar
    inv.shift_move_slot(2);
    assert!(inv.slot(2).is_none(), "source slot emptied");
    // First main-grid slot is index HOTBAR_LEN (9).
    assert_eq!(inv.slot(HOTBAR_LEN), Some(&item(ItemType::Stone, 20)));
}

#[test]
fn shift_move_main_to_hotbar_merges_then_fills() {
    let mut inv = empty_inv();
    inv.slots[0] = Some(item(ItemType::Dirt, 60)); // hotbar, room for 4
    inv.slots[HOTBAR_LEN] = Some(item(ItemType::Dirt, 10)); // main grid
    inv.shift_move_slot(HOTBAR_LEN);
    // 4 merge into slot 0 (to 64), remaining 6 fill the next empty hotbar slot.
    assert_eq!(inv.slot(0), Some(&item(ItemType::Dirt, 64)));
    assert_eq!(inv.slot(1), Some(&item(ItemType::Dirt, 6)));
    assert!(inv.slot(HOTBAR_LEN).is_none());
}

#[test]
fn shift_move_leaves_remainder_when_destination_full() {
    let mut inv = empty_inv();
    // Fill the whole main grid with non-matching full stacks.
    for i in HOTBAR_LEN..TOTAL_SLOTS {
        inv.slots[i] = Some(item(ItemType::Stone, 64));
    }
    inv.slots[0] = Some(item(ItemType::Dirt, 30)); // hotbar source
    inv.shift_move_slot(0);
    // No room in the main grid: the stack stays put (click ignored).
    assert_eq!(inv.slot(0), Some(&item(ItemType::Dirt, 30)));
}

#[test]
fn shift_move_empty_slot_is_noop() {
    let mut inv = empty_inv();
    inv.shift_move_slot(5);
    assert!(inv.slot(5).is_none());
}

#[test]
fn click_external_slot_matches_internal_semantics() {
    // The external-slot click logic must mirror click_slot exactly.
    let mut inv = empty_inv();
    let mut ext: Option<ItemStack> = Some(item(ItemType::Stone, 10));
    // cursor empty, slot full -> pick up.
    inv.click_external_slot(&mut ext);
    assert!(ext.is_none());
    assert_eq!(inv.cursor(), Some(&item(ItemType::Stone, 10)));
    // cursor full, slot empty -> drop.
    inv.click_external_slot(&mut ext);
    assert_eq!(ext, Some(item(ItemType::Stone, 10)));
    assert!(inv.cursor().is_none());
    // right-click split off the larger half.
    inv.right_click_external_slot(&mut ext);
    assert_eq!(inv.cursor(), Some(&item(ItemType::Stone, 5)));
    assert_eq!(ext, Some(item(ItemType::Stone, 5)));
}

#[test]
fn can_add_checks_full_fit() {
    let mut inv = empty_inv();
    assert!(inv.can_add(item(ItemType::Dirt, 64)));
    // Fill the grid: slot 0 a partial dirt (room for 4), the rest full stone.
    inv.slots[0] = Some(item(ItemType::Dirt, 60));
    for i in 1..TOTAL_SLOTS {
        inv.slots[i] = Some(item(ItemType::Stone, 64));
    }
    assert!(inv.can_add(item(ItemType::Dirt, 4)));
    assert!(!inv.can_add(item(ItemType::Dirt, 5)));
}

#[test]
fn fits_count_reports_how_many_would_land() {
    // Empty inventory: the whole stack fits.
    let mut inv = empty_inv();
    assert_eq!(inv.fits_count(item(ItemType::Dirt, 40)), 40);

    // One slot at 63 dirt, the rest full of stone: room for exactly one dirt.
    inv.slots[0] = Some(item(ItemType::Dirt, 63));
    for i in 1..TOTAL_SLOTS {
        inv.slots[i] = Some(item(ItemType::Stone, 64));
    }
    assert_eq!(inv.fits_count(item(ItemType::Dirt, 5)), 1, "only one space");
    assert_eq!(
        inv.fits_count(item(ItemType::Dirt, 1)),
        1,
        "exactly fills it"
    );

    // That slot now maxed: no room at all.
    inv.slots[0] = Some(item(ItemType::Dirt, 64));
    assert_eq!(inv.fits_count(item(ItemType::Dirt, 5)), 0);

    // Two partial matching stacks accumulate their room (2 + 4 = 6 spaces).
    let mut inv = empty_inv();
    inv.slots[0] = Some(item(ItemType::Dirt, 62)); // room 2
    inv.slots[1] = Some(item(ItemType::Dirt, 60)); // room 4
    for i in 2..TOTAL_SLOTS {
        inv.slots[i] = Some(item(ItemType::Stone, 64));
    }
    assert_eq!(
        inv.fits_count(item(ItemType::Dirt, 10)),
        6,
        "summed partial room"
    );
    assert_eq!(
        inv.fits_count(item(ItemType::Dirt, 3)),
        3,
        "capped at the stack"
    );

    // An empty stack fits nothing.
    assert_eq!(inv.fits_count(item(ItemType::Dirt, 0)), 0);
}
