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
    let mut inv = empty_inv();
    inv.slots[0] = Some(item(ItemType::WoodenBucket, 1));
    assert!(inv.replace_selected_one(item(ItemType::WaterBucket, 1)));
    assert_eq!(inv.selected().unwrap().item, ItemType::WaterBucket);
    assert_eq!(inv.selected().unwrap().count, 1);

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
    let mut inv = empty_inv();
    for i in 1..TOTAL_SLOTS {
        inv.slots[i] = Some(item(ItemType::Stone, 64));
    }
    inv.slots[0] = Some(item(ItemType::WoodenBucket, 2));
    assert!(!inv.replace_selected_one(item(ItemType::WaterBucket, 1)));
    assert_eq!(inv.selected().unwrap().item, ItemType::WoodenBucket);
    assert_eq!(inv.selected().unwrap().count, 2);

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
    assert_eq!(inv.active_slot(), (HOTBAR_LEN - 1) as u8);
    inv.scroll_active(1);
    assert_eq!(inv.active_slot(), 0);
    inv.set_active(0);
    inv.scroll_active(10);
    assert_eq!(inv.active_slot(), 1);
    inv.scroll_active(-11);
    assert_eq!(inv.active_slot(), 8);
}

#[test]
fn add_merges_into_existing_then_overflows() {
    let mut inv = Inventory::new();
    let mut inv = {
        for i in 0..TOTAL_SLOTS {
            inv.slots[i] = None;
        }
        inv
    };
    inv.slots[0] = Some(item(ItemType::Dirt, 60));
    let leftover = inv.add(item(ItemType::Dirt, 10));
    assert!(leftover.is_none());
    assert_eq!(inv.slot(0).unwrap().count, 64);
    assert_eq!(inv.slot(1).unwrap().item, ItemType::Dirt);
    assert_eq!(inv.slot(1).unwrap().count, 6);
}

#[test]
fn add_splits_large_stack_across_empty_slots() {
    let mut inv = Inventory::new();
    for i in 0..TOTAL_SLOTS {
        inv.slots[i] = None;
    }
    let leftover = inv.add(item(ItemType::Stone, 64));
    assert!(leftover.is_none());
    assert_eq!(inv.slot(0).unwrap().count, 64);
    assert!(inv.slot(1).is_none());
}

#[test]
fn add_returns_leftover_when_full() {
    let mut inv = Inventory::new();
    for i in 0..TOTAL_SLOTS {
        inv.slots[i] = Some(item(ItemType::Stone, 64));
    }
    let leftover = inv.add(item(ItemType::Dirt, 5));
    assert_eq!(leftover, Some(item(ItemType::Dirt, 5)));

    let mut inv = Inventory::new();
    for i in 0..TOTAL_SLOTS {
        inv.slots[i] = Some(item(ItemType::Stone, 64));
    }
    inv.slots[5] = Some(item(ItemType::Dirt, 62));
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

    inv.click_slot(0);
    assert!(inv.slot(0).is_none());
    assert_eq!(inv.cursor(), Some(&item(ItemType::Stone, 10)));

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
    inv.click_slot(TOTAL_SLOTS);
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
    let mut inv = empty_inv();
    inv.slots[3] = Some(item(ItemType::Stone, 5));
    inv.cursor = Some(item(ItemType::Dirt, 4));
    inv.right_click_slot(3);
    assert_eq!(inv.slot(3), Some(&item(ItemType::Stone, 5)));
    assert_eq!(inv.cursor(), Some(&item(ItemType::Dirt, 4)));
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
    inv.slots[0] = Some(item(ItemType::Dirt, 10));
    inv.slots[3] = Some(item(ItemType::Dirt, 20));
    inv.slots[HOTBAR_LEN] = Some(item(ItemType::Dirt, 40));
    inv.slots[2] = Some(item(ItemType::Stone, 30));

    inv.collect_to_cursor();

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
    inv.slots[0] = Some(item(ItemType::Dirt, 64));
    inv.slots[1] = Some(item(ItemType::Dirt, 5));

    inv.collect_to_cursor();

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
    inv.slots[0] = Some(item(ItemType::Dirt, 64));
    inv.slots[1] = Some(item(ItemType::Dirt, 4));

    inv.collect_to_cursor();

    assert_eq!(inv.cursor(), Some(&item(ItemType::Dirt, 64)));
    assert!(inv.slot(1).is_none(), "partial consumed to fill the cursor");
    assert_eq!(inv.slot(0).unwrap().count, 64, "full stack never touched");
}

#[test]
fn collect_to_cursor_is_noop_when_cursor_empty_or_full() {
    let mut inv = empty_inv();
    inv.slots[0] = Some(item(ItemType::Dirt, 10));
    inv.collect_to_cursor();
    assert!(inv.cursor().is_none());
    assert_eq!(inv.slot(0), Some(&item(ItemType::Dirt, 10)));

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
    assert!(inv.take_cursor().is_none());
}

