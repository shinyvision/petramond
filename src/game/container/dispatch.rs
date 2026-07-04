use super::{ContainerMenu, ContainerTarget};
use crate::controls::PointerButton;
use crate::crafting::Recipes;
use crate::gui::{CraftHit, FurnaceHit, MenuSlot, WorkbenchHit};
use crate::inventory::Inventory;
use crate::world::World;

impl ContainerMenu {
    /// Route a hit-tested click to the open container. This is the single decision
    /// point that replaced the App's four parallel `route_*` routers and its
    /// `is_furnace()/is_chest()` ladder: the App resolves the pixel to a [`MenuSlot`]
    /// (a role for the furnace, an index elsewhere) and hands it here with the button
    /// and the physical Shift state, plus `gather` — the App-owned double-click
    /// verdict that turns a left-click-with-a-held-stack into a same-item collect.
    ///
    /// The per-slot decoding (shift quick-move vs left/right place-split-swap, the
    /// furnace's take-only output, the inventory shift-click routed by the open
    /// target) all lives below, matched on `self.target` so adding a container type
    /// touches this one method, not a ladder repeated per interaction.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::game) fn click(
        &mut self,
        world: &mut World,
        inv: &mut Inventory,
        recipes: &Recipes,
        slot: MenuSlot,
        button: PointerButton,
        shift: bool,
        gather: bool,
    ) {
        match slot {
            MenuSlot::Inventory(i) => {
                if shift {
                    // Shift-click of an inventory slot is routed by the open target:
                    // the furnace tag-routes fuel/smeltable into its slots, the chest
                    // dumps the stack in, and otherwise it shuffles hotbar↔main-grid.
                    match self.target {
                        ContainerTarget::Furnace(_) => {
                            self.furnace_shift_from_inventory(world, inv, i)
                        }
                        ContainerTarget::Chest(_) => self.chest_shift_from_inventory(world, inv, i),
                        ContainerTarget::FurnitureWorkbench => {
                            self.workbench_shift_from_inventory(inv, recipes, i)
                        }
                        _ => inv.shift_move_slot(i),
                    }
                } else if gather {
                    // A double-click while dragging gathers matching items; with the
                    // chest open it sweeps the chest too (matching MC), else just the
                    // inventory. (The App gates this on the cursor holding a stack.)
                    self.collect_to_cursor(world, inv);
                } else {
                    match button {
                        PointerButton::Primary => inv.click_slot(i),
                        PointerButton::Secondary => inv.right_click_slot(i),
                    }
                }
            }
            MenuSlot::Craft(hit) => match hit {
                CraftHit::Input(i) => {
                    if shift {
                        self.craft_shift_slot(inv, recipes, i);
                    } else {
                        match button {
                            PointerButton::Primary => self.craft_click_slot(inv, recipes, i),
                            PointerButton::Secondary => {
                                self.craft_right_click_slot(inv, recipes, i)
                            }
                        }
                    }
                }
                CraftHit::Result => {
                    if shift {
                        self.craft_shift_result(inv, recipes);
                    } else {
                        self.craft_take_result(inv, recipes);
                    }
                }
            },
            MenuSlot::Furnace(hit) => match hit {
                FurnaceHit::Input => {
                    if shift {
                        self.furnace_shift_input(world, inv);
                    } else {
                        match button {
                            PointerButton::Primary => self.furnace_click_input(world, inv),
                            PointerButton::Secondary => self.furnace_right_click_input(world, inv),
                        }
                    }
                }
                FurnaceHit::Fuel => {
                    if shift {
                        self.furnace_shift_fuel(world, inv);
                    } else {
                        match button {
                            PointerButton::Primary => self.furnace_click_fuel(world, inv),
                            PointerButton::Secondary => self.furnace_right_click_fuel(world, inv),
                        }
                    }
                }
                // Output is take-only: any click moves the product out (shift -> inv).
                FurnaceHit::Output => {
                    if shift {
                        self.furnace_shift_output(world, inv);
                    } else {
                        self.furnace_take_output(world, inv);
                    }
                }
            },
            MenuSlot::Chest(i) => {
                if shift {
                    self.chest_shift_slot(world, inv, i);
                } else if gather {
                    // Double-click in the chest gathers from the chest AND inventory.
                    self.collect_to_cursor_in_chest(world, inv);
                } else {
                    match button {
                        PointerButton::Primary => self.chest_click_slot(world, inv, i),
                        PointerButton::Secondary => self.chest_right_click_slot(world, inv, i),
                    }
                }
            }
            MenuSlot::Workbench(hit) => match hit {
                WorkbenchHit::Input => {
                    if shift {
                        self.workbench_shift_input(inv);
                    } else {
                        match button {
                            PointerButton::Primary => {
                                inv.click_external_slot(&mut self.workbench_input)
                            }
                            PointerButton::Secondary => {
                                inv.right_click_external_slot(&mut self.workbench_input)
                            }
                        }
                    }
                }
                // Results are take-only: craft the i-th offered recipe (shift -> inv).
                WorkbenchHit::Result(i) => self.workbench_take_result(inv, recipes, i, shift),
            },
            // Widget clicks mutate no container: `Game::tick_menu` intercepts
            // them before this decode and dispatches to the owning mod.
            MenuSlot::Widget(_) => {}
        }
    }

    /// The gather a double-click on an inventory slot performs: with a chest open it
    /// sweeps the chest's slots too (matching MC), otherwise only the inventory.
    fn collect_to_cursor(&self, world: &mut World, inv: &mut Inventory) {
        if matches!(self.target, ContainerTarget::Chest(_)) {
            self.collect_to_cursor_in_chest(world, inv);
        } else {
            inv.collect_to_cursor();
        }
    }
}
