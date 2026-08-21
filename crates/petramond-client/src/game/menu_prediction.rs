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
use petramond_world::gui_state::PointerButton;
use petramond_world::gui_state::{GuiKind, MenuSlot, MAX_MENU_DRAG_SLOTS};
use petramond_world::inventory::{plan_drag_distribution, slot_capacity};
use petramond_world::item::ItemStack;
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
        specs: &[petramond_world::container::SlotSpec],
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
            MenuSlot::OffHand => {
                slot_capacity(&self.self_view.inventory.off_hand().copied(), held)
            }
            // The same question the server's `drag_capacity` asks, through the
            // same helper: a slot one side counts and the other refuses does
            // not just snap that leg back — the split is by the NUMBER of
            // destinations, so every other slot of the drag lands wrong too.
            MenuSlot::Container(i)
                if petramond_world::container::slot_admits(
                    specs,
                    i,
                    Some(held.item),
                    self.menu_view.gui_state.as_deref(),
                ) =>
            {
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
        specs: &[petramond_world::container::SlotSpec],
        slot: MenuSlot,
        wanted: u8,
    ) {
        let inventory = &mut self.self_view.inventory;
        let menu = &mut self.menu_view;
        match slot {
            MenuSlot::Inventory(i) => {
                inventory.place_cursor_count_in_slot(i, wanted);
            }
            MenuSlot::OffHand => {
                let mut cell = inventory.take_off_hand();
                inventory.place_cursor_count_in_external_slot(&mut cell, wanted);
                *inventory.off_hand_mut() = cell;
            }
            // The same question the server's drag asks, through the same
            // helper. A leg the client mirrors and the server refuses is a
            // stack that visibly lands and then snaps back.
            MenuSlot::Container(i)
                if petramond_world::container::slot_admits(
                    specs,
                    i,
                    inventory.cursor().map(|held| held.item),
                    menu.gui_state.as_deref(),
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
        slot: petramond_world::gui_state::MenuSlot,
        shift: bool,
        gather: bool,
    ) -> bool {
        use petramond_world::gui_state::MenuSlot;
        let v = &self.menu_view;
        match slot {
            MenuSlot::Inventory(_) => {
                // Shift-move and gather both target the open container, so
                // both are unpredictable exactly while one is open; a plain
                // click never leaves the inventory.
                !(shift || gather) || v.container.is_none()
            }
            MenuSlot::OffHand => {
                // The off-hand's shift-move always ships into the OWN grid
                // (nothing container-routes it), so only a gather with an
                // open container is unpredictable.
                !gather || v.container.is_none()
            }
            MenuSlot::Container(i) => {
                !shift
                    && !gather
                    && v.container.is_some()
                    && v.container_kind.is_some()
                    && !self.mask_decides(i, self.self_view.inventory.cursor().map(|c| c.item))
            }
            _ => false,
        }
    }

    /// Whether a runtime `accepts` MASK is the deciding voice on placing
    /// `held` into container cell `i`: the authored filters admit it but the
    /// currently MIRRORED mask refuses. The mirror is one round trip stale,
    /// so a mask-decided refusal is not the client's call to make — the
    /// gesture rides track-only and the server (whose mask is current)
    /// decides. Without this, inserting a tool and quickly dropping a gem
    /// into a socket the tool just unlocked gets locally refused — the icon
    /// never appears — until the forced sync overrules; a prediction that can
    /// only be wrong in the refusing direction is worse than no prediction.
    /// Genuine refusals look identical either way (nothing moves), just one
    /// round trip later. `held` is whatever stack the gesture would deposit:
    /// the cursor for clicks/drags, the off-hand for the F swap.
    fn mask_decides(&self, i: usize, held: Option<petramond_world::item::ItemType>) -> bool {
        let Some(kind) = self.menu_view.container_kind else {
            return false;
        };
        let Some(held) = held else {
            return false;
        };
        let specs = petramond::gui::documents::container_slot_specs(kind);
        let Some(spec) = specs.get(i) else {
            return false;
        };
        if spec.accepts_bind.is_none() {
            return false;
        }
        let mask = spec.accepts_mask(self.menu_view.gui_state.as_deref());
        spec.admits(held, petramond_world::container::FULL_MASK) && !spec.admits(held, mask)
    }

    /// Apply click prediction; callers gate on
    /// [`menu_click_is_predictable`](Self::menu_click_is_predictable), so
    /// every arm here matches what `ContainerMenu::click` will do server-side:
    /// container-slot arms run the same external-slot primitives its generic
    /// decode runs, against the mirror cell instead of the world container.
    fn predict_menu_click(
        &mut self,
        slot: petramond_world::gui_state::MenuSlot,
        button: petramond_world::gui_state::PointerButton,
        shift: bool,
        gather: bool,
    ) {
        use petramond_world::gui_state::PointerButton;
        use petramond_world::gui_state::MenuSlot;
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
            MenuSlot::OffHand => {
                // The same take/click/put shape the server's decode runs.
                if shift {
                    inv.shift_move_off_hand();
                } else if gather {
                    inv.collect_to_cursor();
                } else {
                    let mut cell = inv.take_off_hand();
                    if secondary {
                        inv.right_click_external_slot(&mut cell);
                    } else {
                        inv.click_external_slot(&mut cell);
                    }
                    *inv.off_hand_mut() = cell;
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
                    inv.click_container_cell(
                        specs.get(i),
                        self.menu_view.gui_state.as_deref(),
                        cell,
                        secondary,
                    );
                }
            }
            _ => {}
        }
    }

    /// Latch a hit-tested container click for the next game tick: resolved by
    /// the App to a [`MenuSlot`], a button, Shift, and
    /// its double-click `gather` verdict, shipped as a `MenuClick` message and
    /// applied in arrival order by the tick's menu stage. Optimistically
    /// mutates the predicted inventory when the ledger has room.
    pub fn menu_click(
        &mut self,
        slot: petramond_world::gui_state::MenuSlot,
        button: petramond_world::gui_state::PointerButton,
        shift: bool,
        gather: bool,
    ) {
        // Clicks the prediction cannot faithfully apply ride track-only: no
        // inventory clone, no snapshot slot burned, nothing to roll back. A
        // container-slot click mutates the open menu mirror too, so its
        // rollback unit spans both stores.
        let (can, request_id) = if self.menu_click_is_predictable(slot, shift, gather) {
            if matches!(
                slot,
                petramond_world::gui_state::MenuSlot::Inventory(_)
                    | petramond_world::gui_state::MenuSlot::OffHand
            ) {
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
    pub fn menu_drop(&mut self, slot: petramond_world::gui_state::MenuSlot, all: bool) {
        self.local_hand_threw |= self.menu_slot_has_stack(slot);
        let (can, request_id) = if matches!(
            slot,
            petramond_world::gui_state::MenuSlot::Inventory(_)
                | petramond_world::gui_state::MenuSlot::OffHand
        ) {
            self.begin_inventory_prediction()
        } else {
            (false, self.prediction.begin_track_only())
        };
        if can {
            match slot {
                petramond_world::gui_state::MenuSlot::Inventory(i) => {
                    self.self_view.inventory.take_slot_for_drop(i, all);
                }
                petramond_world::gui_state::MenuSlot::OffHand => {
                    petramond_world::inventory::take_slot_stack(
                        self.self_view.inventory.off_hand_mut(),
                        all,
                    );
                }
                _ => {}
            }
        }
        self.outbox.push(ClientToServer::MenuDrop {
            slot: MenuSlotWire::from_menu_slot(&slot),
            all,
            request_id,
        });
    }

    /// The F gesture: swap the off-hand with `slot` — the selected hotbar
    /// slot in gameplay ([`Game::swap_off_hand`] names it), the hovered slot
    /// in a menu. Predicted with the SAME `Inventory` primitives the server's
    /// decode runs (`swap_off_hand_with_slot` / the spec-gated
    /// `swap_off_hand_with_cell`), against the mirrors. Container cells whose
    /// runtime accepts-MASK is the deciding refusal ride track-only — the
    /// mirror's mask is one round trip stale (the click rule). Hovering the
    /// off-hand cell itself, a transient output, or a widget is a no-op the
    /// server would also refuse: nothing is sent at all.
    pub fn menu_swap_off_hand(&mut self, slot: petramond_world::gui_state::MenuSlot) {
        use petramond_world::gui_state::MenuSlot;
        // The server refuses a spectator's swap; don't burn a request on a
        // known deny.
        if self.player.is_spectator() {
            return;
        }
        let off_item = self.self_view.inventory.off_hand().map(|s| s.item);
        let (can, request_id) = match slot {
            MenuSlot::Inventory(_) => self.begin_inventory_prediction(),
            MenuSlot::Container(i)
                if self.menu_view.container.is_some()
                    && self.menu_view.container_kind.is_some()
                    && !self.mask_decides(i, off_item) =>
            {
                self.begin_menu_prediction()
            }
            MenuSlot::Container(_) => (false, self.prediction.begin_track_only()),
            MenuSlot::OffHand | MenuSlot::CraftResult | MenuSlot::Widget(_) => return,
        };
        if can {
            match slot {
                MenuSlot::Inventory(i) => {
                    self.self_view.inventory.swap_off_hand_with_slot(i);
                }
                MenuSlot::Container(i) => {
                    let kind = self.menu_view.container_kind.expect("gated above");
                    let specs = petramond::gui::documents::container_slot_specs(kind);
                    if let Some(cell) = self
                        .menu_view
                        .container
                        .as_mut()
                        .and_then(|container| container.slots.get_mut(i))
                    {
                        self.self_view.inventory.swap_off_hand_with_cell(
                            specs.get(i),
                            self.menu_view.gui_state.as_deref(),
                            cell,
                        );
                    }
                }
                _ => {}
            }
        }
        self.outbox.push(ClientToServer::MenuSwapOffHand {
            slot: MenuSlotWire::from_menu_slot(&slot),
            request_id,
        });
    }

    fn menu_slot_has_stack(&self, slot: petramond_world::gui_state::MenuSlot) -> bool {
        use petramond_world::gui_state::MenuSlot;
        match slot {
            MenuSlot::Inventory(i) => self.self_view.inventory.slot(i).is_some(),
            MenuSlot::OffHand => self.self_view.inventory.off_hand().is_some(),
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
