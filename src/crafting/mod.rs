mod load;
mod recipe;

#[cfg(test)]
pub use load::load_recipes;
pub use load::load_recipes_for;
pub use recipe::Recipes;
#[cfg(test)]
pub use recipe::{FurnitureRecipe, SmeltingRecipe};

use crate::item::ItemStack;
pub const MAX_GRID: usize = 3;
pub const MAX_CELLS: usize = MAX_GRID * MAX_GRID;
#[derive(Clone, Debug)]
pub struct CraftGrid {
    cells: [Option<ItemStack>; MAX_CELLS],
    cols: usize,
    result: Option<ItemStack>,
}

impl Default for CraftGrid {
    fn default() -> Self {
        Self::new()
    }
}

impl CraftGrid {
    pub fn new() -> Self {
        CraftGrid {
            cells: [None; MAX_CELLS],
            cols: 2,
            result: None,
        }
    }
    #[inline]
    pub fn capacity(&self) -> usize {
        self.cols * self.cols
    }
    #[cfg(test)]
    #[inline]
    pub fn cell(&self, i: usize) -> Option<&ItemStack> {
        if i < self.capacity() {
            self.cells[i].as_ref()
        } else {
            None
        }
    }
    #[inline]
    pub fn cells(&self) -> &[Option<ItemStack>] {
        &self.cells[..self.capacity()]
    }
    #[inline]
    pub fn cell_mut(&mut self, i: usize) -> &mut Option<ItemStack> {
        &mut self.cells[i]
    }
    #[inline]
    pub fn take_cell(&mut self, i: usize) -> Option<ItemStack> {
        self.cells.get_mut(i).and_then(Option::take)
    }
    #[inline]
    pub fn result(&self) -> Option<&ItemStack> {
        self.result.as_ref()
    }
    pub fn reset(&mut self, cols: usize) {
        self.cols = cols.clamp(2, MAX_GRID);
        self.cells = [None; MAX_CELLS];
        self.result = None;
    }
    pub fn recompute(&mut self, recipes: &Recipes) {
        self.result = recipes.find(self.cells(), self.cols);
    }
    pub fn consume_one(&mut self) {
        for cell in self.cells[..self.cols * self.cols].iter_mut() {
            if let Some(stack) = cell {
                stack.count -= 1;
                if stack.count == 0 {
                    *cell = None;
                }
            }
        }
    }
    pub fn take_result(&mut self, recipes: &Recipes, cursor: &mut Option<ItemStack>) {
        let Some(result) = self.result else {
            return;
        };
        let placed = match cursor {
            None => {
                *cursor = Some(result);
                true
            }
            Some(cur) if cur.can_stack_with(&result) && cur.space_left() >= result.count => {
                cur.count += result.count;
                true
            }
            _ => false,
        };
        if placed {
            self.consume_one();
            self.recompute(recipes);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item::ItemType;

    fn recipes() -> Recipes {
        load_recipes()
    }

    #[test]
    fn two_by_two_planks_then_table() {
        let recipes = recipes();
        let mut grid = CraftGrid::new();
        grid.reset(2);
        // One oak log anywhere → 4 oak planks preview.
        *grid.cell_mut(0) = Some(ItemStack::new(ItemType::OakLog, 1));
        grid.recompute(&recipes);
        assert_eq!(
            grid.result().map(|s| (s.item, s.count)),
            Some((ItemType::OakPlanks, 4))
        );

        // Fill the 2×2 with planks → crafting table preview.
        grid.reset(2);
        for i in 0..4 {
            *grid.cell_mut(i) = Some(ItemStack::new(ItemType::OakPlanks, 1));
        }
        grid.recompute(&recipes);
        assert_eq!(grid.result().map(|s| s.item), Some(ItemType::CraftingTable));

        // Consuming one craft empties each occupied cell by one (all were 1).
        grid.consume_one();
        assert!(grid.cells().iter().all(Option::is_none));
        grid.recompute(&recipes);
        assert!(grid.result().is_none());
    }

    #[test]
    fn pickaxe_only_in_three_by_three() {
        let recipes = recipes();
        // Pickaxe layout: planks across the top, sticks down the centre.
        let mut grid = CraftGrid::new();
        grid.reset(3);
        for c in 0..3 {
            *grid.cell_mut(c) = Some(ItemStack::new(ItemType::OakPlanks, 1));
        }
        *grid.cell_mut(4) = Some(ItemStack::new(ItemType::Stick, 1));
        *grid.cell_mut(7) = Some(ItemStack::new(ItemType::Stick, 1));
        grid.recompute(&recipes);
        assert_eq!(grid.result().map(|s| s.item), Some(ItemType::WoodenPickaxe));
    }

    #[test]
    fn reset_changes_size_and_clears() {
        let mut grid = CraftGrid::new();
        grid.reset(3);
        assert_eq!(grid.capacity(), 9);
        *grid.cell_mut(8) = Some(ItemStack::new(ItemType::Stone, 1));
        grid.reset(2);
        assert_eq!(grid.capacity(), 4);
        assert!(grid.cells().iter().all(Option::is_none));
    }
}
