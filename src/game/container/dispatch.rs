use super::{ContainerMenu, ContainerTarget};
use crate::controls::PointerButton;
use crate::crafting::Recipes;
use crate::furnace::{SLOT_FUEL, SLOT_INPUT, SLOT_OUTPUT};
use crate::gui::{FurnaceHit, MenuSlot, WorkbenchHit};
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
    /// Every block-entity container slot (chest, furnace, mod document) decodes
    /// through ONE generic path driven by the target's `SlotSpec`s — the furnace's
    /// role hits just map to their conventional indices first. The transient
    /// crafting output and furniture workbench keep dedicated handling.
    pub(crate) fn click(
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
                    // a container target tag-routes the stack into its slots per its
                    // SlotSpecs, and otherwise it shuffles hotbar↔main-grid.
                    match self.target {
                        ContainerTarget::Chest(_)
                        | ContainerTarget::Furnace(_)
                        | ContainerTarget::ModGui { .. } => {
                            self.container_shift_from_inventory(world, inv, i)
                        }
                        ContainerTarget::FurnitureWorkbench => {
                            self.workbench_shift_from_inventory(inv, recipes, i)
                        }
                        _ => inv.shift_move_slot(i),
                    }
                } else if gather {
                    // A double-click while dragging gathers matching items; with a
                    // container open it sweeps the container too, else just the
                    // inventory. (The App gates this on the cursor holding a stack.)
                    self.collect_to_cursor(world, inv);
                } else {
                    match button {
                        PointerButton::Primary => inv.click_slot(i),
                        PointerButton::Secondary => inv.right_click_slot(i),
                    }
                }
            }
            MenuSlot::CraftResult => self.craft_take_output(inv, shift),
            // The furnace's role hits are its conventional container indices;
            // its SlotSpecs (take-only output, tag filters) drive the rest.
            MenuSlot::Furnace(hit) => {
                let i = match hit {
                    FurnaceHit::Input => SLOT_INPUT,
                    FurnaceHit::Fuel => SLOT_FUEL,
                    FurnaceHit::Output => SLOT_OUTPUT,
                };
                self.container_slot_interaction(world, inv, i, button, shift, gather);
            }
            MenuSlot::Chest(i) | MenuSlot::Container(i) => {
                self.container_slot_interaction(world, inv, i, button, shift, gather);
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
}
