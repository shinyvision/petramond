use crate::item::ItemStack;
pub(crate) fn insert_into_slots(
    slots: &mut [Option<ItemStack>],
    mut stack: ItemStack,
) -> Option<ItemStack> {
    if stack.is_empty() {
        return None;
    }

    // Pass 1: top up existing matching, non-full stacks in slot order.
    for existing in slots.iter_mut().flatten() {
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
    for slot in slots.iter_mut() {
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

/// Move `stack` onto the cursor if it fits: an empty cursor takes it whole; a
/// matching cursor stack absorbs it when there is room. `false` = untouched
/// (a full or mismatched cursor) — take-only output clicks gate on this.
pub(crate) fn stack_onto_cursor(cursor: &mut Option<ItemStack>, stack: ItemStack) -> bool {
    match cursor {
        None => {
            *cursor = Some(stack);
            true
        }
        Some(cur) if cur.can_stack_with(&stack) && cur.space_left() >= stack.count => {
            cur.count += stack.count;
            true
        }
        _ => false,
    }
}

/// Merge `src` into `dst`: fill an empty `dst`, top up a matching stack to
/// its cap (leaving the remainder in `src`), and leave both untouched on a
/// mismatch — the unit move behind every shift-route into container slots.
pub(crate) fn merge_stack(src: &mut Option<ItemStack>, dst: &mut Option<ItemStack>) {
    let Some(mut incoming) = src.take() else {
        return;
    };
    match dst {
        None => *dst = Some(incoming),
        Some(existing) if existing.can_stack_with(&incoming) => {
            let moved = existing.space_left().min(incoming.count);
            existing.count += moved;
            incoming.count -= moved;
            *src = (incoming.count > 0).then_some(incoming);
        }
        Some(_) => *src = Some(incoming),
    }
}
pub const HOTBAR_LEN: usize = 9;
pub const MAIN_LEN: usize = 27;
pub const TOTAL_SLOTS: usize = HOTBAR_LEN + MAIN_LEN; // 36
#[derive(Clone, Debug)]
pub struct Inventory {
    slots: [Option<ItemStack>; TOTAL_SLOTS],
    cursor: Option<ItemStack>,
    active: u8,
    /// Mutation counter for replication: bumped by every mutating public
    /// method (conservatively — a mutable borrow via [`slot_mut`]/
    /// [`cursor_mut`] bumps at borrow time). The server includes the full
    /// inventory in a `SelfState` only when this moved, so a spurious bump
    /// costs one redundant send, never a stale client.
    ///
    /// [`slot_mut`]: Self::slot_mut
    /// [`cursor_mut`]: Self::cursor_mut
    revision: u64,
}

impl Default for Inventory {
    fn default() -> Self {
        Self {
            slots: [None; TOTAL_SLOTS],
            cursor: None,
            active: 0,
            revision: 0,
        }
    }
}

impl Inventory {
    pub fn new() -> Self {
        Self::default()
    }
    /// The mutation counter (see the field docs). Replication compares this
    /// against the last value it shipped.
    #[inline]
    pub fn revision(&self) -> u64 {
        self.revision
    }
    /// Mark the inventory changed. Public for callers that mutate through a
    /// long-lived reference and can't rely on the borrow-time bump.
    #[inline]
    pub fn bump_revision(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }
    #[inline]
    pub fn slot(&self, i: usize) -> Option<&ItemStack> {
        self.slots.get(i).and_then(Option::as_ref)
    }
    #[inline]
    pub fn slot_mut(&mut self, i: usize) -> Option<&mut Option<ItemStack>> {
        // Conservative: assume the borrower mutates.
        self.bump_revision();
        self.slots.get_mut(i)
    }
    #[inline]
    pub fn active_slot(&self) -> u8 {
        self.active
    }
    #[inline]
    pub fn set_active(&mut self, i: u8) {
        let next = i.min(HOTBAR_LEN as u8 - 1);
        if next != self.active {
            self.active = next;
            self.bump_revision();
        }
    }
    pub fn scroll_active(&mut self, delta: i32) {
        let len = HOTBAR_LEN as i32;
        // rem_euclid keeps the result in 0..len for any sign / magnitude.
        let next = (self.active as i32 + delta).rem_euclid(len) as u8;
        if next != self.active {
            self.active = next;
            self.bump_revision();
        }
    }
    #[inline]
    pub fn selected(&self) -> Option<&ItemStack> {
        self.slot(self.active as usize)
    }
    pub fn add(&mut self, stack: ItemStack) -> Option<ItemStack> {
        // The whole inventory in slot order: hotbar `[0, 9)` then main `[9, 36)`.
        self.add_to_range(stack, 0, TOTAL_SLOTS)
    }
    fn add_to_range(&mut self, stack: ItemStack, start: usize, end: usize) -> Option<ItemStack> {
        if !stack.is_empty() {
            self.bump_revision();
        }
        insert_into_slots(&mut self.slots[start..end], stack)
    }
    pub fn pull_from(&mut self, slot: &mut Option<ItemStack>) {
        if let Some(stack) = slot.take() {
            *slot = self.add(stack);
        }
    }
    pub fn decrement_selected(&mut self) {
        let i = self.active as usize;
        if let Some(stack) = self.slots[i].as_mut() {
            stack.count = stack.count.saturating_sub(1);
            if stack.count == 0 {
                self.slots[i] = None;
            }
            self.bump_revision();
        }
    }
    /// Swap ONE of the selected stack for `replacement` — a bucket filling or
    /// emptying in the hand. A single item swaps in place (keeping its slot);
    /// one of a larger stack converts, with the replacement going to any open
    /// slot. Refuses (returning `false`, changing nothing) when the selected
    /// slot is empty or the replacement has nowhere to go — all-or-nothing, so
    /// the world mutation it accompanies can be gated on it.
    pub fn replace_selected_one(&mut self, replacement: ItemStack) -> bool {
        let i = self.active as usize;
        if self.slots[i].is_none() {
            return false;
        }
        self.bump_revision();
        let stack = self.slots[i].as_mut().expect("checked above");
        if stack.count <= 1 {
            self.slots[i] = Some(replacement);
            return true;
        }
        stack.count -= 1;
        if self.add(replacement).is_some() {
            // No room for the replacement anywhere: restore and refuse.
            if let Some(stack) = self.slots[i].as_mut() {
                stack.count += 1;
            }
            return false;
        }
        true
    }
    #[inline]
    pub fn cursor(&self) -> Option<&ItemStack> {
        self.cursor.as_ref()
    }
    #[inline]
    pub fn cursor_mut(&mut self) -> &mut Option<ItemStack> {
        // Conservative: assume the borrower mutates.
        self.bump_revision();
        &mut self.cursor
    }
    pub fn take_cursor(&mut self) -> Option<ItemStack> {
        if self.cursor.is_some() {
            self.bump_revision();
        }
        self.cursor.take()
    }
    pub fn stash_cursor_in_inventory(&mut self) -> Option<ItemStack> {
        if self.cursor.is_none() {
            return None;
        }
        self.bump_revision();
        let stack = self.cursor.take()?;
        if stack.is_empty() {
            return None;
        }
        self.add(stack)
    }
    pub fn click_slot(&mut self, i: usize) {
        if i >= TOTAL_SLOTS {
            return;
        }
        self.bump_revision();
        Self::apply_left_click(&mut self.cursor, &mut self.slots[i]);
    }
    pub fn click_external_slot(&mut self, slot: &mut Option<ItemStack>) {
        self.bump_revision();
        Self::apply_left_click(&mut self.cursor, slot);
    }
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
    pub fn collect_to_cursor(&mut self) {
        let Some(mut cursor) = self.cursor.take() else {
            return;
        };
        self.bump_revision();
        // Two passes so loose partials are merged before any full stack is split:
        // pass 1 skips full stacks, pass 2 (only reached if room remains) takes
        // from them too.
        for take_full in [false, true] {
            Self::drain_into(&mut cursor, &mut self.slots, take_full);
        }
        self.cursor = Some(cursor);
    }
    pub fn collect_to_cursor_including(&mut self, extra: &mut [Option<ItemStack>]) {
        let Some(mut cursor) = self.cursor.take() else {
            return;
        };
        self.bump_revision();
        for take_full in [false, true] {
            Self::drain_into(&mut cursor, extra, take_full);
            Self::drain_into(&mut cursor, &mut self.slots, take_full);
        }
        self.cursor = Some(cursor);
    }
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
    pub fn right_click_slot(&mut self, i: usize) {
        if i >= TOTAL_SLOTS {
            return;
        }
        self.bump_revision();
        Self::apply_right_click(&mut self.cursor, &mut self.slots[i]);
    }
    pub fn right_click_external_slot(&mut self, slot: &mut Option<ItemStack>) {
        self.bump_revision();
        Self::apply_right_click(&mut self.cursor, slot);
    }
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
    pub fn shift_move_slot(&mut self, i: usize) {
        if i >= TOTAL_SLOTS {
            return;
        }
        let Some(stack) = self.slots[i].take() else {
            return;
        };
        self.bump_revision();
        // Hotbar `[0, 9)` ships to the main grid; the main grid ships to the hotbar.
        let (start, end) = if i < HOTBAR_LEN {
            (HOTBAR_LEN, TOTAL_SLOTS)
        } else {
            (0, HOTBAR_LEN)
        };
        // Whatever doesn't fit in the destination region stays in the source slot.
        self.slots[i] = self.add_to_range(stack, start, end);
    }
    pub fn raw_slots(&self) -> &[Option<ItemStack>; TOTAL_SLOTS] {
        &self.slots
    }
    pub fn from_parts(
        slots: [Option<ItemStack>; TOTAL_SLOTS],
        cursor: Option<ItemStack>,
        active: u8,
    ) -> Self {
        Self {
            slots,
            cursor,
            active: active.min(HOTBAR_LEN as u8 - 1),
            revision: 0,
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
}
