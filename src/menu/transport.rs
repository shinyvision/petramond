//! Multi-slot transport gestures and hovered-slot drops.
//!
//! Pointer hit testing stays app-side, but the resulting ordered slot list is
//! applied here on the tick so mixed inventory/container drags remain one
//! deterministic, no-loss mutation.

use super::{ContainerMenu, ContainerTarget};
use petramond_world::gui_state::PointerButton;
use petramond_world::gui_state::{MenuSlot, MAX_MENU_DRAG_SLOTS};
use petramond_world::inventory::{plan_drag_distribution, slot_capacity, take_slot_stack, Inventory};
use petramond_world::item::ItemStack;
use crate::world::World;

impl ContainerMenu {
    /// Distribute the cursor stack across distinct compatible destinations in
    /// first-hit order. Primary divides the original held count evenly and
    /// gives the uneven remainder to the last destination; secondary places
    /// one in each destination. Capacity limits leave the unplaced part on
    /// the cursor, and take-only/virtual outputs are never destinations.
    pub fn drag_slots(
        &mut self,
        world: &mut World,
        inv: &mut Inventory,
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
            |slot| self.drag_capacity(world, inv, slot, &held),
        );
        for (slot, wanted) in plan {
            self.place_cursor_in(world, inv, slot, wanted);
        }
    }

    /// Remove one item or the whole stack from the hovered logical slot.
    /// The returned concrete stack is queued into the ordinary world-drop
    /// sink by the server menu stage.
    pub fn drop_slot(
        &mut self,
        world: &mut World,
        inv: &mut Inventory,
        slot: MenuSlot,
        all: bool,
    ) -> Option<ItemStack> {
        if self.target == ContainerTarget::None {
            return None;
        }
        match slot {
            MenuSlot::Inventory(i) => inv.take_slot_for_drop(i, all),
            MenuSlot::CraftResult if self.crafting_station().is_some() => {
                take_slot_stack(&mut self.craft_output, all)
            }
            MenuSlot::Container(_) => self.drop_open_container_slot(world, slot, all),
            MenuSlot::CraftResult | MenuSlot::Widget(_) => None,
        }
    }

    /// Validate the slot's role identity against the open kind — a forged
    /// role can never address another target's backing container.
    fn open_container_index(&self, slot: MenuSlot) -> Option<usize> {
        match (self.target.kind()?, slot) {
            // Every block-backed container addresses its slots by plain index
            // — engine chest, engine furnace, and a pack's alike. What each
            // index MEANS is the document's `SlotSpec`, not a role name.
            (kind, MenuSlot::Container(i)) if ContainerTarget::kind_block_backed(kind) => Some(i),
            _ => None,
        }
    }

    fn drag_capacity(
        &self,
        world: &World,
        inv: &Inventory,
        slot: MenuSlot,
        held: &ItemStack,
    ) -> u8 {
        match slot {
            MenuSlot::Inventory(i) => inv
                .raw_slots()
                .get(i)
                .map(|cell| slot_capacity(cell, held))
                .unwrap_or(0),
            MenuSlot::Container(_) => {
                let Some(i) = self.open_container_index(slot) else {
                    return 0;
                };
                // A drag is the same deliberate placement a click is, so it
                // asks the same question. Checking only `take_only` here made
                // `accepts` a filter that any held button walked straight
                // through — on both this path and the client's mirror of it.
                if !self.slot_admits(i, Some(held.item)) {
                    return 0;
                }
                self.container_pos()
                    .and_then(|pos| world.container_at(pos))
                    .and_then(|container| container.slots.get(i))
                    .map(|cell| slot_capacity(cell, held))
                    .unwrap_or(0)
            }
            MenuSlot::CraftResult | MenuSlot::Widget(_) => 0,
        }
    }

    /// Whether container slot `i` accepts `held` on a deliberate placement —
    /// this menu's specs through the shared [`petramond_world::container::slot_admits`].
    fn slot_admits(&self, i: usize, held: Option<petramond_world::item::ItemType>) -> bool {
        petramond_world::container::slot_admits(&self.slot_specs(), i, held)
    }

    fn place_cursor_in(
        &mut self,
        world: &mut World,
        inv: &mut Inventory,
        slot: MenuSlot,
        wanted: u8,
    ) {
        match slot {
            MenuSlot::Inventory(i) => {
                inv.place_cursor_count_in_slot(i, wanted);
            }
            MenuSlot::Container(_) => {
                let Some(i) = self.open_container_index(slot) else {
                    return;
                };
                let held = inv.cursor().map(|c| c.item);
                if !self.slot_admits(i, held) {
                    return;
                }
                let Some(pos) = self.container_pos() else {
                    return;
                };
                let moved = world
                    .container_at_mut(pos)
                    .and_then(|container| container.slots.get_mut(i))
                    .map(|cell| inv.place_cursor_count_in_external_slot(cell, wanted))
                    .unwrap_or(0);
                if moved > 0 {
                    world.mark_chunk_modified(pos);
                }
            }
            MenuSlot::CraftResult | MenuSlot::Widget(_) => {}
        }
    }

    fn drop_open_container_slot(
        &self,
        world: &mut World,
        slot: MenuSlot,
        all: bool,
    ) -> Option<ItemStack> {
        let i = self.open_container_index(slot)?;
        let pos = self.container_pos()?;
        let dropped = world
            .container_at_mut(pos)
            .and_then(|container| container.slots.get_mut(i))
            .and_then(|cell| take_slot_stack(cell, all));
        if dropped.is_some() {
            world.mark_chunk_modified(pos);
        }
        dropped
    }
}