#[test]
fn shift_move_hotbar_to_main_grid_uses_first_empty() {
    let mut inv = empty_inv();
    inv.slots[2] = Some(item(ItemType::Stone, 20));
    inv.shift_move_slot(2);
    assert!(inv.slot(2).is_none(), "source slot emptied");
    assert_eq!(inv.slot(HOTBAR_LEN), Some(&item(ItemType::Stone, 20)));
}

#[test]
fn shift_move_main_to_hotbar_merges_then_fills() {
    let mut inv = empty_inv();
    inv.slots[0] = Some(item(ItemType::Dirt, 60));
    inv.slots[HOTBAR_LEN] = Some(item(ItemType::Dirt, 10));
    inv.shift_move_slot(HOTBAR_LEN);
    assert_eq!(inv.slot(0), Some(&item(ItemType::Dirt, 64)));
    assert_eq!(inv.slot(1), Some(&item(ItemType::Dirt, 6)));
    assert!(inv.slot(HOTBAR_LEN).is_none());
}

#[test]
fn shift_move_leaves_remainder_when_destination_full() {
    let mut inv = empty_inv();
    for i in HOTBAR_LEN..TOTAL_SLOTS {
        inv.slots[i] = Some(item(ItemType::Stone, 64));
    }
    inv.slots[0] = Some(item(ItemType::Dirt, 30));
    inv.shift_move_slot(0);
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
    let mut inv = empty_inv();
    let mut ext: Option<ItemStack> = Some(item(ItemType::Stone, 10));
    inv.click_external_slot(&mut ext);
    assert!(ext.is_none());
    assert_eq!(inv.cursor(), Some(&item(ItemType::Stone, 10)));
    inv.click_external_slot(&mut ext);
    assert_eq!(ext, Some(item(ItemType::Stone, 10)));
    assert!(inv.cursor().is_none());
    inv.right_click_external_slot(&mut ext);
    assert_eq!(inv.cursor(), Some(&item(ItemType::Stone, 5)));
    assert_eq!(ext, Some(item(ItemType::Stone, 5)));
}

#[test]
fn can_add_checks_full_fit() {
    let mut inv = empty_inv();
    assert!(inv.can_add(item(ItemType::Dirt, 64)));
    inv.slots[0] = Some(item(ItemType::Dirt, 60));
    for i in 1..TOTAL_SLOTS {
        inv.slots[i] = Some(item(ItemType::Stone, 64));
    }
    assert!(inv.can_add(item(ItemType::Dirt, 4)));
    assert!(!inv.can_add(item(ItemType::Dirt, 5)));
}

#[test]
fn fits_count_reports_how_many_would_land() {
    let mut inv = empty_inv();
    assert_eq!(inv.fits_count(item(ItemType::Dirt, 40)), 40);

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

    inv.slots[0] = Some(item(ItemType::Dirt, 64));
    assert_eq!(inv.fits_count(item(ItemType::Dirt, 5)), 0);

    let mut inv = empty_inv();
    inv.slots[0] = Some(item(ItemType::Dirt, 62));
    inv.slots[1] = Some(item(ItemType::Dirt, 60));
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

    assert_eq!(inv.fits_count(item(ItemType::Dirt, 0)), 0);
}

#[test]
fn off_hand_swaps_with_a_named_slot_both_ways() {
    let mut inv = empty_inv();
    inv.slots[0] = Some(item(ItemType::Dirt, 5));

    inv.swap_off_hand_with_slot(0);
    assert!(inv.selected().is_none());
    assert_eq!(inv.off_hand().map(|s| s.count), Some(5));

    inv.swap_off_hand_with_slot(0);
    assert_eq!(inv.selected().map(|s| s.count), Some(5));
    assert!(inv.off_hand().is_none());

    let before = inv.revision();
    inv.set_active(4);
    let after_select = inv.revision();
    assert!(after_select > before, "selection change bumps");
    inv.swap_off_hand_with_slot(4);
    assert_eq!(
        inv.revision(),
        after_select,
        "a both-empty swap is a true no-op"
    );
}

#[test]
fn the_cell_swap_is_all_or_nothing_under_a_refusing_spec() {
    use crate::container::{SlotFilter, SlotSpec};
    let fuel_only = SlotSpec {
        accepts: vec![SlotFilter::Tag(crate::item::ItemTag::FUEL)],
        take_only: false,
        accepts_bind: None,
    };
    let take_only = SlotSpec {
        accepts: Vec::new(),
        take_only: true,
        accepts_bind: None,
    };
    let coal = ItemType::by_key("petramond:coal").expect("engine item");

    let mut inv = empty_inv();
    *inv.off_hand_mut() = Some(item(ItemType::Stone, 1));
    let mut cell = Some(item(coal, 3));
    assert!(!inv.swap_off_hand_with_cell(Some(&fuel_only), None, &mut cell));
    assert_eq!(cell.map(|s| s.count), Some(3), "the cell keeps its stack");
    assert_eq!(inv.off_hand().map(|s| s.item), Some(ItemType::Stone));

    *inv.off_hand_mut() = Some(item(coal, 2));
    assert!(inv.swap_off_hand_with_cell(Some(&fuel_only), None, &mut cell));
    assert_eq!(cell.map(|s| s.count), Some(2));
    assert_eq!(inv.off_hand().map(|s| s.count), Some(3));

    let mut output = Some(item(coal, 4));
    assert!(!inv.swap_off_hand_with_cell(Some(&take_only), None, &mut output));
    assert_eq!(output.map(|s| s.count), Some(4));
    *inv.off_hand_mut() = None;
    assert!(inv.swap_off_hand_with_cell(Some(&take_only), None, &mut output));
    assert!(output.is_none());
    assert_eq!(inv.off_hand().map(|s| s.count), Some(4));
}

