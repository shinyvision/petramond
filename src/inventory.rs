use crate::item::ItemStack;

mod slots;
#[cfg(test)]
mod tests;

pub(crate) use slots::{
    insert_into_slots, merge_stack, place_cursor_count, plan_drag_distribution, slot_capacity,
    take_slot_stack,
};

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
        self.cursor?;
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
    /// Click a take-only output. Primary takes the whole output when it fits;
    /// secondary takes half onto an empty cursor, or one onto a compatible
    /// cursor. The held cursor stack is never placed into the output cell.
    pub(crate) fn click_take_only_external_slot(
        &mut self,
        slot: &mut Option<ItemStack>,
        secondary: bool,
    ) {
        self.bump_revision();
        Self::apply_take_only_click(&mut self.cursor, slot, secondary);
    }

    fn apply_take_only_click(
        cursor: &mut Option<ItemStack>,
        slot: &mut Option<ItemStack>,
        secondary: bool,
    ) {
        let Some(mut output) = slot.take() else {
            return;
        };
        let moved = match cursor.as_ref() {
            None if secondary => output.count - output.count / 2,
            None => output.count,
            Some(held) if held.can_stack_with(&output) => {
                if secondary {
                    held.space_left().min(1)
                } else if held.space_left() >= output.count {
                    output.count
                } else {
                    0
                }
            }
            Some(_) => 0,
        };
        if moved == 0 {
            *slot = Some(output);
            return;
        }
        match cursor {
            None => *cursor = Some(ItemStack::new(output.item, moved)),
            Some(held) => held.count += moved,
        }
        output.count -= moved;
        *slot = (output.count > 0).then_some(output);
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
    pub(crate) fn place_cursor_count_in_slot(&mut self, i: usize, wanted: u8) -> u8 {
        if i >= TOTAL_SLOTS || wanted == 0 {
            return 0;
        }
        self.bump_revision();
        place_cursor_count(&mut self.cursor, &mut self.slots[i], wanted)
    }
    pub(crate) fn place_cursor_count_in_external_slot(
        &mut self,
        slot: &mut Option<ItemStack>,
        wanted: u8,
    ) -> u8 {
        if wanted == 0 {
            return 0;
        }
        self.bump_revision();
        place_cursor_count(&mut self.cursor, slot, wanted)
    }
    pub(crate) fn take_slot_for_drop(&mut self, i: usize, all: bool) -> Option<ItemStack> {
        if i >= TOTAL_SLOTS || self.slots[i].is_none() {
            return None;
        }
        self.bump_revision();
        take_slot_stack(&mut self.slots[i], all)
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

    /// Atomically decrement the planned quantities from concrete inventory
    /// slots. The crafting planner computes this against the same inventory
    /// borrow immediately before commit; validation here keeps the mutation
    /// all-or-nothing if a future caller ever hands in a stale plan.
    pub(crate) fn consume_slots(&mut self, takes: &[(usize, u8)]) -> bool {
        let mut totals = [0u16; TOTAL_SLOTS];
        for &(slot, count) in takes {
            let Some(total) = totals.get_mut(slot) else {
                return false;
            };
            if count == 0 {
                return false;
            }
            let Some(next) = total.checked_add(u16::from(count)) else {
                return false;
            };
            *total = next;
        }
        if totals.iter().enumerate().any(|(slot, &count)| {
            count > 0
                && self.slots[slot]
                    .as_ref()
                    .is_none_or(|stack| u16::from(stack.count) < count)
        }) {
            return false;
        }
        if takes.is_empty() {
            return true;
        }
        for (slot, count) in totals.into_iter().enumerate() {
            if count == 0 {
                continue;
            }
            let stack = self.slots[slot].as_mut().expect("plan validated above");
            stack.count -= count as u8;
            if stack.count == 0 {
                self.slots[slot] = None;
            }
        }
        self.bump_revision();
        true
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
