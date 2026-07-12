mod crafting;
mod dispatch;
mod furnace;
mod generic;
mod state;
mod target;
mod workbench;

pub(crate) use state::ContainerMenu;
pub(crate) use target::ContainerTarget;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::Block;
    use crate::controls::PointerButton;
    use crate::crafting::Recipes;
    use crate::facing::Facing;
    use crate::gui::{MenuSlot, WorkbenchHit};
    use crate::inventory::Inventory;
    use crate::item::{ItemStack, ItemType};
    use crate::mathh::IVec3;
    use crate::world::World;
    fn world_with_empty_chunk() -> World {
        let mut world = World::new(1, 1);
        let pos = crate::chunk::ChunkPos::new(0, 0);
        world.clear_world();
        world.insert_chunk_for_test(pos, crate::chunk::Chunk::new(0, 0));
        world
    }

    fn recipes() -> Recipes {
        crate::crafting::load_recipes()
    }

    fn count_item(inv: &Inventory, item: ItemType) -> u32 {
        (0..crate::inventory::TOTAL_SLOTS)
            .filter_map(|i| inv.slot(i))
            .filter(|s| s.item == item)
            .map(|s| s.count as u32)
            .sum()
    }
    fn place_in_craft_cell(
        menu: &mut ContainerMenu,
        inv: &mut Inventory,
        recipes: &Recipes,
        cell: usize,
        stack: ItemStack,
    ) {
        inv.add(stack);
        inv.click_slot(0); // pick the stack onto the cursor
        menu.craft_click_slot(inv, recipes, cell); // drop it into the craft cell
    }

    #[test]
    fn crafting_planks_from_log_via_result_slot() {
        let recipes = recipes();
        let mut menu = ContainerMenu::new();
        let mut inv = Inventory::new();
        menu.open_crafting(2, &recipes);
        place_in_craft_cell(
            &mut menu,
            &mut inv,
            &recipes,
            0,
            ItemStack::new(ItemType::OakLog, 1),
        );
        assert_eq!(
            menu.craft_grid().result().map(|s| (s.item, s.count)),
            Some((ItemType::OakPlanks, 4))
        );
        // Take the result: 4 planks onto the cursor, the log consumed, no result.
        menu.craft_take_result(&mut inv, &recipes, |s| panic!("unexpected overflow: {s:?}"));
        assert_eq!(
            inv.cursor().map(|s| (s.item, s.count)),
            Some((ItemType::OakPlanks, 4))
        );
        assert!(menu.craft_grid().result().is_none());
        assert!(menu.craft_grid().cells().iter().all(Option::is_none));
    }

    #[test]
    fn shift_crafting_consumes_every_log_in_the_cell() {
        let recipes = recipes();
        let mut menu = ContainerMenu::new();
        let mut inv = Inventory::new();
        menu.open_crafting(2, &recipes);
        // A cell holding 3 logs shift-crafts three times (one log per craft).
        place_in_craft_cell(
            &mut menu,
            &mut inv,
            &recipes,
            0,
            ItemStack::new(ItemType::OakLog, 3),
        );
        menu.craft_shift_result(&mut inv, &recipes, |s| panic!("unexpected overflow: {s:?}"));
        assert!(
            menu.craft_grid().cells().iter().all(Option::is_none),
            "all logs consumed"
        );
        assert_eq!(count_item(&inv, ItemType::OakPlanks), 12);
    }

    /// The transactional ingredient modes: a `keep` catalyst survives normal
    /// AND shift crafting (which stays bounded by the CONSUMED ingredient),
    /// and a `remainder` occurrence returns its declared item — to the input
    /// cell when it empties, else to the inventory — never deleted.
    #[test]
    fn craft_transaction_keeps_catalysts_and_returns_remainders() {
        use crate::crafting::{Ingredient, IngredientUse, Recipe, RecipeIngredient};
        use crate::item::ItemTag;
        let recipes = Recipes::new(
            vec![
                // 1 coal ground by any retained #shovels catalyst → 2 sticks.
                Recipe::Shapeless {
                    ingredients: vec![
                        RecipeIngredient::consumed(Ingredient::Item(ItemType::Coal)),
                        RecipeIngredient {
                            what: Ingredient::Tag(ItemTag::SHOVELS),
                            mode: IngredientUse::Keep,
                        },
                    ],
                    result: ItemStack::new(ItemType::Stick, 2),
                },
                // 1 water bucket → 1 glass, the drained bucket returned.
                Recipe::Shapeless {
                    ingredients: vec![RecipeIngredient {
                        what: Ingredient::Item(ItemType::WaterBucket),
                        mode: IngredientUse::Remainder(ItemType::WoodenBucket),
                    }],
                    result: ItemStack::new(ItemType::Glass, 1),
                },
                // A STACKABLE ingredient with a remainder, for the
                // occupied-cell fallback scenario below.
                Recipe::Shapeless {
                    ingredients: vec![RecipeIngredient {
                        what: Ingredient::Item(ItemType::Coal),
                        mode: IngredientUse::Remainder(ItemType::Stick),
                    }],
                    result: ItemStack::new(ItemType::Glass, 1),
                },
            ],
            Vec::new(),
            Vec::new(),
        );

        // Catalyst: shift-crafting 5 coal + 1 shovel crafts exactly 5 times.
        let mut menu = ContainerMenu::new();
        let mut inv = Inventory::new();
        menu.open_crafting(2, &recipes);
        place_in_craft_cell(
            &mut menu,
            &mut inv,
            &recipes,
            0,
            ItemStack::new(ItemType::Coal, 5),
        );
        place_in_craft_cell(
            &mut menu,
            &mut inv,
            &recipes,
            1,
            ItemStack::new(ItemType::IronShovel, 1),
        );
        menu.craft_shift_result(&mut inv, &recipes, |s| panic!("unexpected overflow: {s:?}"));
        assert_eq!(count_item(&inv, ItemType::Stick), 10, "5 crafts, 2 each");
        assert_eq!(
            menu.craft_grid().cell(1).map(|s| (s.item, s.count)),
            Some((ItemType::IronShovel, 1)),
            "the retained catalyst survives shift-crafting untouched"
        );
        assert!(menu.craft_grid().cell(0).is_none(), "the coal is spent");

        // Remainder into the EMPTIED cell: one water bucket crafts once and
        // leaves the wooden bucket in its own cell (which also stops the
        // recipe from matching again).
        let mut menu = ContainerMenu::new();
        let mut inv = Inventory::new();
        menu.open_crafting(2, &recipes);
        place_in_craft_cell(
            &mut menu,
            &mut inv,
            &recipes,
            0,
            ItemStack::new(ItemType::WaterBucket, 1),
        );
        menu.craft_take_result(&mut inv, &recipes, |s| panic!("unexpected overflow: {s:?}"));
        assert_eq!(
            inv.cursor().map(|s| s.item),
            Some(ItemType::Glass),
            "the result landed on the cursor"
        );
        assert_eq!(
            menu.craft_grid().cell(0).map(|s| (s.item, s.count)),
            Some((ItemType::WoodenBucket, 1)),
            "the drained bucket returns to the consumed cell"
        );
        assert!(
            menu.craft_grid().result().is_none(),
            "the empty bucket does not satisfy the recipe again"
        );

        // Remainder with an OCCUPIED cell falls back to the inventory.
        let mut menu = ContainerMenu::new();
        let mut inv = Inventory::new();
        menu.open_crafting(2, &recipes);
        place_in_craft_cell(
            &mut menu,
            &mut inv,
            &recipes,
            0,
            ItemStack::new(ItemType::Coal, 2),
        );
        menu.craft_take_result(&mut inv, &recipes, |s| panic!("unexpected overflow: {s:?}"));
        assert_eq!(
            menu.craft_grid().cell(0).map(|s| (s.item, s.count)),
            Some((ItemType::Coal, 1)),
            "one coal consumed, the other still occupies the cell"
        );
        assert_eq!(
            count_item(&inv, ItemType::Stick),
            1,
            "the remainder went to the inventory instead"
        );
    }

    #[test]
    fn closing_crafting_returns_grid_items_to_inventory() {
        let recipes = recipes();
        let mut menu = ContainerMenu::new();
        let mut inv = Inventory::new();
        menu.open_crafting(3, &recipes);
        place_in_craft_cell(
            &mut menu,
            &mut inv,
            &recipes,
            4,
            ItemStack::new(ItemType::OakLog, 5),
        );
        assert!(inv.cursor().is_none());
        menu.close_crafting(&mut inv, &recipes, |_| panic!("nothing should overflow"));
        assert_eq!(count_item(&inv, ItemType::OakLog), 5);
        assert!(menu.craft_grid().cell(4).is_none());
    }

    #[test]
    fn furnace_shift_routes_fuel_and_smeltable_to_their_slots() {
        let mut world = world_with_empty_chunk();
        let mut menu = ContainerMenu::new();
        let pos = IVec3::new(2, 64, 2);
        world.set_block_world(pos.x, pos.y, pos.z, Block::Furnace);
        world.insert_furnace(pos, Facing::North);
        menu.open_furnace_screen(&mut world, pos);

        // Hotbar: coal (slot 0), raw iron (slot 1), oak planks (slot 2 — neither tag).
        let mut inv = Inventory::new();
        inv.add(ItemStack::new(ItemType::Coal, 5));
        inv.add(ItemStack::new(ItemType::RawIron, 3));
        inv.add(ItemStack::new(ItemType::OakPlanks, 4));

        // Coal -> fuel slot.
        menu.container_shift_from_inventory(&mut world, &mut inv, 0);
        assert!(inv.slot(0).is_none(), "coal left the inventory");
        assert_eq!(
            world.container_at(pos).unwrap().slots[crate::furnace::SLOT_FUEL],
            Some(ItemStack::new(ItemType::Coal, 5)),
            "coal went to the fuel slot"
        );

        // Raw iron -> input slot.
        menu.container_shift_from_inventory(&mut world, &mut inv, 1);
        assert!(inv.slot(1).is_none(), "raw iron left the inventory");
        assert_eq!(
            world.container_at(pos).unwrap().slots[crate::furnace::SLOT_INPUT],
            Some(ItemStack::new(ItemType::RawIron, 3)),
            "raw iron went to the input slot"
        );

        // A non-fuel, non-smeltable item is not pulled into the furnace; it falls
        // back to the ordinary hotbar->main-grid shuffle.
        menu.container_shift_from_inventory(&mut world, &mut inv, 2);
        assert!(inv.slot(2).is_none(), "plank moved out of the hotbar slot");
        let c = world.container_at(pos).unwrap();
        for slot in &c.slots {
            assert_ne!(slot.map(|s| s.item), Some(ItemType::OakPlanks));
        }
        // It landed in the main grid (first slot of the 27-slot region).
        assert_eq!(
            inv.slot(crate::inventory::HOTBAR_LEN).map(|s| s.item),
            Some(ItemType::OakPlanks),
        );
    }

    #[test]
    fn furnace_shift_merges_into_a_partly_filled_slot() {
        let mut world = world_with_empty_chunk();
        let mut menu = ContainerMenu::new();
        let pos = IVec3::new(3, 64, 3);
        world.set_block_world(pos.x, pos.y, pos.z, Block::Furnace);
        world.insert_furnace(pos, Facing::North);
        // Seed the fuel slot with some coal already.
        world.container_at_mut(pos).unwrap().slots[crate::furnace::SLOT_FUEL] =
            Some(ItemStack::new(ItemType::Coal, 60));
        menu.open_furnace_screen(&mut world, pos);

        let mut inv = Inventory::new();
        inv.add(ItemStack::new(ItemType::Coal, 10));
        menu.container_shift_from_inventory(&mut world, &mut inv, 0);

        // 4 top up the fuel slot to 64; the remaining 6 stay in the inventory.
        assert_eq!(
            world.container_at(pos).unwrap().slots[crate::furnace::SLOT_FUEL]
                .unwrap()
                .count,
            64
        );
        assert_eq!(inv.slot(0).map(|s| s.count), Some(6));
    }

    #[test]
    fn container_shift_merges_before_opening_an_empty_slot() {
        // Regression: the routed shift-in must top up a matching stack even
        // when an EARLIER slot is empty — index-order routing used to drop
        // the stack into the empty slot 0 and fragment the pile.
        let mut world = world_with_empty_chunk();
        let mut menu = ContainerMenu::new();
        let pos = IVec3::new(4, 64, 4);
        world.set_block_world(pos.x, pos.y, pos.z, Block::Chest);
        world.insert_chest(pos, Facing::North);
        world.container_at_mut(pos).unwrap().slots[1] = Some(ItemStack::new(ItemType::Coal, 30));
        menu.open_chest_screen(&mut world, pos);

        let mut inv = Inventory::new();
        inv.add(ItemStack::new(ItemType::Coal, 10));
        menu.container_shift_from_inventory(&mut world, &mut inv, 0);

        let c = world.container_at(pos).unwrap();
        assert_eq!(c.slots[0], None, "no new stack opened while one can merge");
        assert_eq!(c.slots[1], Some(ItemStack::new(ItemType::Coal, 40)));
        assert!(inv.slot(0).is_none(), "the whole shifted stack moved");
    }
    fn place_in_workbench_input(
        menu: &mut ContainerMenu,
        world: &mut World,
        inv: &mut Inventory,
        recipes: &Recipes,
        stack: ItemStack,
    ) {
        inv.add(stack);
        inv.click_slot(0); // pick onto cursor
        menu.click(
            world,
            inv,
            recipes,
            MenuSlot::Workbench(WorkbenchHit::Input),
            PointerButton::Primary,
            false,
            false,
            |s| panic!("unexpected craft overflow: {s:?}"),
        );
    }

    #[test]
    fn workbench_offers_a_door_for_planks_and_crafting_consumes_input() {
        let recipes = recipes(); // the shipped recipes incl. plank → door
        let mut world = world_with_empty_chunk();
        let mut menu = ContainerMenu::new();
        let mut inv = Inventory::new();
        menu.open_workbench();
        // Empty input offers nothing.
        assert!(menu
            .open_workbench_view(&recipes)
            .unwrap()
            .results
            .is_empty());

        // Three oak planks in → oak door offered and craftable (cost 1).
        place_in_workbench_input(
            &mut menu,
            &mut world,
            &mut inv,
            &recipes,
            ItemStack::new(ItemType::OakPlanks, 3),
        );
        let view = menu.open_workbench_view(&recipes).unwrap();
        assert_eq!(view.input.map(|s| s.count), Some(3));
        assert!(
            view.results
                .iter()
                .any(|&(it, ok)| it == ItemType::OakDoor && ok),
            "oak door offered + craftable"
        );

        // Craft the first result: a door onto the cursor, one plank consumed.
        menu.click(
            &mut world,
            &mut inv,
            &recipes,
            MenuSlot::Workbench(WorkbenchHit::Result(0)),
            PointerButton::Primary,
            false,
            false,
            |s| panic!("unexpected craft overflow: {s:?}"),
        );
        assert_eq!(
            inv.cursor().map(|s| (s.item, s.count)),
            Some((ItemType::OakDoor, 1))
        );
        assert_eq!(
            menu.open_workbench_view(&recipes)
                .unwrap()
                .input
                .unwrap()
                .count,
            2
        );
    }

    #[test]
    fn shift_clicking_inventory_planks_fills_the_workbench_input() {
        let recipes = recipes();
        let mut world = world_with_empty_chunk();
        let mut menu = ContainerMenu::new();
        let mut inv = Inventory::new();
        menu.open_workbench();
        inv.add(ItemStack::new(ItemType::OakPlanks, 5));

        // Shift-click the inventory slot: the planks (a furniture input) jump to the input.
        menu.click(
            &mut world,
            &mut inv,
            &recipes,
            MenuSlot::Inventory(0),
            PointerButton::Primary,
            true,
            false,
            |s| panic!("unexpected craft overflow: {s:?}"),
        );
        assert_eq!(
            menu.open_workbench_view(&recipes)
                .unwrap()
                .input
                .map(|s| (s.item, s.count)),
            Some((ItemType::OakPlanks, 5)),
            "the whole plank stack moved into the input",
        );
        assert!(inv.slot(0).is_none(), "the inventory slot emptied");
    }

    #[test]
    fn shift_clicking_a_non_input_item_does_not_fill_the_workbench() {
        // A stick has no furniture recipe, so shift-click must NOT dump it in the input —
        // it falls back to the ordinary hotbar↔grid move instead.
        let recipes = recipes();
        let mut world = world_with_empty_chunk();
        let mut menu = ContainerMenu::new();
        let mut inv = Inventory::new();
        menu.open_workbench();
        inv.add(ItemStack::new(ItemType::Stick, 4));
        menu.click(
            &mut world,
            &mut inv,
            &recipes,
            MenuSlot::Inventory(0),
            PointerButton::Primary,
            true,
            false,
            |s| panic!("unexpected craft overflow: {s:?}"),
        );
        assert!(
            menu.open_workbench_view(&recipes).unwrap().input.is_none(),
            "a non-input item never lands in the workbench input",
        );
    }

    #[test]
    fn workbench_greys_out_and_refuses_a_result_below_its_cost() {
        // A recipe that needs 6 planks: with fewer it shows greyed and won't craft.
        let recipes = Recipes::new(
            Vec::new(),
            Vec::new(),
            vec![crate::crafting::FurnitureRecipe {
                input: ItemType::OakPlanks,
                result: ItemStack::new(ItemType::OakDoor, 1),
                cost: 6,
            }],
        );
        let mut world = world_with_empty_chunk();
        let mut menu = ContainerMenu::new();
        let mut inv = Inventory::new();
        menu.open_workbench();
        place_in_workbench_input(
            &mut menu,
            &mut world,
            &mut inv,
            &recipes,
            ItemStack::new(ItemType::OakPlanks, 3),
        );
        // Offered but greyed (3 < 6).
        let view = menu.open_workbench_view(&recipes).unwrap();
        assert_eq!(view.results, vec![(ItemType::OakDoor, false)]);
        // Clicking a greyed result does nothing: no door, input untouched.
        menu.click(
            &mut world,
            &mut inv,
            &recipes,
            MenuSlot::Workbench(WorkbenchHit::Result(0)),
            PointerButton::Primary,
            false,
            false,
            |s| panic!("unexpected craft overflow: {s:?}"),
        );
        assert!(inv.cursor().is_none(), "no craft below cost");
        assert_eq!(
            menu.open_workbench_view(&recipes)
                .unwrap()
                .input
                .unwrap()
                .count,
            3
        );
    }
}
