//! Recipe representation + grid matching.
//!
//! A recipe is either *shapeless* (a multiset of ingredients placed anywhere) or
//! *shaped* (a fixed `width×height` pattern matched at any offset within the
//! grid, by bounding box). An [`Ingredient`] is an exact item or a tag (any item
//! in a set, e.g. `#planks`). Matching is pure over a square `cols×cols` grid.

use crate::item::{ItemStack, ItemTag, ItemType};

/// The furnace's processing-recipe class (see [`ProcessingRecipe::class`]).
pub const SMELTING_CLASS: &str = "petramond:smelting";

/// A machine-processing recipe: one `input` item produces `result` when
/// processed by the machine consuming `class` — the furnace smelts
/// [`SMELTING_CLASS`] rows, a mod machine (the kitchen oven) consumes its own
/// namespaced class. Looked up by `(class, input)` (see [`Recipes::process`]).
/// Separate from the grid [`Recipe`]s because it isn't matched over a grid.
#[derive(Clone, Debug)]
pub struct ProcessingRecipe {
    /// Namespaced class key: which machine kind consumes this recipe.
    pub class: String,
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

/// What one craft DOES with the stack in a matched grid cell. The mode belongs
/// to the ingredient OCCURRENCE, not the item: the same item may be consumed
/// by one recipe and retained by another.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum IngredientUse {
    /// Decrement the matched stack by one (the default).
    Consume,
    /// Leave the matched stack untouched — a catalyst (a shovel that grinds
    /// but is not spent). A retained catalyst never bounds shift-crafting.
    Keep,
    /// Decrement by one and return the declared item — a drained container
    /// (a water bucket's empty bucket). A remainder is NEVER deleted: it goes
    /// to the input cell, then the inventory, then the safe overflow drop.
    Remainder(ItemType),
}

/// One recipe-slot requirement plus what the craft transaction does with the
/// stack that matched it.
#[derive(Clone, Debug)]
pub struct RecipeIngredient {
    pub what: Ingredient,
    pub mode: IngredientUse,
}

impl RecipeIngredient {
    /// The default occurrence: consumed on craft.
    pub fn consumed(what: Ingredient) -> Self {
        RecipeIngredient {
            what,
            mode: IngredientUse::Consume,
        }
    }
}

/// A successful grid match: the crafted stack plus, per grid cell, what the
/// craft transaction does with that cell (`None` = empty cell, untouched).
/// Consumption always derives from this plan — never a blind decrement of
/// every occupied cell.
#[derive(Clone, Debug)]
pub struct GridMatch {
    pub result: ItemStack,
    pub uses: [Option<IngredientUse>; super::MAX_CELLS],
}

/// A crafting recipe. `result` is the full output stack (item + yield count).
#[derive(Clone, Debug)]
pub enum Recipe {
    /// Order-independent: the grid's non-empty items, as a multiset, must match
    /// `ingredients` one-to-one.
    Shapeless {
        ingredients: Vec<RecipeIngredient>,
        result: ItemStack,
    },
    /// A `width×height` pattern. `cells` is row-major (`Some` = required
    /// ingredient, `None` = must be empty); it must align with the grid's
    /// occupied bounding box exactly.
    Shaped {
        width: usize,
        height: usize,
        cells: Vec<Option<RecipeIngredient>>,
        result: ItemStack,
    },
}

impl Recipe {
    /// The crafted stack if this recipe is satisfied by `grid` — a `cols×cols`
    /// square in row-major order — else `None`.
    pub fn matches(&self, grid: &[Option<ItemStack>], cols: usize) -> Option<ItemStack> {
        self.match_plan(grid, cols).map(|m| m.result)
    }

    /// [`matches`](Self::matches) plus the per-cell transaction plan.
    pub fn match_plan(&self, grid: &[Option<ItemStack>], cols: usize) -> Option<GridMatch> {
        match self {
            Recipe::Shapeless {
                ingredients,
                result,
            } => Some(GridMatch {
                result: *result,
                uses: match_shapeless(grid, ingredients)?,
            }),
            Recipe::Shaped {
                width,
                height,
                cells,
                result,
            } => Some(GridMatch {
                result: *result,
                uses: match_shaped(grid, cols, *width, *height, cells)?,
            }),
        }
    }
}

/// The loaded recipe set: grid (crafting) recipes searched in declaration order,
/// the machine-processing table looked up by (class, input item), and the
/// furniture-workbench recipes looked up by their input block.
#[derive(Clone, Default)]
pub struct Recipes {
    list: Vec<Recipe>,
    processing: Vec<ProcessingRecipe>,
    furniture: Vec<FurnitureRecipe>,
}

