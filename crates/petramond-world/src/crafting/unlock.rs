//! The recipe-unlock index: which recipes a player's discoveries can open.
//!
//! Unlocking itself is EVENT DRIVEN — something happens, a handler decides a
//! recipe is earned, and `Progression::unlock` records it. This module owns
//! the engine's DEFAULT rule, optional row-data early triggers, and the lookup
//! that makes both cheap:
//!
//! > A recipe unlocks once the player has held at least one item satisfying
//! > EVERY one of its ingredients.
//!
//! That single rule is what "meaningful unlocks" reduces to across the whole
//! catalog: an oak log opens oak planks, oak planks open oak stairs/slabs/
//! fences/doors, an iron ingot opens shears, wool opens wool blocks and the
//! bed, planks open the boat. A pack that ships recipes and no unlock policy
//! inherits it, so no recipe can be invisible forever — which is the failure
//! mode of trigger-only policy. A recipe's `petramond:unlock_on` data may name
//! items or tags that reveal it earlier; the default rule remains in force.
//!
//! Each ingredient compiles to an [`ItemSet`] of everything that satisfies it
//! (one item, or a tag's whole membership), so the test is an AND per
//! ingredient rather than a walk of the catalog.

use std::collections::HashMap;

use crate::item::{ItemSet, ItemType};

use super::recipe::{CraftingCatalog, IngredientSelector};

/// One recipe's gate: an early trigger, or every ingredient, opens it.
struct Gate {
    key: String,
    ingredients: Vec<ItemSet>,
    early: ItemSet,
}

/// Catalog-derived reverse index from an obtained item to the recipes it can
/// open. Built once per session from the enabled catalog.
#[derive(Default)]
pub struct UnlockIndex {
    gates: Vec<Gate>,
    /// Item id → gates naming it (directly or through a tag it carries).
    by_item: HashMap<u16, Vec<u32>>,
}

impl UnlockIndex {
    pub fn build(catalog: &CraftingCatalog) -> Self {
        let mut index = Self::default();
        for recipe in catalog.iter() {
            let ingredients: Vec<ItemSet> = recipe
                .ingredients()
                .iter()
                .map(|ingredient| satisfying_items(ingredient.selector))
                .collect();
            // A gate no item can satisfy would never open; that is a broken
            // row (empty tag), already refused at load, so treat it as such
            // rather than shipping a permanently invisible recipe.
            if ingredients.iter().any(ItemSet::is_empty) {
                continue;
            }
            let gate = index.gates.len() as u32;
            for item in ingredients
                .iter()
                .flat_map(ItemSet::iter)
                .chain(recipe.unlock_on().iter())
            {
                let list = index.by_item.entry(item.id()).or_default();
                if list.last() != Some(&gate) {
                    list.push(gate);
                }
            }
            index.gates.push(Gate {
                key: recipe.key().to_owned(),
                ingredients,
                early: *recipe.unlock_on(),
            });
        }
        index
    }

    /// The recipes `obtained` now satisfies that name `item` — what to unlock
    /// when a player obtains it for the first time. Already-unlocked keys are
    /// included; `Progression::unlock` is idempotent and reports the change.
    pub fn opened_by<'a>(&'a self, item: ItemType, obtained: &ItemSet) -> Vec<&'a str> {
        let Some(gates) = self.by_item.get(&item.id()) else {
            return Vec::new();
        };
        gates
            .iter()
            .filter_map(|g| {
                let gate = &self.gates[*g as usize];
                gate.satisfied_by(obtained).then_some(gate.key.as_str())
            })
            .collect()
    }

    /// Every recipe `obtained` satisfies — the catch-up pass a session runs
    /// when a player joins. It is what keeps the record honest across a
    /// catalog change: a pack installed after the player already held the
    /// ingredients still shows up, with no per-item replay.
    pub fn opened_by_all<'a>(&'a self, obtained: &'a ItemSet) -> impl Iterator<Item = &'a str> {
        self.gates
            .iter()
            .filter(|gate| gate.satisfied_by(obtained))
            .map(|gate| gate.key.as_str())
    }
}

impl Gate {
    #[inline]
    fn satisfied_by(&self, obtained: &ItemSet) -> bool {
        self.early.intersects(obtained)
            || self
                .ingredients
                .iter()
                .all(|mask| mask.intersects(obtained))
    }
}

