//! Recipe representation + grid matching.
//!
//! A recipe is either *shapeless* (a multiset of ingredients placed anywhere) or
//! *shaped* (a fixed `width×height` pattern matched at any offset within the
//! grid, by bounding box). An [`Ingredient`] is an exact item or a tag (any item
//! in a set, e.g. `#planks`). Matching is pure over a square `cols×cols` grid.

use crate::item::{ItemStack, ItemTag, ItemType};

/// A smelting recipe: one `input` item produces `result` when smelted in a
/// furnace. Looked up by input item (see [`Recipes::smelt`]). Separate from the
/// grid [`Recipe`]s because it isn't matched over a crafting grid.
#[derive(Clone, Copy, Debug)]
pub struct SmeltingRecipe {
    pub input: ItemType,
    pub result: ItemStack,
}

/// A furniture-workbench recipe: placing `cost` of `input` in the workbench lets you
/// craft `result`. Looked up by the input item (see [`Recipes::furniture_for`]): the
/// workbench shows every result whose `input` matches the placed block, each greyed
/// out until at least `cost` of it is present. Separate from the grid [`Recipe`]s
/// because the workbench takes a single block, not a grid.
#[derive(Clone, Copy, Debug)]
pub struct FurnitureRecipe {
    pub input: ItemType,
    pub result: ItemStack,
    /// How many `input` items one craft consumes.
    pub cost: u8,
}

/// One cell's requirement: an exact item, or any item carrying a tag (tag
/// membership is defined in item data — see [`ItemType::has_tag`]).
#[derive(Clone, Debug)]
pub enum Ingredient {
    Item(ItemType),
    Tag(ItemTag),
}

impl Ingredient {
    /// Whether `item` satisfies this ingredient.
    #[inline]
    pub fn matches(&self, item: ItemType) -> bool {
        match self {
            Ingredient::Item(i) => *i == item,
            Ingredient::Tag(tag) => item.has_tag(*tag),
        }
    }
}

/// A crafting recipe. `result` is the full output stack (item + yield count).
#[derive(Clone, Debug)]
pub enum Recipe {
    /// Order-independent: the grid's non-empty items, as a multiset, must match
    /// `ingredients` one-to-one.
    Shapeless {
        ingredients: Vec<Ingredient>,
        result: ItemStack,
    },
    /// A `width×height` pattern. `cells` is row-major (`Some` = required
    /// ingredient, `None` = must be empty); it must align with the grid's
    /// occupied bounding box exactly.
    Shaped {
        width: usize,
        height: usize,
        cells: Vec<Option<Ingredient>>,
        result: ItemStack,
    },
}

impl Recipe {
    /// The crafted stack if this recipe is satisfied by `grid` — a `cols×cols`
    /// square in row-major order — else `None`.
    pub fn matches(&self, grid: &[Option<ItemStack>], cols: usize) -> Option<ItemStack> {
        match self {
            Recipe::Shapeless {
                ingredients,
                result,
            } => match_shapeless(grid, ingredients).then_some(*result),
            Recipe::Shaped {
                width,
                height,
                cells,
                result,
            } => match_shaped(grid, cols, *width, *height, cells).then_some(*result),
        }
    }
}

/// The loaded recipe set: grid (crafting) recipes searched in declaration order,
/// the smelting table looked up by input item, and the furniture-workbench recipes
/// looked up by their input block.
#[derive(Default)]
pub struct Recipes {
    list: Vec<Recipe>,
    smelting: Vec<SmeltingRecipe>,
    furniture: Vec<FurnitureRecipe>,
}

impl Recipes {
    pub fn new(
        list: Vec<Recipe>,
        smelting: Vec<SmeltingRecipe>,
        furniture: Vec<FurnitureRecipe>,
    ) -> Self {
        Recipes {
            list,
            smelting,
            furniture,
        }
    }

