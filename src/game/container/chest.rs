use super::{ContainerMenu, ContainerTarget};
use crate::chest::Chest;
use crate::gui::ChestView;
use crate::inventory::{insert_into_slots, Inventory};
use crate::world::World;

impl ContainerMenu {
    pub(in crate::game) fn open_chest_view(&self, world: &World) -> Option<ChestView> {
        let ContainerTarget::Chest(pos) = self.target else {
            return None;
        };
        Some(ChestView {
            slots: world.chest_at(pos)?.slots,
        })
    }
    fn edit_open_chest(
        &self,
        world: &mut World,
        inv: &mut Inventory,
        edit: impl FnOnce(&mut Inventory, &mut Chest),
    ) {
        let ContainerTarget::Chest(pos) = self.target else {
            return;
        };
        if let Some(chest) = world.chest_at_mut(pos) {
            edit(inv, chest);
        }
        world.mark_chunk_modified(pos);
    }
    pub(super) fn chest_click_slot(&self, world: &mut World, inv: &mut Inventory, i: usize) {
        self.edit_open_chest(world, inv, |inv, chest| {
            if let Some(slot) = chest.slots.get_mut(i) {
                inv.click_external_slot(slot);
            }
        });
    }
    pub(super) fn chest_right_click_slot(&self, world: &mut World, inv: &mut Inventory, i: usize) {
        self.edit_open_chest(world, inv, |inv, chest| {
            if let Some(slot) = chest.slots.get_mut(i) {
                inv.right_click_external_slot(slot);
            }
        });
    }
    pub(super) fn chest_shift_slot(&self, world: &mut World, inv: &mut Inventory, i: usize) {
        self.edit_open_chest(world, inv, |inv, chest| {
            if let Some(slot) = chest.slots.get_mut(i) {
                inv.pull_from(slot);
            }
        });
    }
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
        if let Some(chest) = world.chest_at_mut(pos) {
            let Some(src) = inv.slot_mut(i) else {
                return;
            };
            if let Some(stack) = src.take() {
                *src = insert_into_slots(&mut chest.slots, stack);
            }
        }
        world.mark_chunk_modified(pos);
    }
    pub(super) fn collect_to_cursor_in_chest(&self, world: &mut World, inv: &mut Inventory) {
        self.edit_open_chest(world, inv, |inv, chest| {
            inv.collect_to_cursor_including(&mut chest.slots)
        });
    }
}
