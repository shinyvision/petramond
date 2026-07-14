mod crafting;
mod dispatch;
mod furnace;
mod generic;
mod state;
mod target;
mod transport;
mod workbench;

pub(crate) use crafting::CraftMenuFailure;
pub(crate) use state::ContainerMenu;
pub(crate) use target::ContainerTarget;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::Block;
    use crate::controls::PointerButton;
    use crate::crafting::{
        CraftingIngredient, CraftingRecipe, CraftingStation, IngredientSelector, IngredientUse,
        Recipes,
    };
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

    fn player_crafting_recipes(station: CraftingStation) -> Recipes {
        Recipes::new(
            vec![CraftingRecipe::new(
                "test:coal_to_sticks".into(),
                station,
                vec![CraftingIngredient {
                    selector: IngredientSelector::Item(ItemType::Coal),
                    count: 1,
                    use_mode: IngredientUse::Consume,
                }],
                ItemStack::new(ItemType::Stick, 2),
            )],
            Vec::new(),
            Vec::new(),
        )
    }

    #[test]
    fn craft_button_commits_one_stack_then_output_is_taken() {
        let recipes = player_crafting_recipes(CraftingStation::Inventory);
        let mut menu = ContainerMenu::new();
        let mut inv = Inventory::new();
        inv.add(ItemStack::new(ItemType::Coal, 2));
        menu.open_crafting(CraftingStation::Inventory);

        assert_eq!(
            menu.craft_recipe(&mut inv, &recipes, "test:coal_to_sticks", false),
            Ok(Vec::new())
        );
        assert_eq!(
            menu.craft_output(),
            Some(ItemStack::new(ItemType::Stick, 2))
        );
        assert_eq!(
            inv.slot(0).copied(),
            Some(ItemStack::new(ItemType::Coal, 1))
        );

        menu.craft_take_output(&mut inv, PointerButton::Primary, false);
        assert_eq!(
            inv.cursor().copied(),
            Some(ItemStack::new(ItemType::Stick, 2))
        );
        assert!(menu.craft_output().is_none());
    }

    #[test]
    fn secondary_click_takes_half_from_take_only_outputs() {
        let mut menu = ContainerMenu::new();
        let mut inv = Inventory::new();
        let mut world = world_with_empty_chunk();
        menu.open_crafting(CraftingStation::Inventory);
        menu.craft_output = Some(ItemStack::new(ItemType::Stick, 5));

        menu.click(
            &mut world,
            &mut inv,
            &recipes(),
            MenuSlot::CraftResult,
            PointerButton::Secondary,
            false,
            false,
        );

        assert_eq!(
            inv.cursor().copied(),
            Some(ItemStack::new(ItemType::Stick, 3))
        );
        assert_eq!(
            menu.craft_output(),
            Some(ItemStack::new(ItemType::Stick, 2))
        );

        let pos = IVec3::new(2, 64, 2);
        world.set_block_world(pos.x, pos.y, pos.z, Block::Furnace);
        world.insert_furnace(pos, Facing::North);
        world.container_at_mut(pos).unwrap().slots[crate::furnace::SLOT_OUTPUT] =
            Some(ItemStack::new(ItemType::IronIngot, 5));
        menu.open_furnace_screen(&mut world, pos);
        *inv.cursor_mut() = None;

        menu.click(
            &mut world,
            &mut inv,
            &recipes(),
            MenuSlot::Furnace(crate::gui::FurnaceHit::Output),
            PointerButton::Secondary,
            false,
            false,
        );

        assert_eq!(
            inv.cursor().copied(),
            Some(ItemStack::new(ItemType::IronIngot, 3))
        );
        assert_eq!(
            world.container_at(pos).unwrap().slots[crate::furnace::SLOT_OUTPUT],
            Some(ItemStack::new(ItemType::IronIngot, 2))
        );
    }

    #[test]
    fn primary_drag_splits_across_player_and_container_slots_in_hit_order() {
        let mut world = world_with_empty_chunk();
        let mut menu = ContainerMenu::new();
        let pos = IVec3::new(3, 64, 3);
        world.set_block_world(pos.x, pos.y, pos.z, Block::Chest);
        world.insert_chest(pos, Facing::North);
        menu.open_chest_screen(&mut world, pos);

        let mut inv = Inventory::new();
        inv.add(ItemStack::new(ItemType::Grass, 10));
        inv.click_slot(0);
        menu.drag_slots(
            &mut world,
            &mut inv,
            &[
                MenuSlot::Inventory(9),
                MenuSlot::Chest(0),
                MenuSlot::Inventory(9),
                MenuSlot::Chest(1),
            ],
            PointerButton::Primary,
        );

        assert!(inv.cursor().is_none());
        assert_eq!(
            inv.slot(9).copied(),
            Some(ItemStack::new(ItemType::Grass, 3))
        );
        let chest = world.container_at(pos).unwrap();
        assert_eq!(chest.slots[0], Some(ItemStack::new(ItemType::Grass, 3)));
        assert_eq!(
            chest.slots[1],
            Some(ItemStack::new(ItemType::Grass, 4)),
            "the uneven remainder belongs to the last distinct hit"
        );
    }

    #[test]
    fn hovered_slot_drop_removes_one_or_the_whole_container_stack() {
        let mut world = world_with_empty_chunk();
        let mut menu = ContainerMenu::new();
        let mut inv = Inventory::new();
        let pos = IVec3::new(4, 64, 4);
        world.set_block_world(pos.x, pos.y, pos.z, Block::Chest);
        world.insert_chest(pos, Facing::North);
        world.container_at_mut(pos).unwrap().slots[0] = Some(ItemStack::new(ItemType::Coal, 5));
        menu.open_chest_screen(&mut world, pos);

        assert_eq!(
            menu.drop_slot(&mut world, &mut inv, &recipes(), MenuSlot::Chest(0), false),
            Some(ItemStack::new(ItemType::Coal, 1))
        );
        assert_eq!(
            world.container_at(pos).unwrap().slots[0],
            Some(ItemStack::new(ItemType::Coal, 4))
        );
        assert_eq!(
            menu.drop_slot(&mut world, &mut inv, &recipes(), MenuSlot::Chest(0), true),
            Some(ItemStack::new(ItemType::Coal, 4))
        );
        assert!(world.container_at(pos).unwrap().slots[0].is_none());
    }

    #[test]
    fn occupied_output_and_wrong_station_are_atomic_failures() {
        let recipes = player_crafting_recipes(CraftingStation::CraftingTable);
        let mut menu = ContainerMenu::new();
        let mut inv = Inventory::new();
        inv.add(ItemStack::new(ItemType::Coal, 2));

        menu.open_crafting(CraftingStation::Inventory);
        assert_eq!(
            menu.craft_recipe(&mut inv, &recipes, "test:coal_to_sticks", false),
            Err(CraftMenuFailure::InvalidRecipe)
        );
        assert_eq!(
            inv.slot(0).copied(),
            Some(ItemStack::new(ItemType::Coal, 2))
        );

        menu.open_crafting(CraftingStation::CraftingTable);
        assert!(menu
            .craft_recipe(&mut inv, &recipes, "test:coal_to_sticks", false)
            .is_ok());
        // A same-item output MERGES the repeat craft (stackable results keep
        // the button usable) instead of refusing.
        assert!(menu
            .craft_recipe(&mut inv, &recipes, "test:coal_to_sticks", false)
            .is_ok());
        assert_eq!(
            menu.craft_output(),
            Some(ItemStack::new(ItemType::Stick, 4))
        );
        assert!(inv.slot(0).is_none(), "both coal consumed");

        // A foreign-item output still refuses without consuming anything.
        inv.add(ItemStack::new(ItemType::Coal, 1));
        menu.craft_output = Some(ItemStack::new(ItemType::Dirt, 1));
        assert_eq!(
            menu.craft_recipe(&mut inv, &recipes, "test:coal_to_sticks", false),
            Err(CraftMenuFailure::OutputOccupied)
        );
        assert_eq!(
            inv.slot(0).copied(),
            Some(ItemStack::new(ItemType::Coal, 1))
        );
    }

    #[test]
    fn bulk_craft_stops_at_missing_ingredients_or_a_full_output_stack() {
        let recipes = player_crafting_recipes(CraftingStation::Inventory);
        let mut menu = ContainerMenu::new();
        let mut inv = Inventory::new();
        inv.add(ItemStack::new(ItemType::Coal, 5));
        menu.open_crafting(CraftingStation::Inventory);

        // Resource-bound: 5 coal → 5 crafts of 2 sticks each.
        assert_eq!(
            menu.craft_recipe(&mut inv, &recipes, "test:coal_to_sticks", true),
            Ok(Vec::new())
        );
        assert_eq!(
            menu.craft_output(),
            Some(ItemStack::new(ItemType::Stick, 10))
        );
        assert!(inv.slot(0).is_none(), "all coal consumed");

        // Stack-bound: plenty of coal, but the output caps at one full stack.
        let max = ItemType::Stick.max_stack_size();
        inv.add(ItemStack::new(ItemType::Coal, max));
        menu.craft_output = None;
        assert_eq!(
            menu.craft_recipe(&mut inv, &recipes, "test:coal_to_sticks", true),
            Ok(Vec::new())
        );
        assert_eq!(
            menu.craft_output(),
            Some(ItemStack::new(ItemType::Stick, max / 2 * 2))
        );
        assert_eq!(
            inv.slot(0).map(|stack| stack.count),
            Some(max - max / 2),
            "coal beyond the full output stack stays in the inventory"
        );

        // An impossible first craft is still the request's failure.
        assert_eq!(
            menu.craft_recipe(&mut inv, &recipes, "test:coal_to_sticks", true),
            Err(CraftMenuFailure::OutputOccupied)
        );
    }

    #[test]
    fn closing_crafting_returns_real_output_to_inventory() {
        let mut menu = ContainerMenu::new();
        let mut inv = Inventory::new();
        menu.open_crafting(CraftingStation::Inventory);
        menu.craft_output = Some(ItemStack::new(ItemType::Stick, 2));

        menu.close_crafting(&mut inv, |_| panic!("nothing should overflow"));

        assert_eq!(
            inv.slot(0).copied(),
            Some(ItemStack::new(ItemType::Stick, 2))
        );
        assert_eq!(menu.target(), ContainerTarget::None);
    }

    #[test]
    fn closing_crafting_routes_output_overflow_to_the_drop_sink() {
        let mut menu = ContainerMenu::new();
        let mut inv = Inventory::new();
        for _ in 0..crate::inventory::TOTAL_SLOTS {
            assert!(inv
                .add(ItemStack::new(
                    ItemType::Dirt,
                    ItemType::Dirt.max_stack_size(),
                ))
                .is_none());
        }
        menu.open_crafting(CraftingStation::Inventory);
        let output = ItemStack::new(ItemType::Stick, 2);
        menu.craft_output = Some(output);
        let mut overflow = Vec::new();

        menu.close_crafting(&mut inv, |stack| overflow.push(stack));

        assert_eq!(overflow, vec![output]);
        assert!(menu.craft_output().is_none());
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
