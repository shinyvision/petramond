//! Furnace slot behavior for the open furnace session. Slots are the block's
//! generic [`Container`](crate::container::Container) under the furnace slot
//! convention; the burn/cook gauges read the sibling machine state.

use super::{ContainerMenu, ContainerTarget};
use crate::container::Container;
use crate::furnace::{merge_stack, stack_onto_cursor, Furnace, SLOT_FUEL, SLOT_INPUT, SLOT_OUTPUT};
use crate::gui::FurnaceView;
use crate::inventory::Inventory;
use crate::world::World;

impl ContainerMenu {
    pub(in crate::game) fn open_furnace_view(&self, world: &World) -> Option<FurnaceView> {
        let ContainerTarget::Furnace(pos) = self.target else {
            return None;
        };
        let f = world.furnace_at(pos)?;
        let slot = |i: usize| {
            world
                .container_at(pos)
                .and_then(|c| c.slots.get(i).copied().flatten())
        };
        Some(FurnaceView {
            input: slot(SLOT_INPUT),
            fuel: slot(SLOT_FUEL),
            output: slot(SLOT_OUTPUT),
            cook01: f.cook_progress as f32 / crate::furnace::COOK_TICKS as f32,
            burn01: if f.burn_max == 0 {
                0.0
            } else {
                f.burn_remaining as f32 / f.burn_max as f32
            },
        })
    }
    fn edit_open_furnace(
        &self,
        world: &mut World,
        inv: &mut Inventory,
        edit: impl FnOnce(&mut Inventory, &mut Container),
    ) {
        let ContainerTarget::Furnace(pos) = self.target else {
            return;
        };
        if let Some(container) = world.container_at_mut(pos) {
            edit(inv, container);
        }
        world.mark_chunk_modified(pos);
    }
    fn furnace_slot_click(&self, world: &mut World, inv: &mut Inventory, i: usize, right: bool) {
        self.edit_open_furnace(world, inv, |inv, c| {
            if let Some(slot) = c.slots.get_mut(i) {
                if right {
                    inv.right_click_external_slot(slot);
                } else {
                    inv.click_external_slot(slot);
                }
            }
        });
    }
    pub(super) fn furnace_click_input(&self, world: &mut World, inv: &mut Inventory) {
        self.furnace_slot_click(world, inv, SLOT_INPUT, false);
    }
    pub(super) fn furnace_right_click_input(&self, world: &mut World, inv: &mut Inventory) {
        self.furnace_slot_click(world, inv, SLOT_INPUT, true);
    }
    pub(super) fn furnace_click_fuel(&self, world: &mut World, inv: &mut Inventory) {
        self.furnace_slot_click(world, inv, SLOT_FUEL, false);
    }
    pub(super) fn furnace_right_click_fuel(&self, world: &mut World, inv: &mut Inventory) {
        self.furnace_slot_click(world, inv, SLOT_FUEL, true);
    }
    pub(super) fn furnace_take_output(&self, world: &mut World, inv: &mut Inventory) {
        self.edit_open_furnace(world, inv, |inv, c| {
            if let Some(slot) = c.slots.get_mut(SLOT_OUTPUT) {
                if let Some(out) = *slot {
                    if stack_onto_cursor(inv.cursor_mut(), out) {
                        *slot = None;
                    }
                }
            }
        });
    }
    pub(super) fn furnace_shift_input(&self, world: &mut World, inv: &mut Inventory) {
        self.edit_open_furnace(world, inv, |inv, c| {
            if let Some(slot) = c.slots.get_mut(SLOT_INPUT) {
                inv.pull_from(slot);
            }
        });
    }
    pub(super) fn furnace_shift_fuel(&self, world: &mut World, inv: &mut Inventory) {
        self.edit_open_furnace(world, inv, |inv, c| {
            if let Some(slot) = c.slots.get_mut(SLOT_FUEL) {
                inv.pull_from(slot);
            }
        });
    }
    pub(super) fn furnace_shift_output(&self, world: &mut World, inv: &mut Inventory) {
        self.edit_open_furnace(world, inv, |inv, c| {
            if let Some(slot) = c.slots.get_mut(SLOT_OUTPUT) {
                inv.pull_from(slot);
            }
        });
    }
    pub(super) fn furnace_shift_from_inventory(
        &self,
        world: &mut World,
        inv: &mut Inventory,
        i: usize,
    ) {
        let ContainerTarget::Furnace(pos) = self.target else {
            return;
        };
        let Some(stack) = inv.slot(i).copied() else {
            return;
        };
        // The fuel-vs-smeltable routing is furnace behavior (it reads item tags).
        let Some(role) = Furnace::fill_slot_for(stack.item) else {
            // Neither fuel nor smeltable: fall back to the ordinary hotbar↔grid move.
            inv.shift_move_slot(i);
            return;
        };
        if let Some(container) = world.container_at_mut(pos) {
            if let Some(src) = inv.slot_mut(i) {
                if let Some(dst) = container.slots.get_mut(role.index()) {
                    merge_stack(src, dst);
                }
            }
        }
        world.mark_chunk_modified(pos);
    }
}
