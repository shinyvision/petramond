use super::{ContainerMenu, ContainerTarget};
use crate::crafting::Recipes;
use crate::gui::WorkbenchView;
use crate::inventory::Inventory;
use crate::item::ItemType;

impl ContainerMenu {
    pub(crate) fn open_workbench_view(&self, recipes: &Recipes) -> Option<WorkbenchView> {
        if !matches!(self.target, ContainerTarget::FurnitureWorkbench) {
            return None;
        }
        Some(WorkbenchView {
            input: self.workbench_input,
            results: self.workbench_results(recipes),
        })
    }
    fn workbench_results(&self, recipes: &Recipes) -> Vec<(ItemType, bool)> {
        match self.workbench_input {
            Some(stack) => recipes
                .furniture_for(stack.item)
                .map(|r| (r.result.item, stack.count >= r.cost))
                .collect(),
            None => Vec::new(),
        }
    }
    pub(super) fn workbench_shift_from_inventory(
        &mut self,
        inv: &mut Inventory,
        recipes: &Recipes,
        i: usize,
    ) {
        let Some(stack) = inv.slot(i).copied() else {
            return;
        };
        if recipes.furniture_for(stack.item).next().is_none() {
            inv.shift_move_slot(i);
            return;
        }
        // `self.workbench_input` and the inventory slot are disjoint borrows.
        if let Some(src) = inv.slot_mut(i) {
            crate::inventory::merge_stack(src, &mut self.workbench_input);
        }
    }
    pub(super) fn workbench_shift_input(&mut self, inv: &mut Inventory) {
        if let Some(stack) = self.workbench_input.take() {
            if let Some(leftover) = inv.add(stack) {
                self.workbench_input = Some(leftover);
            }
        }
    }
    pub(super) fn workbench_take_result(
        &mut self,
        inv: &mut Inventory,
        recipes: &Recipes,
        i: usize,
        shift: bool,
    ) {
        let Some(input) = self.workbench_input else {
            return;
        };
        let Some(recipe) = recipes.furniture_for(input.item).nth(i).copied() else {
            return;
        };
        if input.count < recipe.cost {
            return; // greyed: not enough of the input block
        }
        if shift {
            // Craft repeatedly into the inventory until the input runs out or it won't fit.
            for _ in 0..(64 * 64) {
                let Some(have) = self.workbench_input else {
                    break;
                };
                if have.count < recipe.cost || !inv.can_add(recipe.result) {
                    break;
                }
                inv.add(recipe.result);
                self.consume_workbench_input(recipe.cost);
            }
        } else {
            // Place one craft onto the cursor (only if the whole result fits), then
            // consume the input cost — exactly the crafting-result take rule.
            let cursor = inv.cursor_mut();
            let placed = match cursor {
                None => {
                    *cursor = Some(recipe.result);
                    true
                }
                Some(cur)
                    if cur.can_stack_with(&recipe.result)
                        && cur.space_left() >= recipe.result.count =>
                {
                    cur.count += recipe.result.count;
                    true
                }
                _ => false,
            };
            if placed {
                self.consume_workbench_input(recipe.cost);
            }
        }
    }
    fn consume_workbench_input(&mut self, cost: u8) {
        if let Some(stack) = &mut self.workbench_input {
            stack.count = stack.count.saturating_sub(cost);
            if stack.count == 0 {
                self.workbench_input = None;
            }
        }
    }
}
