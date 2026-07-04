use crate::entity::DroppedItem;
use crate::inventory::Inventory;
use crate::item::{ItemStack, ItemType};

use super::{entities::light_at_pos, tick::TickEvents, Game};

#[derive(Clone, Debug, Default)]
pub(super) struct DropQueue {
    pending: Vec<PendingDropAction>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum PendingDropAction {
    Selected { slot: u8, all: bool },
    Cursor(ItemStack),
    Stack(ItemStack),
}

impl DropQueue {
    pub(super) fn queue_stack(&mut self, stack: ItemStack) {
        self.pending.push(PendingDropAction::Stack(stack));
    }

    pub(super) fn queue_selected(&mut self, slot: u8, all: bool) {
        self.pending.push(PendingDropAction::Selected { slot, all });
    }

    pub(super) fn queue_cursor_stack(&mut self, inventory: &Inventory) {
        if let Some(stack) = self.available_cursor_throw_stack(inventory) {
            self.pending.push(PendingDropAction::Cursor(stack));
        }
    }

    pub(super) fn queue_cursor_one(&mut self, inventory: &Inventory) {
        if let Some(stack) = self.available_cursor_throw_stack(inventory) {
            self.pending
                .push(PendingDropAction::Cursor(ItemStack::new(stack.item, 1)));
        }
    }

    pub(super) fn close_cursor_stack(&mut self, inventory: &mut Inventory) {
        let Some(cursor) = inventory.cursor().copied() else {
            return;
        };
        let reserved = self
            .pending_cursor_throw_count(cursor.item)
            .min(cursor.count);
        if reserved > 0 {
            let remainder = cursor.count - reserved;
            *inventory.cursor_mut() =
                (remainder > 0).then_some(ItemStack::new(cursor.item, remainder));
        }
        if let Some(stack) = inventory.stash_cursor_in_inventory() {
            self.queue_stack(stack);
        }
    }

    fn pending_cursor_throw_count(&self, item: ItemType) -> u8 {
        self.pending
            .iter()
            .filter_map(|action| match action {
                PendingDropAction::Cursor(stack) if stack.item == item => Some(stack.count),
                _ => None,
            })
            .fold(0u8, u8::saturating_add)
    }

    fn available_cursor_throw_stack(&self, inventory: &Inventory) -> Option<ItemStack> {
        let cursor = *inventory.cursor()?;
        let reserved = self.pending_cursor_throw_count(cursor.item);
        let count = cursor.count.saturating_sub(reserved);
        (count > 0).then_some(ItemStack::new(cursor.item, count))
    }

    fn drain(&mut self) -> Vec<PendingDropAction> {
        std::mem::take(&mut self.pending)
    }
}

impl Game {
    /// Close-time cleanup for a cursor-held GUI stack: merge it back into matching
    /// inventory stacks, then empty slots, and queue only any leftover to drop into
    /// the world on the next tick. Cursor throws already queued by an outside-panel
    /// click are reservations: closing the menu stashes only the unreserved remainder
    /// so the fixed tick can still apply the user's throw.
    pub(crate) fn close_cursor_stack(&mut self) {
        self.drop_queue
            .close_cursor_stack(&mut self.player.inventory);
    }

    /// Throw the whole cursor-held stack out into the world (inventory drag-out
    /// then click outside the panel). No-op when the cursor is empty.
    pub fn throw_cursor_stack(&mut self) {
        self.drop_queue.queue_cursor_stack(&self.player.inventory);
    }

    /// Throw a single item off the cursor-held stack (right-click outside the
    /// panel while dragging). No-op when the cursor is empty.
    pub fn throw_cursor_one(&mut self) {
        self.drop_queue.queue_cursor_one(&self.player.inventory);
    }

    /// Drop the player's held (active hotbar) item into the world via the in-game
    /// drop key. With `all`, the whole stack is thrown (Ctrl+Q); otherwise a
    /// single item (Q). No-op with an empty hand.
    pub fn drop_selected_item(&mut self, all: bool) {
        let slot = self.player.inventory.active_slot();
        self.drop_queue.queue_selected(slot, all);
    }

    /// Apply queued drop intents on the tick: remove the item from the inventory/cursor
    /// and spawn the matching dropped entity in the same fixed-tick phase, before item
    /// physics gives fresh drops their first step.
    pub(super) fn tick_drops(&mut self, events: &mut TickEvents) {
        for action in self.drop_queue.drain() {
            let stack = match action {
                PendingDropAction::Selected { slot, all } => {
                    self.take_hotbar_slot_for_drop(slot, all)
                }
                PendingDropAction::Cursor(stack) => {
                    self.consume_cursor_throw(stack);
                    Some(stack)
                }
                PendingDropAction::Stack(stack) => Some(stack),
            };
            if let Some(stack) = stack {
                self.spawn_thrown_item(stack);
                events.threw_item = true;
            }
        }
    }

    fn take_hotbar_slot_for_drop(&mut self, slot: u8, all: bool) -> Option<ItemStack> {
        let cell = self.player.inventory.slot_mut(slot as usize)?;
        if all {
            return cell.take();
        }
        let stack = cell.as_mut()?;
        let item = stack.item;
        stack.count -= 1;
        if stack.count == 0 {
            *cell = None;
        }
        Some(ItemStack::new(item, 1))
    }

    fn consume_cursor_throw(&mut self, stack: ItemStack) {
        let cell = self.player.inventory.cursor_mut();
        let Some(cursor) = cell.as_mut() else {
            return;
        };
        if cursor.item != stack.item {
            return;
        }
        cursor.count = cursor.count.saturating_sub(stack.count);
        if cursor.count == 0 {
            *cell = None;
        }
    }

    /// Spawn `stack` as a thrown dropped item using the tick-time camera pose.
    fn spawn_thrown_item(&mut self, stack: ItemStack) {
        let dir = self.cam.forward();
        let origin = self.cam.pos + dir * 0.3;
        let mut drop = DroppedItem::thrown(origin, stack, dir);
        (drop.skylight, drop.blocklight) = light_at_pos(&self.world, origin);
        self.world.spawn_item(drop);
    }
}
