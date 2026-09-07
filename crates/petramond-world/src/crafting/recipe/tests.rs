use super::*;

#[test]
fn joined_catalog_round_trips_namespaced_selectors_and_stable_keys() {
    let recipe = CraftingRecipe::new(
        "test:sticks".into(),
        CraftingStation::Inventory,
        vec![CraftingIngredient {
            selector: IngredientSelector::Tag(ItemTag::PLANKS),
            count: 2,
            use_mode: IngredientUse::Consume,
        }],
        ItemStack::new(ItemType::Stick, 4),
    );
    let bench = CraftingStation::from_key("test:bench").expect("mod station registers");
    let bench_recipe = CraftingRecipe::new(
        "test:bench_sticks".into(),
        bench,
        vec![CraftingIngredient {
            selector: IngredientSelector::Tag(ItemTag::PLANKS),
            count: 2,
            use_mode: IngredientUse::Consume,
        }],
        ItemStack::new(ItemType::Stick, 4),
    );
    let catalog = CraftingCatalog::new(vec![recipe, bench_recipe]);
    let restored = CraftingCatalog::from_data(catalog.to_data());
    let sticks = restored.get("test:sticks").expect("stable key lookup");
    assert_eq!(sticks.station(), CraftingStation::Inventory);
    assert_eq!(sticks.ingredients()[0].count, 2);
    assert_eq!(
        sticks.ingredients()[0].selector,
        IngredientSelector::Tag(ItemTag::PLANKS)
    );
    // A pack station survives the joined round trip by key.
    let bench_sticks = restored.get("test:bench_sticks").expect("mod station row");
    assert_eq!(bench_sticks.station(), bench);
}

#[test]
fn joined_catalog_rejects_invalid_identity_capacity_and_empty_items() {
    let data =
        |recipe: &str, ingredient: CraftingIngredientData, result: &str| CraftingRecipeData {
            data: Vec::new(),
            recipe: recipe.into(),
            station: CraftingStation::INVENTORY_KEY.into(),
            ingredients: vec![ingredient],
            result: CraftingStackData {
                item: result.into(),
                count: 1,
            },
        };
    let coal = |count| CraftingIngredientData {
        selector: CraftingSelectorData::Item(ItemType::Coal.key().into()),
        count,
        use_mode: IngredientUseData::Consume,
    };

    assert!(CraftingRecipe::from_data(data("bare", coal(1), ItemType::Stick.key())).is_err());
    assert!(CraftingRecipe::from_data(data(
        "test:too_large",
        coal((MAX_INGREDIENT_UNITS + 1) as u16),
        ItemType::Stick.key(),
    ))
    .is_err());
    assert!(
        CraftingRecipe::from_data(data("test:air_result", coal(1), ItemType::Air.key(),)).is_err()
    );
    assert!(CraftingRecipe::from_data(data(
        "test:air_ingredient",
        CraftingIngredientData {
            selector: CraftingSelectorData::Item(ItemType::Air.key().into()),
            count: 1,
            use_mode: IngredientUseData::Consume,
        },
        ItemType::Stick.key(),
    ))
    .is_err());
    assert!(CraftingRecipe::from_data(data(
        "test:air_remainder",
        CraftingIngredientData {
            selector: CraftingSelectorData::Item(ItemType::Coal.key().into()),
            count: 1,
            use_mode: IngredientUseData::Remainder(ItemType::Air.key().into()),
        },
        ItemType::Stick.key(),
    ))
    .is_err());
}
