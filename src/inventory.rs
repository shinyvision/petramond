//! Player inventory: a fixed 36-slot store with a 9-slot hotbar, a 27-slot
//! main grid, a single cursor-held stack (for drag/drop UI), and an active
//! hotbar selection.
//!
//! Layout matches the classic survival layout: slots `[0, 9)` are the hotbar
//! (the bottom row in the open inventory) and `[9, 36)` are the 3×9 main grid.
//! The active slot is always a hotbar index (`0..9`) and drives what the player
//! holds / places.
//!
//! Storage is a fixed `[Option<ItemStack>; 36]` array — no heap allocation per
//! call. `ItemStack` is `Copy`, so all moves are cheap value moves.

use crate::block::Block;
use crate::item::{ItemStack, ItemType};

/// Number of hotbar slots (the always-visible bottom row).
pub const HOTBAR_LEN: usize = 9;
/// Number of main-grid slots (the 3×9 grid shown when the inventory is open).
pub const MAIN_LEN: usize = 27;
/// Total slot count: hotbar `[0, 9)` + main grid `[9, 36)`.
pub const TOTAL_SLOTS: usize = HOTBAR_LEN + MAIN_LEN; // 36

/// The blocks placed into the hotbar by [`Inventory::new`]'s demo starter set.
///
/// Nine placeable blocks, one stack of 64 each. `Poppy` and `Fern` are
/// cross-plant blocks, included so the flat-sprite item path (slots / held
/// billboard) is exercised out of the box.
const DEMO_HOTBAR: [Block; HOTBAR_LEN] = [
    Block::Grass,
    Block::Dirt,
    Block::Stone,
    Block::OakLog,
    Block::OakLeaves,
    Block::Sand,
    Block::Gravel,
    Block::Poppy,
    Block::Fern,
];

/// A 36-slot inventory with a cursor-held stack and an active hotbar slot.
///
/// `slots` is a fixed array: `[0, HOTBAR_LEN)` is the hotbar, the rest is the
/// main grid. `cursor` is the stack currently "picked up" by drag/drop UI.
/// `active` is the selected hotbar index (`0..HOTBAR_LEN`).
#[derive(Clone, Debug)]
pub struct Inventory {
    slots: [Option<ItemStack>; TOTAL_SLOTS],
    cursor: Option<ItemStack>,
    active: u8,
}

impl Default for Inventory {
    fn default() -> Self {
        Self::new()
    }
}

impl Inventory {
    /// A fresh inventory with the demo starter set: the nine hotbar slots filled
    /// with a stack of 64 of each [`DEMO_HOTBAR`] block, main grid empty, no
    /// cursor stack, active slot `0`.
    pub fn new() -> Self {
        let mut slots: [Option<ItemStack>; TOTAL_SLOTS] = [None; TOTAL_SLOTS];
        for (slot, &block) in slots.iter_mut().zip(DEMO_HOTBAR.iter()) {
            *slot = Some(ItemStack::new(ItemType::from_block(block), 64));
        }
        Inventory {
            slots,
            cursor: None,
            active: 0,
        }
    }

    /// The stack in slot `i` (`0..TOTAL_SLOTS`), or `None` if empty / out of range.
    #[inline]
    pub fn slot(&self, i: usize) -> Option<&ItemStack> {
        self.slots.get(i).and_then(Option::as_ref)
    }

    /// The stack in hotbar slot `i` (`0..HOTBAR_LEN`). Identical to `slot(i)`.
    #[inline]
    pub fn hotbar(&self, i: usize) -> Option<&ItemStack> {
        if i < HOTBAR_LEN {
            self.slot(i)
        } else {
            None
        }
    }

    /// The active (selected) hotbar slot index, always in `0..HOTBAR_LEN`.
    #[inline]
    pub fn active_slot(&self) -> u8 {
        self.active
    }

    /// Set the active hotbar slot, clamped to `0..HOTBAR_LEN`.
    #[inline]
    pub fn set_active(&mut self, i: u8) {
        self.active = i.min(HOTBAR_LEN as u8 - 1);
    }

    /// Move the active hotbar slot by `delta`, wrapping within `0..HOTBAR_LEN`.
    ///
    /// Positive `delta` moves right; negative moves left. Any magnitude is
    /// reduced modulo `HOTBAR_LEN`.
    pub fn scroll_active(&mut self, delta: i32) {
        let len = HOTBAR_LEN as i32;
        // rem_euclid keeps the result in 0..len for any sign / magnitude.
        let next = (self.active as i32 + delta).rem_euclid(len);
        self.active = next as u8;
    }

    /// The stack in the active hotbar slot (what the player currently holds).
    #[inline]
    pub fn selected(&self) -> Option<&ItemStack> {
        self.slot(self.active as usize)
    }

    /// Insert `stack`, merging into existing non-full matching stacks first
    /// (hotbar then main grid, in slot order) and then into the first empty
    /// slot, respecting each item's `max_stack_size`.
    ///
    /// Returns the leftover (`Some` only if every matching/empty slot filled up
    /// before `stack` was exhausted), or `None` if it was fully absorbed. An
    /// empty input stack is a no-op returning `None`.
    pub fn add(&mut self, mut stack: ItemStack) -> Option<ItemStack> {
        if stack.is_empty() {
            return None;
        }

        // Pass 1: top up existing matching, non-full stacks in slot order.
        for existing in self.slots.iter_mut().flatten() {
            if existing.can_stack_with(&stack) {
                let space = existing.space_left();
                if space > 0 {
                    let moved = space.min(stack.count);
                    existing.count += moved;
                    stack.count -= moved;
                    if stack.count == 0 {
                        return None;
                    }
                }
            }
        }

        // Pass 2: drop the remainder into empty slots, one full stack at a time.
        for slot in self.slots.iter_mut() {
            if slot.is_none() {
                let put = stack.count.min(stack.item.max_stack_size());
                *slot = Some(ItemStack::new(stack.item, put));
                stack.count -= put;
                if stack.count == 0 {
                    return None;
                }
            }
        }

        Some(stack)
    }

