use std::collections::HashMap;

use crate::item::{ItemSet, ItemType};

use super::recipe::{CraftingCatalog, IngredientSelector};

struct Gate {
    key: String,
    needs: Vec<ItemSet>,
    early: ItemSet,
}

#[derive(Default)]
pub struct UnlockIndex {
    gates: Vec<Gate>,
    by_item: HashMap<u16, Vec<u32>>,
}

impl UnlockIndex {
    pub fn build(catalog: &CraftingCatalog) -> Self {
        let mut index = Self::default();
        for recipe in catalog.iter() {
            let needs: Vec<ItemSet> = match recipe.unlock_needs() {
                Some(needs) => needs.iter().map(|&need| satisfying_items(need)).collect(),
                None => recipe
                    .ingredients()
                    .iter()
                    .map(|ingredient| satisfying_items(ingredient.selector))
                    .collect(),
            };
            if needs.iter().any(ItemSet::is_empty) {
                continue;
            }
            let gate = index.gates.len() as u32;
            for item in needs
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
                needs,
                early: *recipe.unlock_on(),
            });
        }
        index
    }

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
        self.early.intersects(obtained) || self.needs.iter().all(|mask| mask.intersects(obtained))
    }
}

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
        let mut spruce = ItemSet::EMPTY;
        spruce.insert(ItemType::SprucePlanks);
        spruce.insert(ItemType::Stick);
        assert_eq!(
            index.opened_by(ItemType::SprucePlanks, &spruce),
            vec!["test:tool"],
            "a tag ingredient is satisfied by ANY member"
        );
    }

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
            assert_eq!(
                index.opened_by_all(&obtained).collect::<Vec<_>>(),
                vec!["test:station"]
            );
        }

        let obtained: ItemSet = [ItemType::OakLog, ItemType::Stick].into_iter().collect();
        assert_eq!(
            index.opened_by(ItemType::Stick, &obtained),
            vec!["test:station"]
        );
    }

    #[test]
    fn unlock_needs_replace_the_ingredient_gate() {
        let mut row = recipe(
            "test:easel",
            vec![
                tagged(ItemTag::PLANKS),
                exact(ItemType::Stick),
                exact(ItemType::Diamond),
            ],
            ItemType::CraftingTable,
        );
        row.set_data(vec![(
            "petramond:unlock_needs".into(),
            r#"[{"tag":"petramond:planks"},{"item":"petramond:stick"}]"#.into(),
        )])
        .unwrap();
        let index = UnlockIndex::build(&CraftingCatalog::new(vec![row]));

        let spruce_only: ItemSet = [ItemType::SprucePlanks].into_iter().collect();
        assert!(index.opened_by_all(&spruce_only).next().is_none());
        let needs: ItemSet = [ItemType::SprucePlanks, ItemType::Stick]
            .into_iter()
            .collect();
        assert_eq!(
            index.opened_by(ItemType::Stick, &needs),
            vec!["test:easel"],
            "the needs open the recipe although the diamond was never seen"
        );
        let without_stick: ItemSet = [ItemType::OakPlanks, ItemType::Diamond]
            .into_iter()
            .collect();
        assert!(
            index
                .opened_by(ItemType::Diamond, &without_stick)
                .is_empty(),
            "an ingredient the needs leave out no longer gates the recipe"
        );
    }

    #[test]
    fn invalid_early_unlock_policy_rejects_the_recipe_row() {
        let mut row = recipe(
            "test:station",
            vec![exact(ItemType::OakLog)],
            ItemType::OakPlanks,
        );
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
        let needs = |value: &str| vec![("petramond:unlock_needs".into(), value.into())];
        assert!(row.set_data(needs("[]")).is_err());
        assert!(row
            .set_data(needs(
                r#"[{"item":"petramond:stick","tag":"petramond:planks"}]"#
            ))
            .is_err());
        assert!(row
            .set_data(needs(r#"[{"item":"petramond:missing"}]"#))
            .is_err());
    }
}