/// Everything that satisfies one ingredient: the named item, or the tag's
/// whole membership.
fn satisfying_items(selector: IngredientSelector) -> ItemSet {
    match selector {
        IngredientSelector::Item(item) => std::iter::once(item).collect(),
        IngredientSelector::Tag(tag) => ItemType::all()
            .iter()
            .copied()
            .filter(|item| item.has_tag(tag))
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crafting::{CraftingIngredient, CraftingRecipe, CraftingStation, IngredientUse};
    use crate::item::{ItemStack, ItemTag};

    fn recipe(key: &str, ingredients: Vec<CraftingIngredient>, result: ItemType) -> CraftingRecipe {
        CraftingRecipe::new(
            key.into(),
            CraftingStation::Inventory,
            ingredients,
            ItemStack::new(result, 1),
        )
    }

    fn exact(item: ItemType) -> CraftingIngredient {
        CraftingIngredient {
            selector: IngredientSelector::Item(item),
            count: 1,
            use_mode: IngredientUse::Consume,
        }
    }

    fn tagged(tag: ItemTag) -> CraftingIngredient {
        CraftingIngredient {
            selector: IngredientSelector::Tag(tag),
            count: 1,
            use_mode: IngredientUse::Consume,
        }
    }

    /// The default rule is CONJUNCTIVE and tag-aware: every ingredient must be
    /// covered before a recipe opens, a tag ingredient is covered by any
    /// member, and a recipe that names an item is never reached by an
    /// unrelated one.
    #[test]
    fn a_recipe_opens_only_once_every_ingredient_is_covered() {
        let catalog = CraftingCatalog::new(vec![
            recipe(
                "test:planks",
                vec![exact(ItemType::OakLog)],
                ItemType::OakPlanks,
            ),
            recipe(
                "test:tool",
                vec![tagged(ItemTag::PLANKS), exact(ItemType::Stick)],
                ItemType::StonePickaxe,
            ),
        ]);
        let index = UnlockIndex::build(&catalog);

        let mut obtained = ItemSet::EMPTY;
        obtained.insert(ItemType::OakLog);
        assert_eq!(
            index.opened_by(ItemType::OakLog, &obtained),
            vec!["test:planks"],
            "a log opens its own planks and nothing else"
        );

        // One of the tool's two ingredients: the gate stays shut.
        obtained.insert(ItemType::OakPlanks);
        assert!(
            index.opened_by(ItemType::OakPlanks, &obtained).is_empty(),
            "planks alone must not open a recipe that also needs sticks"
        );

        obtained.insert(ItemType::Stick);
        assert_eq!(
            index.opened_by(ItemType::Stick, &obtained),
            vec!["test:tool"],
            "the last missing ingredient opens the gate"
        );
        // Any OTHER member of the tag reaches the same gate.
        let mut spruce = ItemSet::EMPTY;
        spruce.insert(ItemType::SprucePlanks);
        spruce.insert(ItemType::Stick);
        assert_eq!(
            index.opened_by(ItemType::SprucePlanks, &spruce),
            vec!["test:tool"],
            "a tag ingredient is satisfied by ANY member"
        );
    }

    /// The join catch-up sees everything the per-item path would have, which
    /// is what makes an installed-later pack recoverable.
    #[test]
    fn the_catch_up_pass_agrees_with_the_per_item_path() {
        let catalog = CraftingCatalog::new(vec![
            recipe(
                "test:planks",
                vec![exact(ItemType::OakLog)],
                ItemType::OakPlanks,
            ),
            recipe(
                "test:shears",
                vec![exact(ItemType::IronIngot)],
                ItemType::Shears,
            ),
        ]);
        let index = UnlockIndex::build(&catalog);
        let obtained: ItemSet = [ItemType::OakLog, ItemType::IronIngot]
            .into_iter()
            .collect();
        let mut all: Vec<&str> = index.opened_by_all(&obtained).collect();
        all.sort_unstable();
        assert_eq!(all, vec!["test:planks", "test:shears"]);
    }

    #[test]
    fn an_early_item_or_tag_opens_a_recipe_without_disabling_the_default_gate() {
        let mut row = recipe(
            "test:station",
            vec![exact(ItemType::OakLog), exact(ItemType::Stick)],
            ItemType::CraftingTable,
        );
        row.set_data(vec![(
            "petramond:unlock_on".into(),
            r#"{"items":["petramond:diamond"],"tags":["petramond:raw_ore"]}"#.into(),
        )])
        .unwrap();
        let index = UnlockIndex::build(&CraftingCatalog::new(vec![row]));

        for early in [ItemType::Diamond, ItemType::RawIron] {
            let obtained = [early].into_iter().collect();
            assert_eq!(index.opened_by(early, &obtained), vec!["test:station"]);
            assert_eq!(index.opened_by_all(&obtained).collect::<Vec<_>>(), vec!["test:station"]);
        }

        let obtained: ItemSet = [ItemType::OakLog, ItemType::Stick].into_iter().collect();
        assert_eq!(index.opened_by(ItemType::Stick, &obtained), vec!["test:station"]);
    }

    #[test]
    fn invalid_early_unlock_policy_rejects_the_recipe_row() {
        let mut row = recipe("test:station", vec![exact(ItemType::OakLog)], ItemType::OakPlanks);
        let data = |value: &str| vec![("petramond:unlock_on".into(), value.into())];
        assert!(row.set_data(data("{}")).is_err());
        assert!(row
            .set_data(data(r#"{"items":["petramond:missing"]}"#))
            .is_err());
        assert!(row
            .set_data(data(r#"{"tags":["petramond:missing"]}"#))
            .is_err());
        assert!(row
            .set_data(data(r#"{"items":["petramond:diamond"],"typo":1}"#))
            .is_err());
    }
}
