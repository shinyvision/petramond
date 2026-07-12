use super::ContainerMenu;
use crate::crafting::Recipes;
use crate::inventory::Inventory;
use crate::item::ItemStack;

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

    /// Take one craft from the result slot onto the cursor: places the result
    /// on the cursor (stacking onto a matching held stack with room) and
    /// applies the matched recipe's transaction to the inputs. No-op if
    /// nothing matches or the cursor can't accept the whole result. Remainders
    /// go input cell → inventory → `overflow` (the caller's world-drop path).
    pub(crate) fn craft_take_result(
        &mut self,
        inv: &mut Inventory,
        recipes: &Recipes,
        mut overflow: impl FnMut(ItemStack),
    ) {
        let mut leftovers = Vec::new();
        self.craft
            .take_result(recipes, inv.cursor_mut(), |s| leftovers.push(s));
        for stack in leftovers {
            if let Some(rest) = inv.add(stack) {
                overflow(rest);
            }
        }
    }

    /// Shift-click the result: craft as many times as possible straight into
    /// the inventory. Bounded by CONSUMED ingredients — a retained catalyst
    /// never depletes, so the loop ends when a consumed cell runs out, the
    /// recipe stops matching (a remainder replaced its input), or the next
    /// result won't fully fit.
    pub(crate) fn craft_shift_result(
        &mut self,
        inv: &mut Inventory,
        recipes: &Recipes,
        mut overflow: impl FnMut(ItemStack),
    ) {
        for _ in 0..(64 * crate::crafting::MAX_CELLS) {
            let Some(m) = self.craft.current_match(recipes) else {
                break;
            };
            if !inv.can_add(m.result) {
                break;
            }
            inv.add(m.result);
            let mut leftovers = Vec::new();
            self.craft.apply_craft(&m, |s| leftovers.push(s));
            self.craft.recompute(recipes);
            for stack in leftovers {
                if let Some(rest) = inv.add(stack) {
                    overflow(rest);
                }
            }
        }
    }
}
