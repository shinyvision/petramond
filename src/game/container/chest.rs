use super::{with_open_container, ContainerMenu, ContainerTarget};
use crate::chest::Chest;
use crate::gui::ChestView;
use crate::inventory::{Inventory, SlotGrid};
use crate::world::World;

impl ContainerMenu {
    /// The view of the currently-open chest for the UI (its 27 storage slots), or
    /// `None` if no chest screen is up or it has unloaded.
    pub(in crate::game) fn open_chest_view(&self, world: &World) -> Option<ChestView> {
        let ContainerTarget::Chest(pos) = self.target else {
            return None;
        };
        Some(world.chest_at(pos)?.view())
    }

    /// Run `edit` on the open chest's contents, then mark its chunk modified so the
    /// change persists (an idle chest wouldn't otherwise be re-saved). No-op when no
    /// chest screen is open or the chest has unloaded.
    fn edit_open_chest(
        &self,
        world: &mut World,
        inv: &mut Inventory,
        edit: impl FnOnce(&mut Inventory, &mut Chest),
    ) {
        let ContainerTarget::Chest(pos) = self.target else {
            return;
        };
        with_open_container(world, pos, |chest: &mut Chest| edit(inv, chest));
    }

    /// Left-click a chest storage slot: cursor pick/drop/merge/swap.
    pub(super) fn chest_click_slot(&self, world: &mut World, inv: &mut Inventory, i: usize) {
        self.edit_open_chest(world, inv, |inv, chest| {
            if let Some(slot) = chest.slots_mut().get_mut(i) {
                inv.click_external_slot(slot);
            }
        });
    }

    /// Right-click a chest storage slot: split / place-one.
    pub(super) fn chest_right_click_slot(&self, world: &mut World, inv: &mut Inventory, i: usize) {
        self.edit_open_chest(world, inv, |inv, chest| {
            if let Some(slot) = chest.slots_mut().get_mut(i) {
                inv.right_click_external_slot(slot);
            }
        });
    }

    /// Shift-click a chest storage slot: move its stack to the inventory (whatever
    /// doesn't fit stays put).
    pub(super) fn chest_shift_slot(&self, world: &mut World, inv: &mut Inventory, i: usize) {
        self.edit_open_chest(world, inv, |inv, chest| {
            if let Some(slot) = chest.slots_mut().get_mut(i) {
                inv.pull_from(slot);
            }
        });
    }

    /// Shift-click inventory slot `i` while the chest screen is open: move its whole
    /// stack into the chest (merging into matching stacks, then the first empty slot;
    /// leftover stays in the inventory).
    pub(super) fn chest_shift_from_inventory(
        &self,
        world: &mut World,
        inv: &mut Inventory,
        i: usize,
    ) {
        let ContainerTarget::Chest(pos) = self.target else {
            return;
        };
        if inv.slot(i).is_none() {
            return;
        }
        // `world` and the inventory slot are disjoint borrows, so the chest slots
        // and the inventory slot can be borrowed together for the move.
        with_open_container(world, pos, |chest: &mut Chest| {
            let Some(src) = inv.slot_mut(i) else {
                return;
            };
            // First-fit the whole source stack into the chest; whatever didn't fit
            // (a single source stack is ≤ one max stack, so the general insert lands
            // it in one empty slot just like the old single-slot fill) stays behind.
            if let Some(stack) = src.take() {
                *src = chest.insert(stack);
            }
        });
    }

    /// Double-click gather in the open chest screen: top up the cursor-held stack
    /// with matching items from BOTH the chest and the inventory.
    pub(super) fn collect_to_cursor_in_chest(&self, world: &mut World, inv: &mut Inventory) {
        self.edit_open_chest(world, inv, |inv, chest| {
            inv.collect_to_cursor_including(chest.slots_mut())
        });
    }
}