    /// Number of grid (crafting) recipes.
    pub fn len(&self) -> usize {
        self.list.len()
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    /// The result of the first grid recipe that matches `grid` (a `cols×cols`
    /// square), or `None` if nothing matches.
    pub fn find(&self, grid: &[Option<ItemStack>], cols: usize) -> Option<ItemStack> {
        self.list.iter().find_map(|r| r.matches(grid, cols))
    }

    /// The smelted product of `input`, or `None` if it has no smelting recipe.
    pub fn smelt(&self, input: ItemType) -> Option<ItemStack> {
        self.smelting
            .iter()
            .find(|r| r.input == input)
            .map(|r| r.result)
    }

    /// Every furniture-workbench recipe whose input is `input`, in declaration order —
    /// the items the workbench offers when that block is placed in it.
    pub fn furniture_for(&self, input: ItemType) -> impl Iterator<Item = &FurnitureRecipe> {
        self.furniture.iter().filter(move |r| r.input == input)
    }
}

/// Shapeless match: the grid's non-empty items map one-to-one onto `ingredients`.
/// Exact-item ingredients are matched before tags so a tag never consumes an item
/// an exact ingredient still needs.
fn match_shapeless(grid: &[Option<ItemStack>], ingredients: &[Ingredient]) -> bool {
    let items: Vec<ItemType> = grid.iter().flatten().map(|s| s.item).collect();
    if items.is_empty() || items.len() != ingredients.len() {
        return false;
    }
    let mut used = [false; super::MAX_CELLS];
    for &it in &items {
        let pick = ingredients
            .iter()
            .position(|ing| matches!(ing, Ingredient::Item(i) if *i == it))
            .filter(|&k| !used[k])
            .or_else(|| {
                ingredients
                    .iter()
                    .enumerate()
                    .find(|(k, ing)| !used[*k] && ing.matches(it))
                    .map(|(k, _)| k)
            });
        match pick {
            Some(k) if !used[k] => used[k] = true,
            _ => return false,
        }
    }
    true
}

/// Shaped match: the grid's occupied bounding box must equal the pattern's
/// dimensions, and each pattern cell must agree with the grid (ingredient ↔
/// matching item, blank ↔ empty).
fn match_shaped(
    grid: &[Option<ItemStack>],
    cols: usize,
    width: usize,
    height: usize,
    cells: &[Option<Ingredient>],
) -> bool {
    let rows = cols; // the grid is always square
    if width == 0 || height == 0 || width > cols || height > rows {
        return false;
    }
    // Bounding box of the occupied cells.
    let (mut min_r, mut min_c, mut max_r, mut max_c) = (rows, cols, 0usize, 0usize);
    let mut any = false;
    for r in 0..rows {
        for c in 0..cols {
            if grid[r * cols + c].is_some() {
                any = true;
                min_r = min_r.min(r);
                max_r = max_r.max(r);
                min_c = min_c.min(c);
                max_c = max_c.max(c);
            }
        }
    }
    if !any || (max_c - min_c + 1) != width || (max_r - min_r + 1) != height {
        return false;
    }
    for r in 0..height {
        for c in 0..width {
            let cell = &grid[(min_r + r) * cols + (min_c + c)];
            let ing = &cells[r * width + c];
            match (ing, cell) {
                (Some(ing), Some(stack)) => {
                    if !ing.matches(stack.item) {
                        return false;
                    }
                }
                (None, None) => {}
                _ => return false,
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid_from(items: &[Option<ItemType>], cols: usize) -> Vec<Option<ItemStack>> {
        items
            .iter()
            .map(|o| o.map(|i| ItemStack::new(i, 1)))
            .chain(std::iter::repeat(None))
            .take(cols * cols)
            .collect()
    }

    fn stick_recipe() -> Recipe {
        Recipe::Shaped {
            width: 1,
            height: 2,
            cells: vec![
                Some(Ingredient::Tag(ItemTag::Planks)),
                Some(Ingredient::Tag(ItemTag::Planks)),
            ],
            result: ItemStack::new(ItemType::Stick, 4),
        }
    }

    #[test]
    fn shapeless_single_ingredient_matches_anywhere() {
        let r = Recipe::Shapeless {
            ingredients: vec![Ingredient::Item(ItemType::OakLog)],
            result: ItemStack::new(ItemType::OakPlanks, 4),
        };
        // One oak log in any of the four 2×2 cells crafts planks.
        for slot in 0..4 {
            let mut items = [None; 4];
            items[slot] = Some(ItemType::OakLog);
            let g = grid_from(&items, 2);
            assert_eq!(
                r.matches(&g, 2),
                Some(ItemStack::new(ItemType::OakPlanks, 4))
            );
        }
        // Two logs (extra ingredient) does NOT match the one-log recipe.
        let g = grid_from(
            &[Some(ItemType::OakLog), Some(ItemType::OakLog), None, None],
            2,
        );
        assert_eq!(r.matches(&g, 2), None);
    }

    #[test]
    fn shaped_stick_fits_any_column_of_two() {
        let r = stick_recipe();
        // Vertical pair in the left column of a 2×2.
        let g = grid_from(
            &[
                Some(ItemType::OakPlanks),
                None,
                Some(ItemType::OakPlanks),
                None,
            ],
            2,
        );
        assert_eq!(r.matches(&g, 2), Some(ItemStack::new(ItemType::Stick, 4)));
        // The same shape fits anywhere in a 3×3 via the bounding box.
        let mut items = [None; 9];
        items[1] = Some(ItemType::SprucePlanks);
        items[4] = Some(ItemType::SprucePlanks);
        let g = grid_from(&items, 3);
        assert_eq!(r.matches(&g, 3), Some(ItemStack::new(ItemType::Stick, 4)));
        // A horizontal pair must NOT match the vertical pattern.
        let g = grid_from(
            &[
                Some(ItemType::OakPlanks),
                Some(ItemType::OakPlanks),
                None,
                None,
            ],
            2,
        );
        assert_eq!(r.matches(&g, 2), None);
    }

    #[test]
    fn shaped_pickaxe_requires_exact_layout_and_tag_mixing() {
        let r = Recipe::Shaped {
            width: 3,
            height: 3,
            cells: vec![
                Some(Ingredient::Tag(ItemTag::Planks)),
                Some(Ingredient::Tag(ItemTag::Planks)),
                Some(Ingredient::Tag(ItemTag::Planks)),
                None,
                Some(Ingredient::Item(ItemType::Stick)),
                None,
                None,
                Some(Ingredient::Item(ItemType::Stick)),
                None,
            ],
            result: ItemStack::new(ItemType::WoodenPickaxe, 1),
        };
        // Top row mixes plank types (tag), sticks down the centre.
        let g = grid_from(
            &[
                Some(ItemType::OakPlanks),
                Some(ItemType::SprucePlanks),
                Some(ItemType::OakPlanks),
                None,
                Some(ItemType::Stick),
                None,
                None,
                Some(ItemType::Stick),
                None,
            ],
            3,
        );
        assert_eq!(
            r.matches(&g, 3),
            Some(ItemStack::new(ItemType::WoodenPickaxe, 1))
        );
        // A pickaxe pattern is 3 wide: it can NEVER match a 2×2 grid.
        let g2 = grid_from(&[Some(ItemType::OakPlanks); 4], 2);
        assert_eq!(r.matches(&g2, 2), None);
    }

    #[test]
    fn empty_grid_matches_nothing() {
        let r = stick_recipe();
        assert_eq!(r.matches(&grid_from(&[None; 4], 2), 2), None);
    }
}