    /// Remove one item from the active hotbar slot (e.g. after placing a block).
    /// Clears the slot when it reaches zero. No-op if the slot is empty.
    pub fn decrement_selected(&mut self) {
        let i = self.active as usize;
        if let Some(stack) = self.slots[i].as_mut() {
            stack.count = stack.count.saturating_sub(1);
            if stack.count == 0 {
                self.slots[i] = None;
            }
        }
    }

    /// The cursor-held stack (the stack currently being dragged in the UI).
    #[inline]
    pub fn cursor(&self) -> Option<&ItemStack> {
        self.cursor.as_ref()
    }

    /// Left-click drag/drop interaction on slot `i` (whole-stack semantics):
    ///  - cursor empty, slot full  → pick the whole slot up into the cursor
    ///  - cursor full,  slot empty → drop the cursor stack into the slot
    ///  - both full, same item     → merge cursor into slot up to max; any
    ///    remainder stays on the cursor
    ///  - both full, different item / slot already at max → swap cursor ↔ slot
    ///
    /// Out-of-range `i` and the both-empty case are no-ops.
    pub fn click_slot(&mut self, i: usize) {
        if i >= TOTAL_SLOTS {
            return;
        }

        match (self.cursor.take(), self.slots[i].take()) {
            // Both empty: nothing to do.
            (None, None) => {}

            // Cursor empty, slot full: pick the slot up.
            (None, Some(slot)) => {
                self.cursor = Some(slot);
            }

            // Cursor full, slot empty: drop into the slot.
            (Some(cur), None) => {
                self.slots[i] = Some(cur);
            }

            // Both full.
            (Some(mut cur), Some(mut slot)) => {
                if slot.can_stack_with(&cur) && slot.space_left() > 0 {
                    // Same item, slot has room: merge cursor into slot.
                    let moved = slot.space_left().min(cur.count);
                    slot.count += moved;
                    cur.count -= moved;
                    self.slots[i] = Some(slot);
                    // Keep any remainder on the cursor.
                    self.cursor = if cur.count > 0 { Some(cur) } else { None };
                } else {
                    // Different item, or slot full: swap.
                    self.slots[i] = Some(cur);
                    self.cursor = Some(slot);
                }
            }
        }
    }

    /// `true` if every slot and the cursor are empty.
    pub fn is_empty(&self) -> bool {
        self.cursor.is_none() && self.slots.iter().all(Option::is_none)
    }

    /// All slots in order (`[0,9)` hotbar, `[9,36)` main grid). For saving.
    pub fn raw_slots(&self) -> &[Option<ItemStack>; TOTAL_SLOTS] {
        &self.slots
    }

    /// Reconstruct an inventory from saved parts (`active` clamped to the hotbar).
    pub fn from_parts(
        slots: [Option<ItemStack>; TOTAL_SLOTS],
        cursor: Option<ItemStack>,
        active: u8,
    ) -> Self {
        Self {
            slots,
            cursor,
            active: active.min(HOTBAR_LEN as u8 - 1),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(t: ItemType, n: u8) -> ItemStack {
        ItemStack::new(t, n)
    }

    #[test]
    fn new_has_demo_hotbar() {
        let inv = Inventory::new();
        assert_eq!(inv.active_slot(), 0);
        assert!(inv.cursor().is_none());
        // Nine full hotbar stacks of the demo blocks.
        for (i, &block) in DEMO_HOTBAR.iter().enumerate() {
            let s = inv.hotbar(i).expect("demo hotbar slot filled");
            assert_eq!(s.item, ItemType::from_block(block));
            assert_eq!(s.count, 64);
        }
        // Main grid is empty.
        for i in HOTBAR_LEN..TOTAL_SLOTS {
            assert!(inv.slot(i).is_none(), "main slot {i} should be empty");
        }
        // Includes the two cross-plants for the sprite path.
        assert_eq!(inv.hotbar(7).unwrap().item, ItemType::Poppy);
        assert_eq!(inv.hotbar(8).unwrap().item, ItemType::Fern);
        assert!(!inv.is_empty());
    }

    #[test]
    fn selected_follows_active() {
        let mut inv = Inventory::new();
        assert_eq!(inv.selected().unwrap().item, ItemType::Grass);
        inv.set_active(2);
        assert_eq!(inv.selected().unwrap().item, ItemType::Stone);
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

    #[test]
    fn is_empty_reports_cursor_and_slots() {
        let mut inv = Inventory::new();
        for i in 0..TOTAL_SLOTS {
            inv.slots[i] = None;
        }
        assert!(inv.is_empty());
        inv.cursor = Some(item(ItemType::Dirt, 1));
        assert!(!inv.is_empty());
        inv.cursor = None;
        inv.slots[10] = Some(item(ItemType::Dirt, 1));
        assert!(!inv.is_empty());
    }
}
