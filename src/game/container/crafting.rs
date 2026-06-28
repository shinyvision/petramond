use super::ContainerMenu;
use crate::crafting::Recipes;
use crate::inventory::Inventory;

impl ContainerMenu {
    /// Left-click a crafting input cell (cursor pick/drop/merge/swap), then
    /// refresh the result preview.
    pub(super) fn craft_click_slot(&mut self, inv: &mut Inventory, recipes: &Recipes, i: usize) {
        if i >= self.craft.capacity() {
            return;
        }
        inv.click_external_slot(self.craft.cell_mut(i));
        self.craft.recompute(recipes);
    }

    /// Right-click a crafting input cell (split / place-one), then refresh.
    pub(super) fn craft_right_click_slot(
        &mut self,
        inv: &mut Inventory,
        recipes: &Recipes,
        i: usize,
    ) {
        if i >= self.craft.capacity() {
            return;
        }
        inv.right_click_external_slot(self.craft.cell_mut(i));
        self.craft.recompute(recipes);
    }

    /// Shift-click a crafting input cell: move its whole stack to the inventory
    /// (whatever doesn't fit stays in the cell), then refresh.
    pub(super) fn craft_shift_slot(&mut self, inv: &mut Inventory, recipes: &Recipes, i: usize) {
        if i >= self.craft.capacity() {
            return;
        }
        if let Some(stack) = self.craft.take_cell(i) {
            if let Some(leftover) = inv.add(stack) {
                *self.craft.cell_mut(i) = Some(leftover);
            }
        }
        self.craft.recompute(recipes);
    }

    /// Take one craft from the result slot onto the cursor: places the result on
    /// the cursor (stacking onto a matching held stack with room) and consumes one
    /// item from every occupied input cell. No-op if there's no result or the
    /// cursor can't accept the whole result.
    pub(super) fn craft_take_result(&mut self, inv: &mut Inventory, recipes: &Recipes) {
        self.craft.take_result(recipes, inv.cursor_mut());
    }

    /// Shift-click the result: craft as many times as possible straight into the
    /// inventory, stopping when an ingredient runs out or the next result won't
    /// fully fit. The hotbar/main grid both receive results (via `add`).
    pub(super) fn craft_shift_result(&mut self, inv: &mut Inventory, recipes: &Recipes) {
        // Bounded by the grid contents: each craft consumes ≥1 from every cell.
        for _ in 0..(64 * crate::crafting::MAX_CELLS) {
            let Some(result) = self.craft.result().copied() else {
                break;
            };
            if !inv.can_add(result) {
                break;
            }
            inv.add(result);
            self.craft.consume_one();
            self.craft.recompute(recipes);
        }
    }
}
