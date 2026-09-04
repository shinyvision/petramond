use crate::item::variant::{self, VariantMap};
use crate::item::{ItemStack, ItemType};

mod slots;
#[cfg(test)]
mod tests;

pub use slots::{
    insert_into_slots, merge_stack, place_cursor_count, plan_drag_distribution, slot_capacity,
    take_slot_stack,
};

pub const HOTBAR_LEN: usize = 9;
pub const MAIN_LEN: usize = 27;
pub const TOTAL_SLOTS: usize = HOTBAR_LEN + MAIN_LEN; // 36

/// Which hand an action reads its held item from. `Main` is the selected
/// hotbar slot; `Off` is the dedicated off-hand slot. The off-hand never
/// participates in pickup routing (`add`) or crafting — it only holds what a
/// player deliberately put there.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum Hand {
    #[default]
    Main,
    Off,
}

#[derive(Clone, Debug)]
pub struct Inventory {
    slots: [Option<ItemStack>; TOTAL_SLOTS],
    cursor: Option<ItemStack>,
    off_hand: Option<ItemStack>,
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
            off_hand: None,
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
    #[inline]
    pub fn off_hand(&self) -> Option<&ItemStack> {
        self.off_hand.as_ref()
    }
    #[inline]
    pub fn off_hand_mut(&mut self) -> &mut Option<ItemStack> {
        // Conservative: assume the borrower mutates.
        self.bump_revision();
        &mut self.off_hand
    }
    /// The stack a `hand` holds — the one accessor every acting-hand read
    /// resolves through, so main and off behave identically by construction.
    #[inline]
    pub fn held_in(&self, hand: Hand) -> Option<&ItemStack> {
        match hand {
            Hand::Main => self.selected(),
            Hand::Off => self.off_hand(),
        }
    }
    pub fn take_off_hand(&mut self) -> Option<ItemStack> {
        if self.off_hand.is_some() {
            self.bump_revision();
        }
        self.off_hand.take()
    }
    /// Swap the off-hand slot with inventory slot `i` — the F gesture: the
    /// selected hotbar slot in gameplay, the hovered slot in a menu. A swap
    /// with one side empty moves the stack across; both empty is a true
    /// no-op (revision untouched).
    pub fn swap_off_hand_with_slot(&mut self, i: usize) {
        let Some(slot) = self.slots.get_mut(i) else {
            return;
        };
        if slot.is_none() && self.off_hand.is_none() {
            return;
        }
        std::mem::swap(slot, &mut self.off_hand);
        self.bump_revision();
    }
    /// The off-hand swap against an EXTERNAL cell (an open container's slot),
    /// spec-checked like a click — but ALL-OR-NOTHING: a cell whose spec
    /// refuses the off-hand stack (filter or runtime mask or take-only)
    /// swaps NOTHING, unlike a click, which still takes. A swap is one
    /// gesture over two stacks; half-executing it (take without give) reads
    /// as item loss, not as a refusal. An EMPTY off-hand is a pure take and
    /// is never gated (takes never are). Runs identically on the server's
    /// world cell and the client's mirror cell — the `click_container_cell`
    /// parity contract.
    pub fn swap_off_hand_with_cell(
        &mut self,
        spec: Option<&crate::container::SlotSpec>,
        gui_state: Option<&crate::gui_state::GuiStateMap>,
        cell: &mut Option<ItemStack>,
    ) -> bool {
        if let (Some(spec), Some(held)) = (spec, self.off_hand.as_ref()) {
            if spec.take_only || !spec.admits(held.item, spec.accepts_mask(gui_state)) {
                return false;
            }
        }
        if cell.is_none() && self.off_hand.is_none() {
            return false;
        }
        std::mem::swap(cell, &mut self.off_hand);
        self.bump_revision();
        true
    }
    /// World-PICKUP routing: top up the OFF-HAND first when it already holds
    /// the same item (a torch stack in the left hand keeps itself filled),
    /// then route the remainder through the ordinary hotbar→main insertion
    /// ([`add`](Self::add)). An empty or different-item off-hand is never a
    /// pickup destination. Pickup is the ONLY path with this routing — gives,
    /// crafting returns, and menu moves deliberately stay on `add`.
    pub fn pickup(&mut self, stack: ItemStack) -> Option<ItemStack> {
        match self.pickup_into_off_hand(stack) {
            None => None,
            Some(rest) => self.add(rest),
        }
    }
    /// Capacity twin of [`pickup`](Self::pickup) — the partial-pickup planner.
    pub fn pickup_fits_count(&self, stack: ItemStack) -> u8 {
        if stack.is_empty() {
            return 0;
        }
        let off_room = self
            .off_hand
            .as_ref()
            .filter(|off| off.can_stack_with(&stack))
            .map_or(0, ItemStack::space_left);
        if off_room >= stack.count {
            return stack.count;
        }
        off_room + self.fits_count(stack.restack(stack.count - off_room))
    }
    fn pickup_into_off_hand(&mut self, stack: ItemStack) -> Option<ItemStack> {
        let Some(off) = self.off_hand.as_mut() else {
            return Some(stack);
        };
        if !off.can_stack_with(&stack) || off.space_left() == 0 {
            return Some(stack);
        }
        let moved = off.space_left().min(stack.count);
        off.count += moved;
        self.bump_revision();
        let rest = stack.count - moved;
        (rest > 0).then(|| stack.restack(rest))
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
        self.decrement_held(Hand::Main);
    }
    pub fn decrement_held(&mut self, hand: Hand) {
        if self.held_in(hand).is_none() {
            return;
        }
        self.bump_revision();
        let slot = self.held_slot_mut(hand);
        if let Some(stack) = slot.as_mut() {
            stack.count = stack.count.saturating_sub(1);
            if stack.count == 0 {
                *slot = None;
            }
        }
    }
    /// Swap ONE of the selected stack for `replacement` — a bucket filling or
    /// emptying in the hand. A single item swaps in place (keeping its slot);
    /// one of a larger stack converts, with the replacement going to any open
    /// slot. Refuses (returning `false`, changing nothing) when the selected
    /// slot is empty or the replacement has nowhere to go — all-or-nothing, so
    /// the world mutation it accompanies can be gated on it.
    pub fn replace_selected_one(&mut self, replacement: ItemStack) -> bool {
        self.replace_held_one(Hand::Main, replacement)
    }
    pub fn replace_held_one(&mut self, hand: Hand, replacement: ItemStack) -> bool {
        if self.held_in(hand).is_none() {
            return false;
        }
        self.bump_revision();
        let held = self.held_slot_mut(hand);
        let stack = held.as_mut().expect("checked above");
        if stack.count <= 1 {
            *held = Some(replacement);
            return true;
        }
        stack.count -= 1;
        if self.add(replacement).is_some() {
            // No room for the replacement anywhere: restore and refuse.
            if let Some(stack) = self.held_slot_mut(hand).as_mut() {
                stack.count += 1;
            }
            return false;
        }
        true
    }
    fn held_slot_mut(&mut self, hand: Hand) -> &mut Option<ItemStack> {
        match hand {
            Hand::Main => &mut self.slots[self.active as usize],
            Hand::Off => &mut self.off_hand,
        }
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
            Self::drain_into(
                &mut cursor,
                core::slice::from_mut(&mut self.off_hand),
                take_full,
            );
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
            Self::drain_into(
                &mut cursor,
                core::slice::from_mut(&mut self.off_hand),
                take_full,
            );
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
    /// One container cell's click, spec and all — the SINGLE decision the
    /// server's menu tick and the client's optimistic mirror both run.
    ///
    /// Two copies of this rule is a class of bug, not one bug: whatever the
    /// client does differently shows up as a click that lands and then snaps
    /// back a tick later, which reads as lag rather than as a refusal.
    pub fn click_container_cell(
        &mut self,
        spec: Option<&crate::container::SlotSpec>,
        gui_state: Option<&crate::gui_state::GuiStateMap>,
        cell: &mut Option<ItemStack>,
        secondary: bool,
    ) {
        // A slot that declares what it ACCEPTS refuses everything else on a
        // manual click, not only on shift-routing: a filter that holds for the
        // convenience gesture and not the deliberate one is not a filter, and
        // a machine reading a fixed slot index cannot defend itself against
        // what it finds there. Refusing behaves exactly like a take-only slot —
        // the click still takes the cell's contents out. The session's
        // `gui_state` narrows a bound slot's filters the same way on both
        // mirrors (see `SlotSpec::accepts_mask`).
        let refuses = match spec {
            None => false,
            Some(spec) => match self.cursor() {
                Some(held) => !spec.admits(held.item, spec.accepts_mask(gui_state)),
                None => spec.take_only,
            },
        };
        if refuses {
            self.click_take_only_external_slot(cell, secondary);
        } else if secondary {
            self.right_click_external_slot(cell);
        } else {
            self.click_external_slot(cell);
        }
    }

    /// Click a take-only output. Primary takes the whole output when it fits;
    /// secondary takes half onto an empty cursor, or one onto a compatible
    /// cursor. The held cursor stack is never placed into the output cell.
    pub fn click_take_only_external_slot(&mut self, slot: &mut Option<ItemStack>, secondary: bool) {
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
            None => *cursor = Some(output.restack(moved)),
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
                s.count -= take;
                *cursor = Some(s.restack(take));
                *slot = (s.count > 0).then_some(s);
            }
            (Some(mut c), None) => {
                *slot = Some(c.restack(1));
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
    pub fn place_cursor_count_in_slot(&mut self, i: usize, wanted: u8) -> u8 {
        if i >= TOTAL_SLOTS || wanted == 0 {
            return 0;
        }
        self.bump_revision();
        place_cursor_count(&mut self.cursor, &mut self.slots[i], wanted)
    }
    pub fn place_cursor_count_in_external_slot(
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
    pub fn take_slot_for_drop(&mut self, i: usize, all: bool) -> Option<ItemStack> {
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
    pub fn consume_slots(&mut self, takes: &[(usize, u8)]) -> bool {
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

    /// Every carried slot in the ONE layout the mod surface publishes and
    /// spends in: the grid in slot order (hotbar first), then the off hand
    /// last. The cursor is not carried — it is a click in progress.
    pub fn carried(&self) -> impl Iterator<Item = Option<&ItemStack>> {
        self.slots
            .iter()
            .chain(std::iter::once(&self.off_hand))
            .map(Option::as_ref)
    }

    /// Remove `count` of `item` in [`carried`](Self::carried) order and
    /// answer the taken stack. `data` picks which stacks may give: `None` =
    /// those sharing the FIRST matching stack's instance data, so exactly
    /// one variant leaves; `Some(map)` = only stacks carrying exactly `map`
    /// (empty = plain stacks). The map is COMPARED against what is carried,
    /// never interned — a spend that finds nothing must not mint a variant
    /// row. Whole or nothing: short by even one, nothing moves and `None`
    /// answers.
    pub fn take(
        &mut self,
        item: ItemType,
        count: u8,
        data: Option<&VariantMap>,
    ) -> Option<ItemStack> {
        if count == 0 {
            return None;
        }
        let variant = self
            .carried()
            .flatten()
            .find(|stack| {
                stack.item == item && data.is_none_or(|map| variant::matches(stack.variant, map))
            })
            .map(|stack| stack.variant)?;
        let same = |stack: &ItemStack| stack.item == item && stack.variant == variant;
        let available: u32 = self
            .carried()
            .flatten()
            .filter(|stack| same(stack))
            .map(|stack| u32::from(stack.count))
            .sum();
        if available < u32::from(count) {
            return None;
        }
        let mut left = count;
        for slot in self
            .slots
            .iter_mut()
            .chain(std::iter::once(&mut self.off_hand))
        {
            if left == 0 {
                break;
            }
            if let Some(stack) = slot.filter(same) {
                let take = stack.count.min(left);
                left -= take;
                *slot = (stack.count > take).then(|| stack.restack(stack.count - take));
            }
        }
        self.bump_revision();
        Some(ItemStack::with_variant(item, count, variant))
    }

    /// Move the off-hand stack into the ordinary grid (the shift-click on the
    /// off-hand cell). Whatever does not fit stays in the off-hand.
    pub fn shift_move_off_hand(&mut self) {
        let Some(stack) = self.off_hand.take() else {
            return;
        };
        self.bump_revision();
        self.off_hand = self.add_to_range(stack, 0, TOTAL_SLOTS);
    }

    pub fn from_parts(
        slots: [Option<ItemStack>; TOTAL_SLOTS],
        cursor: Option<ItemStack>,
        off_hand: Option<ItemStack>,
        active: u8,
    ) -> Self {
        Self {
            slots,
            cursor,
            off_hand,
            active: active.min(HOTBAR_LEN as u8 - 1),
            revision: 0,
        }
    }
}

#[cfg(test)]
mod take_tests {
    use super::*;
    use crate::item::{variant, VariantId};

    fn tinted() -> VariantId {
        let mut m = VariantMap::new();
        m.insert("t:k".into(), vec![1]);
        variant::intern(&m).unwrap()
    }

    /// `take` is whole-or-nothing across stacks and never blends variants:
    /// a partial take would strand a spend that already committed, and a
    /// blended one would hand one variant's data to a stack of another.
    #[test]
    fn take_is_whole_or_nothing_and_drains_one_variant_across_stacks() {
        let tinted = tinted();
        let mut inv = Inventory::new();
        *inv.slot_mut(0).unwrap() = Some(ItemStack::new(ItemType::Dirt, 2));
        *inv.slot_mut(1).unwrap() = Some(ItemStack::with_variant(ItemType::Dirt, 5, tinted));
        *inv.off_hand_mut() = Some(ItemStack::new(ItemType::Dirt, 1));

        assert!(
            inv.take(ItemType::Dirt, 4, None).is_none(),
            "three plain, five tinted: no four of one"
        );
        assert_eq!(inv.slot(0).map(|s| s.count), Some(2), "nothing moved");

        let took = inv
            .take(ItemType::Dirt, 3, None)
            .expect("three plain across the grid and the off hand");
        assert_eq!(
            (took.item, took.count, took.variant),
            (ItemType::Dirt, 3, VariantId::NONE)
        );
        assert!(inv.slot(0).is_none() && inv.off_hand().is_none());
        assert_eq!(
            inv.slot(1).map(|s| s.count),
            Some(5),
            "the other variant untouched"
        );
        assert!(inv.take(ItemType::Stone, 1, None).is_none());
    }

    /// A named variant takes only its own stacks, even when a plain stack
    /// sits earlier in the layout.
    #[test]
    fn take_by_data_skips_every_other_variant() {
        let tinted = tinted();
        let tint_map = variant::get(tinted).unwrap();
        let mut inv = Inventory::new();
        *inv.slot_mut(0).unwrap() = Some(ItemStack::new(ItemType::Dirt, 4));
        *inv.slot_mut(3).unwrap() = Some(ItemStack::with_variant(ItemType::Dirt, 2, tinted));

        assert!(inv.take(ItemType::Dirt, 3, Some(&tint_map)).is_none());
        let took = inv
            .take(ItemType::Dirt, 2, Some(&tint_map))
            .expect("both tinted");
        assert_eq!(took.variant, tinted);
        assert!(inv.slot(3).is_none());
        assert_eq!(
            inv.slot(0).map(|s| s.count),
            Some(4),
            "the plain stack untouched"
        );
        let plain = VariantMap::new();
        assert!(inv.take(ItemType::Dirt, 1, Some(&plain)).is_some());
        let mut unknown = VariantMap::new();
        unknown.insert("t:nobody".into(), vec![9]);
        assert!(
            inv.take(ItemType::Dirt, 1, Some(&unknown)).is_none(),
            "a map nobody carries takes nothing and mints nothing"
        );
    }

    /// The layout is one fact the read and the spend share: the grid, then
    /// the off hand last.
    #[test]
    fn carried_lists_the_grid_then_the_off_hand() {
        let mut inv = Inventory::new();
        *inv.slot_mut(0).unwrap() = Some(ItemStack::new(ItemType::Stone, 1));
        *inv.off_hand_mut() = Some(ItemStack::new(ItemType::Dirt, 1));
        let carried: Vec<Option<ItemType>> = inv.carried().map(|s| s.map(|s| s.item)).collect();
        assert_eq!(carried.len(), TOTAL_SLOTS + 1);
        assert_eq!(carried[0], Some(ItemType::Stone));
        assert_eq!(carried[TOTAL_SLOTS], Some(ItemType::Dirt));
    }
}

#[cfg(test)]
mod click_filter_tests {
    use super::*;
    use crate::container::SlotSpec;
    use crate::item::{ItemTag, ItemType};

    fn spec(accepts: Vec<crate::container::SlotFilter>, take_only: bool) -> SlotSpec {
        SlotSpec {
            accepts,
            take_only,
            accepts_bind: None,
        }
    }

    /// A filtered slot refuses the cursor stack on a plain click, and refusing
    /// still TAKES: the click is not swallowed, it just does not deposit.
    ///
    /// This exact call runs on BOTH sides of prediction — the client mirror in
    /// `menu_prediction` and the server's menu tick — so a divergence here is
    /// not a wrong pixel, it is a click that visibly lands and then snaps back
    /// a tick later, which reads as lag rather than as a refusal.
    #[test]
    fn a_filtered_container_cell_refuses_a_place_but_still_gives_up_its_stack() {
        let fuel_only = spec(
            vec![crate::container::SlotFilter::Tag(ItemTag::FUEL)],
            false,
        );
        let unfiltered = spec(Vec::new(), false);
        let coal = ItemType::by_key("petramond:coal").expect("engine item");
        let stone = ItemType::by_key("petramond:stone").expect("engine item");

        let mut inv = Inventory::new();
        *inv.cursor_mut() = Some(ItemStack::new(stone, 1));
        let mut cell = None;
        inv.click_container_cell(Some(&fuel_only), None, &mut cell, false);
        assert!(
            cell.is_none(),
            "a non-fuel stack must not enter a fuel slot"
        );
        assert!(inv.cursor().is_some(), "and the cursor keeps it");

        *inv.cursor_mut() = Some(ItemStack::new(coal, 1));
        inv.click_container_cell(Some(&fuel_only), None, &mut cell, false);
        assert!(cell.is_some(), "fuel enters a fuel slot");

        // Refusing still takes: a filtered slot is not inert.
        *inv.cursor_mut() = None;
        let mut occupied = Some(ItemStack::new(coal, 3));
        inv.click_container_cell(Some(&fuel_only), None, &mut occupied, false);
        assert!(occupied.is_none(), "the click still empties the slot");

        // An unfiltered slot is unchanged by any of this.
        *inv.cursor_mut() = Some(ItemStack::new(stone, 1));
        let mut open = None;
        inv.click_container_cell(Some(&unfiltered), None, &mut open, false);
        assert!(open.is_some(), "an unfiltered slot still takes anything");
    }
}
