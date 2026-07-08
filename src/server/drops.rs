use crate::entity::DroppedItem;
use crate::inventory::Inventory;
use crate::item::{ItemStack, ItemType};

use super::entities::light_at_pos;
use super::game::ServerGame;
use crate::game::tick::TickEvents;

#[derive(Clone, Debug, Default)]
pub(crate) struct DropQueue {
    pending: Vec<PendingDropAction>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum PendingDropAction {
    Selected { slot: u8, all: bool },
    Cursor(ItemStack),
    Stack(ItemStack),
}

impl DropQueue {
    pub(crate) fn queue_stack(&mut self, stack: ItemStack) {
        self.pending.push(PendingDropAction::Stack(stack));
    }

    pub(crate) fn queue_selected(&mut self, slot: u8, all: bool) {
        self.pending.push(PendingDropAction::Selected { slot, all });
    }

    pub(crate) fn queue_cursor_stack(&mut self, inventory: &Inventory) {
        if let Some(stack) = self.available_cursor_throw_stack(inventory) {
            self.pending.push(PendingDropAction::Cursor(stack));
        }
    }

    pub(crate) fn queue_cursor_one(&mut self, inventory: &Inventory) {
        if let Some(stack) = self.available_cursor_throw_stack(inventory) {
            self.pending
                .push(PendingDropAction::Cursor(ItemStack::new(stack.item, 1)));
        }
    }

    pub(crate) fn close_cursor_stack(&mut self, inventory: &mut Inventory) {
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

impl ServerGame {
    /// Close-time cleanup for a cursor-held GUI stack: merge it back into matching
    /// inventory stacks, then empty slots, and queue only any leftover to drop into
    /// the world on the next tick. Cursor throws already queued by an outside-panel
    /// click are reservations: closing the menu stashes only the unreserved remainder
    /// so the fixed tick can still apply the user's throw.
    pub(crate) fn close_cursor_stack_for(&mut self, s: usize) {
        let sess = &mut self.sessions[s];
        sess.drop_queue
            .close_cursor_stack(&mut sess.player.inventory);
    }

    /// Apply queued drop intents on the tick: remove the item from the inventory/cursor
    /// and spawn the matching dropped entity in the same fixed-tick phase, before item
    /// physics gives fresh drops their first step.
    pub(crate) fn tick_drops(&mut self, s: usize, events: &mut TickEvents) {
        for action in self.sessions[s].drop_queue.drain() {
            let stack = match action {
                PendingDropAction::Selected { slot, all } => {
                    self.take_hotbar_slot_for_drop(s, slot, all)
                }
                PendingDropAction::Cursor(stack) => {
                    self.consume_cursor_throw(s, stack);
                    Some(stack)
                }
                PendingDropAction::Stack(stack) => Some(stack),
            };
            if let Some(stack) = stack {
                self.spawn_thrown_item(s, stack);
                events.player(s).threw_item = true;
            }
        }
    }

    fn take_hotbar_slot_for_drop(&mut self, s: usize, slot: u8, all: bool) -> Option<ItemStack> {
        let cell = self.sessions[s].player.inventory.slot_mut(slot as usize)?;
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

    fn consume_cursor_throw(&mut self, s: usize, stack: ItemStack) {
        let cell = self.sessions[s].player.inventory.cursor_mut();
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

    /// Spawn `stack` as a thrown dropped item from the throwing player's eye,
    /// along their view direction.
    fn spawn_thrown_item(&mut self, s: usize, stack: ItemStack) {
        let (eye, dir) = {
            let p = &self.sessions[s].player;
            (p.eye(), p.forward())
        };
        let origin = eye + dir * 0.3;
        let mut drop = DroppedItem::thrown(origin, stack, dir);
        (drop.skylight, drop.blocklight) = light_at_pos(&self.world, origin);
        self.world.spawn_item(drop);
    }
}
