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

use crate::item::ItemStack;

/// Number of hotbar slots (the always-visible bottom row).
pub const HOTBAR_LEN: usize = 9;
/// Number of main-grid slots (the 3×9 grid shown when the inventory is open).
pub const MAIN_LEN: usize = 27;
/// Total slot count: hotbar `[0, 9)` + main grid `[9, 36)`.
pub const TOTAL_SLOTS: usize = HOTBAR_LEN + MAIN_LEN; // 36

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
    /// A fresh, empty inventory: every slot empty, no cursor stack, active slot
    /// `0`. The player collects items by breaking blocks in the world.
    pub fn new() -> Self {
        Inventory {
            slots: [None; TOTAL_SLOTS],
            cursor: None,
            active: 0,
        }
    }

    /// The stack in slot `i` (`0..TOTAL_SLOTS`), or `None` if empty / out of range.
    #[inline]
    pub fn slot(&self, i: usize) -> Option<&ItemStack> {
        self.slots.get(i).and_then(Option::as_ref)
    }

    /// Mutable handle to slot `i`'s cell (`None` if out of range), for moving a
    /// stack directly between the inventory and an external slot such as a furnace
    /// slot. The cursor is not involved.
    #[inline]
    pub fn slot_mut(&mut self, i: usize) -> Option<&mut Option<ItemStack>> {
        self.slots.get_mut(i)
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
    pub fn add(&mut self, stack: ItemStack) -> Option<ItemStack> {
        // The whole inventory in slot order: hotbar `[0, 9)` then main `[9, 36)`.
        self.add_to_range(stack, 0, TOTAL_SLOTS)
    }

    /// Like [`add`](Self::add) but restricted to slots `[start, end)`: merge into
    /// matching non-full stacks first, then the first empty slot, both in
    /// ascending slot order (left-to-right, top-to-bottom). Returns the leftover,
    /// or `None` if fully absorbed. Empty input is a no-op returning `None`.
    /// Used by [`add`](Self::add) (whole range) and shift-click transfer (one
    /// region at a time).
    fn add_to_range(&mut self, mut stack: ItemStack, start: usize, end: usize) -> Option<ItemStack> {
        if stack.is_empty() {
            return None;
        }

        // Pass 1: top up existing matching, non-full stacks in slot order.
        for existing in self.slots[start..end].iter_mut().flatten() {
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
        for slot in self.slots[start..end].iter_mut() {
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

    /// Take a single item off the active hotbar slot (for the in-game drop key),
    /// shrinking it by one and clearing the slot when it empties. Returns a
    /// 1-count stack of the held item, or `None` if the slot is empty.
    pub fn take_selected_one(&mut self) -> Option<ItemStack> {
        let i = self.active as usize;
        let stack = self.slots[i].as_mut()?;
        let item = stack.item;
        stack.count -= 1;
        if stack.count == 0 {
            self.slots[i] = None;
        }
        Some(ItemStack::new(item, 1))
    }

    /// Take the entire active hotbar slot stack out (for the Ctrl+drop key),
    /// clearing the slot. Returns `None` if the slot is empty.
    pub fn take_selected_all(&mut self) -> Option<ItemStack> {
        self.slots[self.active as usize].take()
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

    /// Take the whole cursor-held stack out, clearing the cursor. Returns `None`
    /// if the cursor was empty. Used to throw the held stack into the world.
    pub fn take_cursor(&mut self) -> Option<ItemStack> {
        self.cursor.take()
    }

    /// Take a single item off the cursor-held stack, shrinking it by one (and
    /// clearing the cursor when the last item leaves). Returns a 1-count stack of
    /// the held item, or `None` if the cursor was empty. Used to throw one item
    /// out at a time.
    pub fn take_cursor_one(&mut self) -> Option<ItemStack> {
        let cur = self.cursor.as_mut()?;
        let item = cur.item;
        cur.count -= 1;
        if cur.count == 0 {
            self.cursor = None;
        }
        Some(ItemStack::new(item, 1))
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
        Self::apply_left_click(&mut self.cursor, &mut self.slots[i]);
    }

    /// Left-click drag/drop against an EXTERNAL slot (a crafting input cell that
    /// lives outside the inventory array). Same pick/drop/merge/swap semantics as
    /// [`click_slot`](Self::click_slot), but the slot is borrowed in.
    pub fn click_external_slot(&mut self, slot: &mut Option<ItemStack>) {
        Self::apply_left_click(&mut self.cursor, slot);
    }

    /// The shared left-click rule between the cursor and a single slot: pick a
    /// slot up, drop into an empty slot, merge same items, else swap.
    fn apply_left_click(cursor: &mut Option<ItemStack>, slot: &mut Option<ItemStack>) {
        match (cursor.take(), slot.take()) {
            (None, None) => {}
            (None, Some(s)) => *cursor = Some(s),
            (Some(c), None) => *slot = Some(c),
            (Some(mut c), Some(mut s)) => {
                if s.can_stack_with(&c) && s.space_left() > 0 {
                    let moved = s.space_left().min(c.count);
                    s.count += moved;
                    c.count -= moved;
                    *slot = Some(s);
                    *cursor = (c.count > 0).then_some(c);
                } else {
                    *slot = Some(c);
                    *cursor = Some(s);
                }
            }
        }
    }

    /// Double-click "collect all": with a stack held on the cursor, pull matching
    /// items out of every slot into the cursor until it reaches the item's max
    /// stack size. Loose items are consolidated first — pass 1 drains only partial
    /// (non-full) stacks, and only if the cursor still has room does pass 2 break
    /// into full stacks. Within each pass slots are visited in order (hotbar
    /// `[0, 9)` then main grid `[9, 36)`); emptied slots are cleared. No-op when
    /// the cursor is empty or already full.
    ///
    /// This is the fast-double-click gather: the first click picks a stack up onto
    /// the cursor (see [`click_slot`](Self::click_slot)), and a quick second click
    /// on the same slot calls this instead of dropping the stack back down.
    pub fn collect_to_cursor(&mut self) {
        let Some(mut cursor) = self.cursor.take() else {
            return;
        };
        // Two passes so loose partials are merged before any full stack is split:
        // pass 1 skips full stacks, pass 2 (only reached if room remains) takes
        // from them too.
        for take_full in [false, true] {
            Self::drain_into(&mut cursor, &mut self.slots, take_full);
        }
        self.cursor = Some(cursor);
    }

    /// Like [`collect_to_cursor`](Self::collect_to_cursor) but also pulls from
    /// `extra` — an open container's slots (a chest) — so a double-click in the
    /// chest screen tops the cursor up from BOTH the chest and the inventory.
    /// Partials are drained everywhere before any full stack is split: each pass
    /// visits `extra` first (the open container the player clicked in) then the
    /// inventory. No-op when the cursor is empty.
    pub fn collect_to_cursor_including(&mut self, extra: &mut [Option<ItemStack>]) {
        let Some(mut cursor) = self.cursor.take() else {
            return;
        };
        for take_full in [false, true] {
            Self::drain_into(&mut cursor, extra, take_full);
            Self::drain_into(&mut cursor, &mut self.slots, take_full);
        }
        self.cursor = Some(cursor);
    }

    /// One gather pass for the double-click collect: pull matching items from
    /// `slots` into `cursor` in slot order, clearing emptied slots and stopping once
    /// the cursor is full. When `take_full` is false, only partial (non-max) stacks
    /// are drained, so loose items consolidate before any full stack is split.
    fn drain_into(cursor: &mut ItemStack, slots: &mut [Option<ItemStack>], take_full: bool) {
        for slot in slots.iter_mut() {
            let space = cursor.space_left();
            if space == 0 {
                return;
            }
            let Some(existing) = slot.as_mut() else {
                continue;
            };
            if !existing.can_stack_with(cursor) {
                continue;
            }
            if !take_full && existing.count >= existing.item.max_stack_size() {
                continue;
            }
            let moved = space.min(existing.count);
            cursor.count += moved;
            existing.count -= moved;
            if existing.count == 0 {
                *slot = None;
            }
        }
    }

    /// Right-click drag/drop interaction on slot `i`:
    ///  - cursor empty, slot has a stack → split it: the larger half (`ceil`)
    ///    goes onto the cursor, the rest stays (a 5-stack leaves 2, drags 3)
    ///  - cursor full, slot empty → drop ONE item into the slot
    ///  - cursor full, slot same item with room → add ONE to the slot
    ///  - cursor full, slot different item / already at max → no-op
    ///
    /// Out-of-range `i` and the both-empty case are no-ops.
    pub fn right_click_slot(&mut self, i: usize) {
        if i >= TOTAL_SLOTS {
            return;
        }
        Self::apply_right_click(&mut self.cursor, &mut self.slots[i]);
    }

    /// Right-click drag/drop against an EXTERNAL slot (a crafting input cell).
    /// Same split / place-one / drip-one semantics as
    /// [`right_click_slot`](Self::right_click_slot).
    pub fn right_click_external_slot(&mut self, slot: &mut Option<ItemStack>) {
        Self::apply_right_click(&mut self.cursor, slot);
    }

    /// The shared right-click rule: split a slot in half onto the cursor, place
    /// one held item into an empty/matching slot, else no-op.
    fn apply_right_click(cursor: &mut Option<ItemStack>, slot: &mut Option<ItemStack>) {
        match (cursor.take(), slot.take()) {
            (None, None) => {}
            (None, Some(mut s)) => {
                // ceil(count / 2): the dragged half is the larger one.
                let take = s.count - s.count / 2;
                let item = s.item;
                s.count -= take;
                *cursor = Some(ItemStack::new(item, take));
                *slot = (s.count > 0).then_some(s);
            }
            (Some(mut c), None) => {
                *slot = Some(ItemStack::new(c.item, 1));
                c.count -= 1;
                *cursor = (c.count > 0).then_some(c);
            }
            (Some(mut c), Some(mut s)) => {
                if s.can_stack_with(&c) && s.space_left() > 0 {
                    s.count += 1;
                    c.count -= 1;
                    *slot = Some(s);
                    *cursor = (c.count > 0).then_some(c);
                } else {
                    *slot = Some(s);
                    *cursor = Some(c);
                }
            }
        }
    }

    /// Whether `stack` would fit ENTIRELY into the inventory (matching partial
    /// stacks first, then empty slots). Used by shift-crafting to refuse a craft
    /// that wouldn't fully fit. An empty stack always "fits".
    pub fn can_add(&self, stack: ItemStack) -> bool {
        if stack.is_empty() {
            return true;
        }
        let mut need = stack.count as u32;
        for s in self.slots.iter().flatten() {
            if s.can_stack_with(&stack) {
                need = need.saturating_sub(s.space_left() as u32);
            }
        }
        if need == 0 {
            return true;
        }
        let empties = self.slots.iter().filter(|s| s.is_none()).count() as u32;
        need <= empties * stack.item.max_stack_size() as u32
    }

    /// How many of `stack` would actually be absorbed if added now — summing the
    /// space in matching partial stacks and empty slots, capped at `stack.count`.
    /// `0` means none fit, `stack.count` means the whole stack fits. Used by the
    /// pickup path to peel off exactly the portion that fits a near-full inventory.
    pub fn fits_count(&self, stack: ItemStack) -> u8 {
        if stack.is_empty() {
            return 0;
        }
        let want = stack.count as u32;
        let cap = stack.item.max_stack_size() as u32;
        let mut room: u32 = 0;
        for slot in &self.slots {
            room += match slot {
                None => cap,
                Some(existing) if existing.can_stack_with(&stack) => existing.space_left() as u32,
                Some(_) => 0,
            };
            if room >= want {
                return stack.count;
            }
        }
        room.min(want) as u8
    }

    /// Place a crafted `stack` onto the cursor: succeeds when the cursor is empty
    /// (it becomes `stack`) or holds the same item with room for the WHOLE stack.
    /// Returns whether it was placed — i.e. whether the craft may proceed.
    pub fn try_stack_onto_cursor(&mut self, stack: ItemStack) -> bool {
        match &mut self.cursor {
            None => {
                self.cursor = Some(stack);
                true
            }
            Some(cur) if cur.can_stack_with(&stack) && cur.space_left() >= stack.count => {
                cur.count += stack.count;
                true
            }
            _ => false,
        }
    }

    /// Shift-click transfer: move the whole stack in slot `i` to the OTHER region
    /// — a hotbar stack goes to the main grid and a main-grid stack to the hotbar
    /// — merging into matching stacks first then the first empty slot, filling in
    /// ascending slot order (left-to-right, top-to-bottom). Any part that doesn't
    /// fit stays behind; if nothing fits the click is effectively ignored. No-op
    /// on an empty or out-of-range slot. The cursor is not involved.
    pub fn shift_move_slot(&mut self, i: usize) {
        if i >= TOTAL_SLOTS {
            return;
        }
        let Some(stack) = self.slots[i].take() else {
            return;
        };
        // Hotbar `[0, 9)` ships to the main grid; the main grid ships to the hotbar.
        let (start, end) = if i < HOTBAR_LEN {
            (HOTBAR_LEN, TOTAL_SLOTS)
        } else {
            (0, HOTBAR_LEN)
        };
        // Whatever doesn't fit in the destination region stays in the source slot.
        self.slots[i] = self.add_to_range(stack, start, end);
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
        assert!(inv.is_empty());
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
        assert_eq!(inv.slot(HOTBAR_LEN).unwrap().count, 11, "last source keeps the remainder");
        assert_eq!(inv.slot(2), Some(&item(ItemType::Stone, 30)), "other items untouched");
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
        assert_eq!(inv.slot(0).unwrap().count, 6, "full stack broken only for the remainder");
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
        assert_eq!(inv.cursor(), Some(&item(ItemType::Dirt, 5)), "nothing to gather");
        assert_eq!(inv.slot(0), Some(&item(ItemType::Stone, 64)));
        assert_eq!(inv.slot(1), Some(&item(ItemType::Sand, 30)));
    }

    #[test]
    fn take_cursor_all_and_one() {
        let mut inv = empty_inv();
        inv.cursor = Some(item(ItemType::Dirt, 3));
        assert_eq!(inv.take_cursor_one(), Some(item(ItemType::Dirt, 1)));
        assert_eq!(inv.cursor(), Some(&item(ItemType::Dirt, 2)));
        assert_eq!(inv.take_cursor(), Some(item(ItemType::Dirt, 2)));
        assert!(inv.cursor().is_none());
        // Empty cursor: both are None.
        assert!(inv.take_cursor().is_none());
        assert!(inv.take_cursor_one().is_none());
        // Taking the last item clears the cursor.
        inv.cursor = Some(item(ItemType::Stone, 1));
        assert_eq!(inv.take_cursor_one(), Some(item(ItemType::Stone, 1)));
        assert!(inv.cursor().is_none());
    }

    #[test]
    fn take_selected_one_and_all_from_active_slot() {
        let mut inv = empty_inv();
        inv.slots[2] = Some(item(ItemType::Stone, 3));
        inv.set_active(2);
        assert_eq!(inv.take_selected_one(), Some(item(ItemType::Stone, 1)));
        assert_eq!(inv.slot(2), Some(&item(ItemType::Stone, 2)));
        assert_eq!(inv.take_selected_all(), Some(item(ItemType::Stone, 2)));
        assert!(inv.slot(2).is_none());
        // Empty active slot: both return None.
        assert!(inv.take_selected_one().is_none());
        assert!(inv.take_selected_all().is_none());
    }

    #[test]
    fn take_selected_one_clears_slot_at_zero() {
        let mut inv = empty_inv();
        inv.slots[0] = Some(item(ItemType::Dirt, 1));
        inv.set_active(0);
        assert_eq!(inv.take_selected_one(), Some(item(ItemType::Dirt, 1)));
        assert!(inv.slot(0).is_none());
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

    #[test]
    fn try_stack_onto_cursor_rules() {
        let mut inv = empty_inv();
        // Empty cursor takes the stack.
        assert!(inv.try_stack_onto_cursor(item(ItemType::OakPlanks, 4)));
        assert_eq!(inv.cursor(), Some(&item(ItemType::OakPlanks, 4)));
        // Same item with room stacks on.
        assert!(inv.try_stack_onto_cursor(item(ItemType::OakPlanks, 4)));
        assert_eq!(inv.cursor().unwrap().count, 8);
        // A different item is refused.
        assert!(!inv.try_stack_onto_cursor(item(ItemType::Stick, 1)));
        // No room for the WHOLE batch is refused (62 + 4 > 64).
        inv.cursor = Some(item(ItemType::OakPlanks, 62));
        assert!(!inv.try_stack_onto_cursor(item(ItemType::OakPlanks, 4)));
        assert_eq!(inv.cursor().unwrap().count, 62);
    }
}
