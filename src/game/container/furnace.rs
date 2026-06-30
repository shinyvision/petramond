use super::{ContainerMenu, ContainerTarget};
use crate::furnace::Furnace;
use crate::gui::FurnaceView;
use crate::inventory::Inventory;
use crate::world::World;

impl ContainerMenu {
    pub(in crate::game) fn open_furnace_view(&self, world: &World) -> Option<FurnaceView> {
        let ContainerTarget::Furnace(pos) = self.target else {
            return None;
        };
        let f = world.furnace_at(pos)?;
        Some(FurnaceView {
            input: f.input,
            fuel: f.fuel,
            output: f.output,
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
        edit: impl FnOnce(&mut Inventory, &mut Furnace),
    ) {
        let ContainerTarget::Furnace(pos) = self.target else {
            return;
        };
        if let Some(furnace) = world.furnace_at_mut(pos) {
            edit(inv, furnace);
        }
        world.mark_chunk_modified(pos);
    }
    pub(super) fn furnace_click_input(&self, world: &mut World, inv: &mut Inventory) {
        self.edit_open_furnace(world, inv, |inv, f| inv.click_external_slot(f.input_slot()));
    }
    pub(super) fn furnace_right_click_input(&self, world: &mut World, inv: &mut Inventory) {
        self.edit_open_furnace(world, inv, |inv, f| {
            inv.right_click_external_slot(f.input_slot())
        });
    }
    pub(super) fn furnace_click_fuel(&self, world: &mut World, inv: &mut Inventory) {
        self.edit_open_furnace(world, inv, |inv, f| inv.click_external_slot(f.fuel_slot()));
    }
    pub(super) fn furnace_right_click_fuel(&self, world: &mut World, inv: &mut Inventory) {
        self.edit_open_furnace(world, inv, |inv, f| {
            inv.right_click_external_slot(f.fuel_slot())
        });
    }
    pub(super) fn furnace_take_output(&self, world: &mut World, inv: &mut Inventory) {
        self.edit_open_furnace(world, inv, |inv, f| {
            f.take_output(inv.cursor_mut());
        });
    }
    pub(super) fn furnace_shift_input(&self, world: &mut World, inv: &mut Inventory) {
        self.edit_open_furnace(world, inv, |inv, f| inv.pull_from(f.input_slot()));
    }
    pub(super) fn furnace_shift_fuel(&self, world: &mut World, inv: &mut Inventory) {
        self.edit_open_furnace(world, inv, |inv, f| inv.pull_from(f.fuel_slot()));
    }
    pub(super) fn furnace_shift_output(&self, world: &mut World, inv: &mut Inventory) {
        self.edit_open_furnace(world, inv, |inv, f| f.shift_output_into(inv));
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
        if let Some(furnace) = world.furnace_at_mut(pos) {
            if let Some(src) = inv.slot_mut(i) {
                furnace.shift_in(role, src);
            }
        }
        world.mark_chunk_modified(pos);
    }
}
