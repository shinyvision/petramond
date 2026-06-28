use super::{with_open_container, ContainerMenu, ContainerTarget};
use crate::furnace::Furnace;
use crate::gui::FurnaceView;
use crate::inventory::Inventory;
use crate::world::World;

impl ContainerMenu {
    /// The view of the currently-open furnace for the UI (its slots + the two
    /// progress gauges), or `None` if no furnace screen is up or it has unloaded.
    pub(in crate::game) fn open_furnace_view(&self, world: &World) -> Option<FurnaceView> {
        let ContainerTarget::Furnace(pos) = self.target else {
            return None;
        };
        Some(world.furnace_at(pos)?.view())
    }

    /// Run `edit` on the open furnace's contents, then mark its chunk modified so the
    /// change persists (an idle furnace wouldn't otherwise be re-saved). No-op when
    /// no furnace screen is open or the furnace has unloaded.
    fn edit_open_furnace(
        &self,
        world: &mut World,
        inv: &mut Inventory,
        edit: impl FnOnce(&mut Inventory, &mut Furnace),
    ) {
        let ContainerTarget::Furnace(pos) = self.target else {
            return;
        };
        with_open_container(world, pos, |f: &mut Furnace| edit(inv, f));
    }

    /// Left-click the furnace input (smeltable) slot: cursor pick/drop/merge/swap.
    pub(super) fn furnace_click_input(&self, world: &mut World, inv: &mut Inventory) {
        self.edit_open_furnace(world, inv, |inv, f| inv.click_external_slot(f.input_slot()));
    }

    /// Right-click the furnace input slot: split / place-one.
    pub(super) fn furnace_right_click_input(&self, world: &mut World, inv: &mut Inventory) {
        self.edit_open_furnace(world, inv, |inv, f| {
            inv.right_click_external_slot(f.input_slot())
        });
    }

    /// Left-click the furnace fuel slot: cursor pick/drop/merge/swap.
    pub(super) fn furnace_click_fuel(&self, world: &mut World, inv: &mut Inventory) {
        self.edit_open_furnace(world, inv, |inv, f| inv.click_external_slot(f.fuel_slot()));
    }

    /// Right-click the furnace fuel slot: split / place-one.
    pub(super) fn furnace_right_click_fuel(&self, world: &mut World, inv: &mut Inventory) {
        self.edit_open_furnace(world, inv, |inv, f| {
            inv.right_click_external_slot(f.fuel_slot())
        });
    }

    /// Click the furnace output: take-only — move the whole product onto the cursor
    /// if it fits. The take-only rule lives in [`Furnace::take_output`].
    pub(super) fn furnace_take_output(&self, world: &mut World, inv: &mut Inventory) {
        self.edit_open_furnace(world, inv, |inv, f| {
            f.take_output(inv.cursor_mut());
        });
    }

    /// Shift-click the furnace input slot: move its stack to the inventory (whatever
    /// doesn't fit stays put).
    pub(super) fn furnace_shift_input(&self, world: &mut World, inv: &mut Inventory) {
        self.edit_open_furnace(world, inv, |inv, f| inv.pull_from(f.input_slot()));
    }

    /// Shift-click the furnace fuel slot: move its stack to the inventory.
    pub(super) fn furnace_shift_fuel(&self, world: &mut World, inv: &mut Inventory) {
        self.edit_open_furnace(world, inv, |inv, f| inv.pull_from(f.fuel_slot()));
    }

    /// Shift-click the furnace output slot: move the product to the inventory
    /// (take-only out — never a deposit). See [`Furnace::shift_output_into`].
    pub(super) fn furnace_shift_output(&self, world: &mut World, inv: &mut Inventory) {
        self.edit_open_furnace(world, inv, |inv, f| f.shift_output_into(inv));
    }

    /// Shift-click inventory slot `i` while the furnace screen is open: routed by the
    /// furnace via [`Furnace::fill_slot_for`] — a fuel stack goes to the fuel slot and
    /// a smeltable stack to the input slot (leftover stays in the inventory). Items
    /// that are neither fall back to the normal hotbar↔grid move, so shift-click still
    /// does something sensible for them.
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
        // `world` and the inventory slot are disjoint borrows, so the furnace and
        // the inventory slot can be borrowed together for the move.
        with_open_container(world, pos, |furnace: &mut Furnace| {
            if let Some(src) = inv.slot_mut(i) {
                furnace.shift_in(role, src);
            }
        });
    }
}
