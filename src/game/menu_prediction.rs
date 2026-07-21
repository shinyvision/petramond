//! Optimistic client prediction for atomic multi-slot menu transport.
//!
//! The active pointer gesture is previewed by the app. On release this module
//! commits the identical plan into the replicated client inventory/menu view
//! and snapshots both for ledger rollback, so presentation never falls back
//! to the pre-drag state while the authoritative outcome is in flight.

use super::Game;
use crate::controls::PointerButton;
use crate::gui::{FurnaceHit, GuiKind, MenuSlot, MAX_MENU_DRAG_SLOTS};
use crate::inventory::{plan_drag_distribution, slot_capacity};
use crate::item::ItemType;
use crate::net::protocol::{ClientToServer, MenuSlotWire};

impl Game {
    /// Predict one complete cursor-stack distribution and send the same
    /// ordered intent to the server. The inventory and open menu mirror are
    /// one rollback unit because a gesture may span both stores.
    pub fn menu_drag(&mut self, kind: GuiKind, slots: Vec<MenuSlot>, button: PointerButton) {
        let slots: Vec<_> = slots.into_iter().take(MAX_MENU_DRAG_SLOTS).collect();
        if slots.len() < 2 {
            return;
        }

        let can_predict = self.prediction.can_predict();
        let snapshot = if can_predict {
            crate::game::prediction::PredictionSnapshot::Menu {
                inventory: self.self_view.inventory.clone(),
                menu: self.menu_view.clone(),
            }
        } else {
            crate::game::prediction::PredictionSnapshot::None
        };
        let request_id = self.prediction.begin(snapshot);
        if can_predict {
            self.predict_menu_drag(kind, &slots, button);
        }

        self.outbox.push(ClientToServer::MenuDrag {
            slots: slots.iter().map(MenuSlotWire::from_menu_slot).collect(),
            button: crate::net::protocol::button_to_wire(button),
            request_id,
        });
    }

    fn predict_menu_drag(&mut self, kind: GuiKind, slots: &[MenuSlot], button: PointerButton) {
        let Some(held) = self.self_view.inventory.cursor().copied() else {
            return;
        };
        let specs = crate::gui::documents::container_slot_specs(kind);
        let plan = plan_drag_distribution(
            slots,
            held.count,
            button == PointerButton::Secondary,
            |slot| self.predicted_drag_capacity(&specs, slot, held.item),
        );
        for (slot, wanted) in plan {
            self.predicted_drag_place(&specs, slot, wanted);
        }
    }

    fn predicted_drag_capacity(
        &self,
        specs: &[crate::container::SlotSpec],
        slot: MenuSlot,
        item: ItemType,
    ) -> u8 {
        match slot {
            MenuSlot::Inventory(i) => self
                .self_view
                .inventory
                .raw_slots()
                .get(i)
                .map(|cell| slot_capacity(cell, item))
                .unwrap_or(0),
            MenuSlot::Furnace(hit) => self
                .menu_view
                .furnace
                .as_ref()
                .map(|furnace| match hit {
                    FurnaceHit::Input => slot_capacity(&furnace.input, item),
                    FurnaceHit::Fuel => slot_capacity(&furnace.fuel, item),
                    FurnaceHit::Output => 0,
                })
                .unwrap_or(0),
            MenuSlot::Chest(i) => self
                .menu_view
                .chest
                .as_ref()
                .and_then(|chest| chest.slots.get(i))
                .map(|cell| slot_capacity(cell, item))
                .unwrap_or(0),
            MenuSlot::Container(i) if specs.get(i).is_some_and(|spec| !spec.take_only) => self
                .menu_view
                .container
                .as_ref()
                .and_then(|container| container.slots.get(i))
                .map(|cell| slot_capacity(cell, item))
                .unwrap_or(0),
            MenuSlot::CraftResult | MenuSlot::Container(_) | MenuSlot::Widget(_) => 0,
        }
    }

    fn predicted_drag_place(
        &mut self,
        specs: &[crate::container::SlotSpec],
        slot: MenuSlot,
        wanted: u8,
    ) {
        let inventory = &mut self.self_view.inventory;
        let menu = &mut self.menu_view;
        match slot {
            MenuSlot::Inventory(i) => {
                inventory.place_cursor_count_in_slot(i, wanted);
            }
            MenuSlot::Furnace(FurnaceHit::Input) => {
                if let Some(furnace) = menu.furnace.as_mut() {
                    inventory.place_cursor_count_in_external_slot(&mut furnace.input, wanted);
                }
            }
            MenuSlot::Furnace(FurnaceHit::Fuel) => {
                if let Some(furnace) = menu.furnace.as_mut() {
                    inventory.place_cursor_count_in_external_slot(&mut furnace.fuel, wanted);
                }
            }
            MenuSlot::Chest(i) => {
                if let Some(cell) = menu.chest.as_mut().and_then(|chest| chest.slots.get_mut(i)) {
                    inventory.place_cursor_count_in_external_slot(cell, wanted);
                }
            }
            MenuSlot::Container(i) if specs.get(i).is_some_and(|spec| !spec.take_only) => {
                if let Some(cell) = menu
                    .container
                    .as_mut()
                    .and_then(|container| container.slots.get_mut(i))
                {
                    inventory.place_cursor_count_in_external_slot(cell, wanted);
                }
            }
            MenuSlot::CraftResult
            | MenuSlot::Furnace(FurnaceHit::Output)
            | MenuSlot::Container(_)
            | MenuSlot::Widget(_) => {}
        }
    }
}