#[test]
fn pickup_tops_up_a_matching_off_hand_first_and_never_a_foreign_one() {
    let mut inv = empty_inv();
    *inv.off_hand_mut() = Some(item(ItemType::Dirt, 60));
    assert_eq!(inv.pickup_fits_count(item(ItemType::Dirt, 10)), 10);
    assert!(inv.pickup(item(ItemType::Dirt, 10)).is_none());
    assert_eq!(
        inv.off_hand().map(|s| s.count),
        Some(64),
        "the off-hand tops up first"
    );
    assert_eq!(
        inv.slots
            .iter()
            .flatten()
            .map(|s| s.count as u32)
            .sum::<u32>(),
        6,
        "the remainder routes through the ordinary insertion"
    );

    let mut inv = empty_inv();
    *inv.off_hand_mut() = Some(item(ItemType::Dirt, 1));
    assert!(inv.pickup(item(ItemType::Stone, 3)).is_none());
    assert_eq!(inv.off_hand().map(|s| s.count), Some(1));

    let mut inv = empty_inv();
    assert!(inv.pickup(item(ItemType::Dirt, 3)).is_none());
    assert!(inv.off_hand().is_none());

    let mut inv = empty_inv();
    for slot in inv.slots.iter_mut() {
        *slot = Some(item(ItemType::Stone, 64));
    }
    *inv.off_hand_mut() = Some(item(ItemType::Dirt, 62));
    assert_eq!(inv.pickup_fits_count(item(ItemType::Dirt, 10)), 2);
    assert_eq!(
        inv.pickup(item(ItemType::Dirt, 2)),
        None,
        "the planned amount fits exactly"
    );
    assert_eq!(inv.off_hand().map(|s| s.count), Some(64));
}

#[test]
fn held_in_and_decrement_resolve_per_hand() {
    let mut inv = empty_inv();
    inv.slots[0] = Some(item(ItemType::Dirt, 2));
    *inv.off_hand_mut() = Some(item(ItemType::Stone, 1));

    assert_eq!(inv.held_in(Hand::Main).unwrap().item, ItemType::Dirt);
    assert_eq!(inv.held_in(Hand::Off).unwrap().item, ItemType::Stone);

    inv.decrement_held(Hand::Off);
    assert!(inv.held_in(Hand::Off).is_none(), "the last one empties");
    assert_eq!(
        inv.held_in(Hand::Main).map(|s| s.count),
        Some(2),
        "the other hand is untouched"
    );
}

#[test]
fn replace_held_one_swaps_the_off_hand_in_place() {
    let mut inv = empty_inv();
    *inv.off_hand_mut() = Some(item(ItemType::Dirt, 1));
    assert!(inv.replace_held_one(Hand::Off, item(ItemType::Stone, 1)));
    assert_eq!(inv.off_hand().map(|s| s.item), Some(ItemType::Stone));

    *inv.off_hand_mut() = Some(item(ItemType::Dirt, 3));
    assert!(inv.replace_held_one(Hand::Off, item(ItemType::Grass, 1)));
    assert_eq!(inv.off_hand().map(|s| s.count), Some(2));
    assert_eq!(
        inv.slots
            .iter()
            .flatten()
            .find(|s| s.item == ItemType::Grass)
            .map(|s| s.count),
        Some(1)
    );

    *inv.off_hand_mut() = None;
    assert!(!inv.replace_held_one(Hand::Off, item(ItemType::Stone, 1)));
}

#[test]
fn gather_sweeps_the_off_hand_and_shift_ships_it_to_the_grid() {
    let mut inv = empty_inv();
    inv.slots[3] = Some(item(ItemType::Dirt, 4));
    *inv.off_hand_mut() = Some(item(ItemType::Dirt, 6));
    *inv.cursor_mut() = Some(item(ItemType::Dirt, 1));
    inv.collect_to_cursor();
    assert_eq!(
        inv.cursor().map(|s| s.count),
        Some(11),
        "the double-click gather sweeps the off-hand too"
    );
    assert!(inv.off_hand().is_none());

    *inv.cursor_mut() = None;
    *inv.off_hand_mut() = Some(item(ItemType::Stone, 9));
    inv.shift_move_off_hand();
    assert!(inv.off_hand().is_none());
    assert_eq!(
        inv.slots
            .iter()
            .flatten()
            .map(|s| s.count as u32)
            .sum::<u32>(),
        9
    );

    let mut inv = empty_inv();
    assert!(inv.add(item(ItemType::Dirt, 1)).is_none());
    assert!(inv.off_hand().is_none());
}