impl Recipes {
    pub fn new(
        list: Vec<Recipe>,
        processing: Vec<ProcessingRecipe>,
        furniture: Vec<FurnitureRecipe>,
    ) -> Self {
        Recipes {
            list,
            processing,
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
        self.find_match(grid, cols).map(|m| m.result)
    }

    /// [`find`](Self::find) plus the matched recipe's per-cell transaction
    /// plan — what taking the result does to each grid cell.
    pub fn find_match(&self, grid: &[Option<ItemStack>], cols: usize) -> Option<GridMatch> {
        self.list.iter().find_map(|r| r.match_plan(grid, cols))
    }

    /// The product `class` machines make from `input`, or `None` if that
    /// class has no recipe for it.
    pub fn process(&self, class: &str, input: ItemType) -> Option<ItemStack> {
        self.processing
            .iter()
            .find(|r| r.class == class && r.input == input)
            .map(|r| r.result)
    }

    /// [`process`](Self::process) for the furnace's [`SMELTING_CLASS`].
    pub fn smelt(&self, input: ItemType) -> Option<ItemStack> {
        self.process(SMELTING_CLASS, input)
    }

    /// Every furniture-workbench recipe whose input is `input`, in declaration order —
    /// the items the workbench offers when that block is placed in it.
    pub fn furniture_for(&self, input: ItemType) -> impl Iterator<Item = &FurnitureRecipe> {
        self.furniture.iter().filter(move |r| r.input == input)
    }
}

/// Shapeless match: the grid's non-empty items map one-to-one onto `ingredients`.
/// Exact-item ingredients are matched before tags so a tag never consumes an item
/// an exact ingredient still needs. On success, each occupied grid cell carries
/// the use mode of the ingredient occurrence it was assigned.
fn match_shapeless(
    grid: &[Option<ItemStack>],
    ingredients: &[RecipeIngredient],
) -> Option<[Option<IngredientUse>; super::MAX_CELLS]> {
    let occupied = grid.iter().flatten().count();
    if occupied == 0 || occupied != ingredients.len() {
        return None;
    }
    let mut used = [false; super::MAX_CELLS];
    let mut uses = [None; super::MAX_CELLS];
    for (cell, stack) in grid.iter().enumerate() {
        let Some(stack) = stack else { continue };
        let it = stack.item;
        let pick = ingredients
            .iter()
            .position(|ing| matches!(ing.what, Ingredient::Item(i) if i == it))
            .filter(|&k| !used[k])
            .or_else(|| {
                ingredients
                    .iter()
                    .enumerate()
                    .find(|(k, ing)| !used[*k] && ing.what.matches(it))
                    .map(|(k, _)| k)
            });
        match pick {
            Some(k) if !used[k] => {
                used[k] = true;
                uses[cell] = Some(ingredients[k].mode);
            }
            _ => return None,
        }
    }
    Some(uses)
}

/// Shaped match: the grid's occupied bounding box must equal the pattern's
/// dimensions, and each pattern cell must agree with the grid (ingredient ↔
/// matching item, blank ↔ empty). On success, each occupied grid cell carries
/// its pattern cell's use mode.
fn match_shaped(
    grid: &[Option<ItemStack>],
    cols: usize,
    width: usize,
    height: usize,
    cells: &[Option<RecipeIngredient>],
) -> Option<[Option<IngredientUse>; super::MAX_CELLS]> {
    let rows = cols; // the grid is always square
    if width == 0 || height == 0 || width > cols || height > rows {
        return None;
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
        return None;
    }
    let mut uses = [None; super::MAX_CELLS];
    for r in 0..height {
        for c in 0..width {
            let grid_idx = (min_r + r) * cols + (min_c + c);
            let ing = &cells[r * width + c];
            match (ing, &grid[grid_idx]) {
                (Some(ing), Some(stack)) => {
                    if !ing.what.matches(stack.item) {
                        return None;
                    }
                    uses[grid_idx] = Some(ing.mode);
                }
                (None, None) => {}
                _ => return None,
            }
        }
    }
    Some(uses)
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

    fn ing(what: Ingredient) -> RecipeIngredient {
        RecipeIngredient::consumed(what)
    }

    fn stick_recipe() -> Recipe {
        Recipe::Shaped {
            width: 1,
            height: 2,
            cells: vec![
                Some(ing(Ingredient::Tag(ItemTag::PLANKS))),
                Some(ing(Ingredient::Tag(ItemTag::PLANKS))),
            ],
            result: ItemStack::new(ItemType::Stick, 4),
        }
    }

    #[test]
    fn shapeless_single_ingredient_matches_anywhere() {
        let r = Recipe::Shapeless {
            ingredients: vec![ing(Ingredient::Item(ItemType::OakLog))],
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
                Some(ing(Ingredient::Tag(ItemTag::PLANKS))),
                Some(ing(Ingredient::Tag(ItemTag::PLANKS))),
                Some(ing(Ingredient::Tag(ItemTag::PLANKS))),
                None,
                Some(ing(Ingredient::Item(ItemType::Stick))),
                None,
                None,
                Some(ing(Ingredient::Item(ItemType::Stick))),
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
