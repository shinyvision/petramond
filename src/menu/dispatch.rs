use super::{ContainerMenu, ContainerTarget};
use crate::world::ServerWorld;
use petramond_world::gui_state::MenuSlot;
use petramond_world::gui_state::PointerButton;
use petramond_world::inventory::Inventory;

impl ContainerMenu {
    #[allow(clippy::too_many_arguments)]
    pub fn click(
        &mut self,
        world: &mut ServerWorld,
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
                    match self.target.kind() {
                        Some(kind) if ContainerTarget::kind_anchor_backed(kind) => {
                            self.container_shift_from_inventory(world, inv, gui, i)
                        }
                        _ => inv.shift_move_slot(i),
                    }
                } else if gather {
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
            MenuSlot::Widget(_) => {}
        }
    }
}
