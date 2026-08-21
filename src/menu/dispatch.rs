use super::{ContainerMenu, ContainerTarget};
use crate::world::World;
use petramond_world::gui_state::MenuSlot;
use petramond_world::gui_state::PointerButton;
use petramond_world::inventory::Inventory;

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
    /// crafting output keeps dedicated handling.
    pub fn click(
        &mut self,
        world: &mut World,
        inv: &mut Inventory,
        gui: Option<&petramond_world::gui_state::GuiStateMap>,
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
                    match self.target.kind() {
                        Some(kind) if ContainerTarget::kind_block_backed(kind) => {
                            self.container_shift_from_inventory(world, inv, gui, i)
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
            MenuSlot::OffHand => {
                if shift {
                    // Shift-click ships the off-hand stack back into the grid;
                    // nothing shift-routes INTO the off-hand (a deliberate
                    // placement only).
                    inv.shift_move_off_hand();
                } else if gather {
                    self.collect_to_cursor(world, inv);
                } else {
                    let mut cell = inv.take_off_hand();
                    match button {
                        PointerButton::Primary => inv.click_external_slot(&mut cell),
                        PointerButton::Secondary => inv.right_click_external_slot(&mut cell),
                    }
                    *inv.off_hand_mut() = cell;
                }
            }
            MenuSlot::CraftResult => self.craft_take_output(inv, button, shift),
            MenuSlot::Container(i) => {
                self.container_slot_interaction(world, inv, gui, i, button, shift, gather);
            }
            // Widget clicks mutate no container: `Game::tick_menu` intercepts
            // them before this decode and dispatches to the owning mod.
            MenuSlot::Widget(_) => {}
        }
    }
}
