use super::{ContainerMenu, ContainerTarget};
use crate::crafting::Recipes;
use crate::gui::WorkbenchView;
use crate::inventory::Inventory;
use crate::item::ItemType;

impl ContainerMenu {
    /// The view of the open furniture workbench for the UI: its input block plus the
    /// results that block offers, each flagged craftable (enough input). `None` if no
    /// workbench screen is up. Recomputed from the recipes each call — cheap (≤21
    /// entries) and keeps the result list a pure function of the input.
    pub(in crate::game) fn open_workbench_view(&self, recipes: &Recipes) -> Option<WorkbenchView> {
        if !matches!(self.target, ContainerTarget::FurnitureWorkbench) {
            return None;
        }
        Some(WorkbenchView {
            input: self.workbench_input,
            results: self.workbench_results(recipes),
        })
    }

    /// The results the placed input block offers: `(result item, craftable now)` per
    /// furniture recipe whose input matches, row-major. Empty when the input is empty.
    fn workbench_results(&self, recipes: &Recipes) -> Vec<(ItemType, bool)> {
        match self.workbench_input {
            Some(stack) => recipes
                .furniture_for(stack.item)
                .map(|r| (r.result.item, stack.count >= r.cost))
                .collect(),
            None => Vec::new(),
        }
    }

    /// Shift-click inventory slot `i` while the workbench screen is open: move the stack
    /// INTO the single input slot (merging onto a matching block, filling it if empty).
    /// Only a block that actually feeds a furniture recipe is routed there; anything else
    /// falls back to the ordinary hotbar↔grid move so shift-click still does something —
    /// mirroring the furnace's tag-routed shift-in. The reverse direction (input → inv)
    /// is [`workbench_shift_input`](Self::workbench_shift_input).
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
            crate::furnace::merge_stack(src, &mut self.workbench_input);
        }
    }

    /// Shift-click the workbench input: move the whole input stack to the inventory
    /// (whatever doesn't fit stays put).
    pub(super) fn workbench_shift_input(&mut self, inv: &mut Inventory) {
        if let Some(stack) = self.workbench_input.take() {
            if let Some(leftover) = inv.add(stack) {
                self.workbench_input = Some(leftover);
            }
        }
    }

    /// Take (craft) the `i`-th offered result: consume `cost` of the input block and
    /// yield the result. A left-click places one craft on the cursor (only if it fits);
    /// shift-click crafts as many as the input + inventory allow, straight into the
    /// inventory. No-op when the input is empty, the i-th recipe doesn't exist, or there
    /// isn't enough input (a greyed result).
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

    /// Remove `cost` items from the workbench input, clearing it when emptied.
    fn consume_workbench_input(&mut self, cost: u8) {
        if let Some(stack) = &mut self.workbench_input {
            stack.count = stack.count.saturating_sub(cost);
            if stack.count == 0 {
                self.workbench_input = None;
            }
        }
    }
}
