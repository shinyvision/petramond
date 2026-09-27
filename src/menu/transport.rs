use super::{ContainerMenu, ContainerTarget};
use crate::world::ServerWorld;
use petramond_world::gui_state::PointerButton;
use petramond_world::gui_state::{MenuSlot, MAX_MENU_DRAG_SLOTS};
use petramond_world::inventory::{
    plan_drag_distribution, slot_capacity, take_slot_stack, Inventory,
};
use petramond_world::item::ItemStack;

impl ContainerMenu {
    pub fn drag_slots(
        &mut self,
        world: &mut ServerWorld,
        inv: &mut Inventory,
        gui: Option<&petramond_world::gui_state::GuiStateMap>,
        slots: &[MenuSlot],
        button: PointerButton,
    ) {
        let Some(held) = inv.cursor().copied() else {
            return;
        };
        if self.target == ContainerTarget::None {
            return;
        }

        let hits = &slots[..slots.len().min(MAX_MENU_DRAG_SLOTS)];
        let plan = plan_drag_distribution(
            hits,
            held.count,
            button == PointerButton::Secondary,
            |slot| self.drag_capacity(world, inv, gui, slot, &held),
        );
        for (slot, wanted) in plan {
            self.place_cursor_in(world, inv, gui, slot, wanted);
        }
    }

    pub fn drop_slot(
        &mut self,
        world: &mut ServerWorld,
        inv: &mut Inventory,
        slot: MenuSlot,
        all: bool,
    ) -> Option<ItemStack> {
        if self.target == ContainerTarget::None {
            return None;
        }
        match slot {
            MenuSlot::Inventory(i) => inv.take_slot_for_drop(i, all),
            MenuSlot::OffHand => take_slot_stack(inv.off_hand_mut(), all),
            MenuSlot::CraftResult if self.crafting_station().is_some() => {
                take_slot_stack(&mut self.craft_output, all)
            }
            MenuSlot::Container(_) => self.drop_open_container_slot(world, slot, all),
            MenuSlot::CraftResult | MenuSlot::Widget(_) => None,
        }
    }

    pub fn swap_off_hand(
        &mut self,
        world: &mut ServerWorld,
        inv: &mut Inventory,
        gui: Option<&petramond_world::gui_state::GuiStateMap>,
        slot: MenuSlot,
    ) {
        match slot {
            MenuSlot::Inventory(i) => inv.swap_off_hand_with_slot(i),
            MenuSlot::Container(_) => {
                let Some(i) = self.open_container_index(slot) else {
                    return;
                };
                let specs = self.slot_specs();
                self.edit_open_container(world, |container| {
                    if let Some(cell) = container.slots.get_mut(i) {
                        inv.swap_off_hand_with_cell(specs.get(i), gui, cell);
                    }
                });
            }
            MenuSlot::OffHand | MenuSlot::CraftResult | MenuSlot::Widget(_) => {}
        }
    }

    fn open_container_index(&self, slot: MenuSlot) -> Option<usize> {
        match (self.target.kind()?, slot) {
            (kind, MenuSlot::Container(i)) if ContainerTarget::kind_anchor_backed(kind) => Some(i),
            _ => None,
        }
    }

    fn drag_capacity(
        &self,
        world: &ServerWorld,
        inv: &Inventory,
        gui: Option<&petramond_world::gui_state::GuiStateMap>,
        slot: MenuSlot,
        held: &ItemStack,
    ) -> u8 {
        match slot {
            MenuSlot::Inventory(i) => inv
                .raw_slots()
                .get(i)
                .map(|cell| slot_capacity(cell, held))
                .unwrap_or(0),
            MenuSlot::OffHand => slot_capacity(&inv.off_hand().copied(), held),
            MenuSlot::Container(_) => {
                let Some(i) = self.open_container_index(slot) else {
                    return 0;
                };
                if !self.slot_admits(i, Some(held.item), gui) {
                    return 0;
                }
                self.open_container(world)
                    .and_then(|container| container.slots.get(i))
                    .map(|cell| slot_capacity(cell, held))
                    .unwrap_or(0)
            }
            MenuSlot::CraftResult | MenuSlot::Widget(_) => 0,
        }
    }

    fn slot_admits(
        &self,
        i: usize,
        held: Option<petramond_world::item::ItemType>,
        gui: Option<&petramond_world::gui_state::GuiStateMap>,
    ) -> bool {
        petramond_world::container::slot_admits(&self.slot_specs(), i, held, gui)
    }

    fn place_cursor_in(
        &mut self,
        world: &mut ServerWorld,
        inv: &mut Inventory,
        gui: Option<&petramond_world::gui_state::GuiStateMap>,
        slot: MenuSlot,
        wanted: u8,
    ) {
        match slot {
            MenuSlot::Inventory(i) => {
                inv.place_cursor_count_in_slot(i, wanted);
            }
            MenuSlot::OffHand => {
                let mut cell = inv.take_off_hand();
                inv.place_cursor_count_in_external_slot(&mut cell, wanted);
                *inv.off_hand_mut() = cell;
            }
            MenuSlot::Container(_) => {
                let Some(i) = self.open_container_index(slot) else {
                    return;
                };
                let held = inv.cursor().map(|c| c.item);
                if !self.slot_admits(i, held, gui) {
                    return;
                }
                self.edit_open_container(world, |container| {
                    if let Some(cell) = container.slots.get_mut(i) {
                        inv.place_cursor_count_in_external_slot(cell, wanted);
                    }
                });
            }
            MenuSlot::CraftResult | MenuSlot::Widget(_) => {}
        }
    }

    fn drop_open_container_slot(
        &self,
        world: &mut ServerWorld,
        slot: MenuSlot,
        all: bool,
    ) -> Option<ItemStack> {
        let i = self.open_container_index(slot)?;
        self.edit_open_container(world, |container| {
            container
                .slots
                .get_mut(i)
                .and_then(|cell| take_slot_stack(cell, all))
        })
        .flatten()
    }
}
