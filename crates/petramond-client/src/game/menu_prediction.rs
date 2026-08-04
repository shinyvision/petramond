//! Optimistic client prediction for menu clicks, drops, and atomic
//! multi-slot drag transport.
//!
//! The active pointer gesture is previewed by the app. On release this module
//! commits the identical plan into the replicated client inventory/menu view
//! and snapshots both for ledger rollback, so presentation never falls back
//! to the pre-drag state while the authoritative outcome is in flight.
//!
//! Every menu gesture lives here with the predicate that decides whether it
//! CAN be predicted and the apply that mirrors the server's own — one file, so
//! the gate and the mutation it guards cannot drift apart.

use super::Game;
use petramond::gui_state::PointerButton;
use petramond::gui_state::{GuiKind, MenuSlot, MAX_MENU_DRAG_SLOTS};
use petramond::inventory::{plan_drag_distribution, slot_capacity};
use petramond::item::ItemStack;
use petramond::net::protocol::{ClientToServer, MenuSlotWire};

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
            button: petramond::net::protocol::button_to_wire(button),
            request_id,
        });
    }

    fn predict_menu_drag(&mut self, kind: GuiKind, slots: &[MenuSlot], button: PointerButton) {
        let Some(held) = self.self_view.inventory.cursor().copied() else {
            return;
        };
        let specs = petramond::gui::documents::container_slot_specs(kind);
        let plan = plan_drag_distribution(
            slots,
            held.count,
            button == PointerButton::Secondary,
            |slot| self.predicted_drag_capacity(&specs, slot, &held),
        );
        for (slot, wanted) in plan {
            self.predicted_drag_place(&specs, slot, wanted);
        }
    }

    fn predicted_drag_capacity(
        &self,
        specs: &[petramond::container::SlotSpec],
        slot: MenuSlot,
        held: &ItemStack,
    ) -> u8 {
        match slot {
            MenuSlot::Inventory(i) => self
                .self_view
                .inventory
                .raw_slots()
                .get(i)
                .map(|cell| slot_capacity(cell, held))
                .unwrap_or(0),
            // The same question the server's `drag_capacity` asks, through the
            // same helper: a slot one side counts and the other refuses does
            // not just snap that leg back — the split is by the NUMBER of
            // destinations, so every other slot of the drag lands wrong too.
            MenuSlot::Container(i) if petramond::container::slot_admits(specs, i, Some(held.item)) => {
                self.menu_view
                    .container
                    .as_ref()
                    .and_then(|container| container.slots.get(i))
                    .map(|cell| slot_capacity(cell, held))
                    .unwrap_or(0)
            }
            MenuSlot::CraftResult | MenuSlot::Container(_) | MenuSlot::Widget(_) => 0,
        }
    }

    fn predicted_drag_place(
        &mut self,
        specs: &[petramond::container::SlotSpec],
        slot: MenuSlot,
        wanted: u8,
    ) {
        let inventory = &mut self.self_view.inventory;
        let menu = &mut self.menu_view;
        match slot {
            MenuSlot::Inventory(i) => {
                inventory.place_cursor_count_in_slot(i, wanted);
            }
            // The same question the server's drag asks, through the same
            // helper. A leg the client mirrors and the server refuses is a
            // stack that visibly lands and then snaps back.
            MenuSlot::Container(i)
                if petramond::container::slot_admits(
                    specs,
                    i,
                    inventory.cursor().map(|held| held.item),
                ) =>
            {
                if let Some(cell) = menu
                    .container
                    .as_mut()
                    .and_then(|container| container.slots.get_mut(i))
                {
                    inventory.place_cursor_count_in_external_slot(cell, wanted);
                }
            }
            MenuSlot::CraftResult | MenuSlot::Container(_) | MenuSlot::Widget(_) => {}
        }
    }

    /// Whether the client can faithfully predict a click's outcome. Inventory
    /// slots: always for plain clicks; shift/gather only while no open target
    /// reroutes them (the shared apply routes a shifted stack INTO an open
    /// chest/furnace/mod/workbench, and a gather sweeps an open block
    /// container — predicting those with inventory-only primitives would
    /// drift from the server). Container slots (chest/furnace/mod document):
    /// plain clicks only, and only while the mirror view is present — the
    /// mutation is cursor ↔ mirrored slot through the same external-slot
    /// primitives the server's decode runs. Shift quick-moves and gathers on
    /// those still ride track-only (the single-apply-path rule).
    fn menu_click_is_predictable(
        &self,
        slot: petramond::gui_state::MenuSlot,
        shift: bool,
        gather: bool,
    ) -> bool {
        use petramond::gui_state::MenuSlot;
        let v = &self.menu_view;
        match slot {
            MenuSlot::Inventory(_) => {
                // Shift-move and gather both target the open container, so
                // both are unpredictable exactly while one is open; a plain
                // click never leaves the inventory.
                !(shift || gather) || v.container.is_none()
            }
            MenuSlot::Container(_) => {
                !shift && !gather && v.container.is_some() && v.container_kind.is_some()
            }
            _ => false,
        }
    }

    /// Apply click prediction; callers gate on
    /// [`menu_click_is_predictable`](Self::menu_click_is_predictable), so
    /// every arm here matches what `ContainerMenu::click` will do server-side:
    /// container-slot arms run the same external-slot primitives its generic
    /// decode runs, against the mirror cell instead of the world container.
    fn predict_menu_click(
        &mut self,
        slot: petramond::gui_state::MenuSlot,
        button: petramond::gui_state::PointerButton,
        shift: bool,
        gather: bool,
    ) {
        use petramond::gui_state::PointerButton;
        use petramond::gui_state::MenuSlot;
        let secondary = button == PointerButton::Secondary;
        let inv = &mut self.self_view.inventory;
        match slot {
            MenuSlot::Inventory(i) => {
                if shift {
                    inv.shift_move_slot(i);
                } else if gather {
                    inv.collect_to_cursor();
                } else if secondary {
                    inv.right_click_slot(i);
                } else {
                    inv.click_slot(i);
                }
            }
            MenuSlot::Container(i) => {
                let Some(kind) = self.menu_view.container_kind else {
                    return;
                };
                let specs = petramond::gui::documents::container_slot_specs(kind);
                if let Some(cell) = self
                    .menu_view
                    .container
                    .as_mut()
                    .and_then(|container| container.slots.get_mut(i))
                {
                    inv.click_container_cell(specs.get(i), cell, secondary);
                }
            }
            _ => {}
        }
    }

    /// Latch a hit-tested container click for the next game tick: resolved by
    /// the App to a [`MenuSlot`](petramond::gui_state::MenuSlot), a button, Shift, and
    /// its double-click `gather` verdict, shipped as a `MenuClick` message and
    /// applied in arrival order by the tick's menu stage. Optimistically
    /// mutates the predicted inventory when the ledger has room.
    pub fn menu_click(
        &mut self,
        slot: petramond::gui_state::MenuSlot,
        button: petramond::gui_state::PointerButton,
        shift: bool,
        gather: bool,
    ) {
        // Clicks the prediction cannot faithfully apply ride track-only: no
        // inventory clone, no snapshot slot burned, nothing to roll back. A
        // container-slot click mutates the open menu mirror too, so its
        // rollback unit spans both stores.
        let (can, request_id) = if self.menu_click_is_predictable(slot, shift, gather) {
            if matches!(slot, petramond::gui_state::MenuSlot::Inventory(_)) {
                self.begin_inventory_prediction()
            } else {
                self.begin_menu_prediction()
            }
        } else {
            (false, self.prediction.begin_track_only())
        };
        if can {
            self.predict_menu_click(slot, button, shift, gather);
        }
        self.outbox.push(ClientToServer::MenuClick {
            slot: MenuSlotWire::from_menu_slot(&slot),
            button: petramond::net::protocol::button_to_wire(button),
            shift,
            gather,
            request_id,
        });
    }

    /// Drop from the hovered menu slot (Q / Ctrl-Q by default). Inventory
    /// cells can be predicted locally; container and transient output cells
    /// ride track-only until the authoritative menu tick applies them.
    pub fn menu_drop(&mut self, slot: petramond::gui_state::MenuSlot, all: bool) {
        self.local_hand_threw |= self.menu_slot_has_stack(slot);
        let (can, request_id) = if matches!(slot, petramond::gui_state::MenuSlot::Inventory(_)) {
            self.begin_inventory_prediction()
        } else {
            (false, self.prediction.begin_track_only())
        };
        if can {
            if let petramond::gui_state::MenuSlot::Inventory(i) = slot {
                self.self_view.inventory.take_slot_for_drop(i, all);
            }
        }
        self.outbox.push(ClientToServer::MenuDrop {
            slot: MenuSlotWire::from_menu_slot(&slot),
            all,
            request_id,
        });
    }

    fn menu_slot_has_stack(&self, slot: petramond::gui_state::MenuSlot) -> bool {
        use petramond::gui_state::MenuSlot;
        match slot {
            MenuSlot::Inventory(i) => self.self_view.inventory.slot(i).is_some(),
            MenuSlot::CraftResult => self.menu_view.craft_output.is_some(),
            MenuSlot::Container(i) => self
                .menu_view
                .container
                .as_ref()
                .and_then(|container| container.slots.get(i).copied().flatten())
                .is_some(),
            MenuSlot::Widget(_) => false,
        }
    }
}
