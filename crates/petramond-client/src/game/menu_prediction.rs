use super::Game;
use petramond::net::protocol::{ClientToServer, MenuSlotWire};
use petramond_world::gui_state::PointerButton;
use petramond_world::gui_state::{GuiKind, MenuSlot, MAX_MENU_DRAG_SLOTS};
use petramond_world::inventory::{plan_drag_distribution, slot_capacity};
use petramond_world::item::ItemStack;

impl Game {
    pub fn creative_pick(&mut self, item: petramond_world::item::ItemType) {
        self.creative_set_cursor(Some(item));
    }

    pub fn creative_discard_cursor(&mut self) {
        self.creative_set_cursor(None);
    }

    fn creative_set_cursor(&mut self, item: Option<petramond_world::item::ItemType>) {
        if !self.creative_mode() || item.is_some_and(|i| !i.creative_visible()) {
            return;
        }
        let (can, request_id) = self.begin_inventory_prediction();
        if can {
            *self.replica.self_view.inventory.cursor_mut() =
                item.map(|i| ItemStack::new(i, i.max_stack_size()));
        }
        self.net.queue(ClientToServer::CreativeCursor {
            item: item.map(|i| i.registry_name().into()),
            request_id,
        });
    }

    pub fn menu_drag(&mut self, kind: GuiKind, slots: Vec<MenuSlot>, button: PointerButton) {
        let slots: Vec<_> = slots.into_iter().take(MAX_MENU_DRAG_SLOTS).collect();
        if slots.len() < 2 {
            return;
        }

        let can_predict = self.prediction.can_predict();
        let snapshot = if can_predict {
            crate::game::prediction::PredictionSnapshot::Menu {
                inventory: self.replica.self_view.inventory.clone(),
                menu: self.replica.menu_view.clone(),
            }
        } else {
            crate::game::prediction::PredictionSnapshot::None
        };
        let request_id = self.prediction.begin(snapshot);
        if can_predict {
            self.predict_menu_drag(kind, &slots, button);
        }

        self.net.queue(ClientToServer::MenuDrag {
            slots: slots.iter().map(MenuSlotWire::from_menu_slot).collect(),
            button: petramond::net::protocol::button_to_wire(button),
            request_id,
        });
    }

    fn predict_menu_drag(&mut self, kind: GuiKind, slots: &[MenuSlot], button: PointerButton) {
        let Some(held) = self.replica.self_view.inventory.cursor().copied() else {
            return;
        };
        let specs = petramond::menu::slot_specs_for_kind(kind);
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
                .replica
                .self_view
                .inventory
                .raw_slots()
                .get(i)
                .map(|cell| slot_capacity(cell, held))
                .unwrap_or(0),
            MenuSlot::OffHand => {
                slot_capacity(&self.replica.self_view.inventory.off_hand().copied(), held)
            }
            MenuSlot::Container(i)
                if petramond_world::container::slot_admits(
                    specs,
                    i,
                    Some(held.item),
                    self.replica.menu_view.gui_state.as_deref(),
                ) =>
            {
                self.replica
                    .menu_view
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
        let inventory = &mut self.replica.self_view.inventory;
        let menu = &mut self.replica.menu_view;
        match slot {
            MenuSlot::Inventory(i) => {
                inventory.place_cursor_count_in_slot(i, wanted);
            }
            MenuSlot::OffHand => {
                let mut cell = inventory.take_off_hand();
                inventory.place_cursor_count_in_external_slot(&mut cell, wanted);
                *inventory.off_hand_mut() = cell;
            }
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
        let v = &self.replica.menu_view;
        match slot {
            MenuSlot::Inventory(_) => !(shift || gather) || v.container.is_none(),
            MenuSlot::OffHand => !gather || v.container.is_none(),
            MenuSlot::Container(i) => {
                !shift
                    && !gather
                    && v.container.is_some()
                    && v.container_kind.is_some()
                    && !self
                        .mask_decides(i, self.replica.self_view.inventory.cursor().map(|c| c.item))
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
        let Some(kind) = self.replica.menu_view.container_kind else {
            return false;
        };
        let Some(held) = held else {
            return false;
        };
        let specs = petramond::menu::slot_specs_for_kind(kind);
        let Some(spec) = specs.get(i) else {
            return false;
        };
        if spec.accepts_bind.is_none() {
            return false;
        }
        let mask = spec.accepts_mask(self.replica.menu_view.gui_state.as_deref());
        spec.admits(held, petramond_world::container::FULL_MASK) && !spec.admits(held, mask)
    }

    fn predict_menu_click(
        &mut self,
        slot: petramond_world::gui_state::MenuSlot,
        button: petramond_world::gui_state::PointerButton,
        shift: bool,
        gather: bool,
    ) {
        use petramond_world::gui_state::MenuSlot;
        use petramond_world::gui_state::PointerButton;
        let secondary = button == PointerButton::Secondary;
        let inv = &mut self.replica.self_view.inventory;
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
                let Some(kind) = self.replica.menu_view.container_kind else {
                    return;
                };
                let specs = petramond::menu::slot_specs_for_kind(kind);
                if let Some(cell) = self
                    .replica
                    .menu_view
                    .container
                    .as_mut()
                    .and_then(|container| container.slots.get_mut(i))
                {
                    inv.click_container_cell(
                        specs.get(i),
                        self.replica.menu_view.gui_state.as_deref(),
                        cell,
                        secondary,
                    );
                }
            }
            _ => {}
        }
    }

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
        self.net.queue(ClientToServer::MenuClick {
            slot: MenuSlotWire::from_menu_slot(&slot),
            button: petramond::net::protocol::button_to_wire(button),
            shift,
            gather,
            request_id,
        });
    }

    pub fn menu_drop(&mut self, slot: petramond_world::gui_state::MenuSlot, all: bool) {
        let held = self.menu_slot_has_stack(slot);
        self.hand.latch_throw(held);
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
                    self.replica.self_view.inventory.take_slot_for_drop(i, all);
                }
                petramond_world::gui_state::MenuSlot::OffHand => {
                    petramond_world::inventory::take_slot_stack(
                        self.replica.self_view.inventory.off_hand_mut(),
                        all,
                    );
                }
                _ => {}
            }
        }
        self.net.queue(ClientToServer::MenuDrop {
            slot: MenuSlotWire::from_menu_slot(&slot),
            all,
            request_id,
        });
    }

    /// The F gesture: swap the off-hand with `slot`, the selected hotbar slot in gameplay or the
    /// hovered slot in a menu.
    ///
    /// Predicted against the mirrors with the same `Inventory` primitives the server's decode
    /// runs. Container cells whose accepts-mask decides the refusal are track-only, because the
    /// mirror's mask is one round trip stale. Hovering the off-hand cell, a transient output or a
    /// widget sends nothing, since the server would refuse it too.
    pub fn menu_swap_off_hand(&mut self, slot: petramond_world::gui_state::MenuSlot) {
        use petramond_world::gui_state::MenuSlot;
        if self.local.player.is_spectator() {
            return;
        }
        let off_item = self.replica.self_view.inventory.off_hand().map(|s| s.item);
        let (can, request_id) = match slot {
            MenuSlot::Inventory(_) => self.begin_inventory_prediction(),
            MenuSlot::Container(i)
                if self.replica.menu_view.container.is_some()
                    && self.replica.menu_view.container_kind.is_some()
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
                    self.replica.self_view.inventory.swap_off_hand_with_slot(i);
                }
                MenuSlot::Container(i) => {
                    let kind = self.replica.menu_view.container_kind.expect("gated above");
                    let specs = petramond::menu::slot_specs_for_kind(kind);
                    if let Some(cell) = self
                        .replica
                        .menu_view
                        .container
                        .as_mut()
                        .and_then(|container| container.slots.get_mut(i))
                    {
                        self.replica.self_view.inventory.swap_off_hand_with_cell(
                            specs.get(i),
                            self.replica.menu_view.gui_state.as_deref(),
                            cell,
                        );
                    }
                }
                _ => {}
            }
        }
        self.net.queue(ClientToServer::MenuSwapOffHand {
            slot: MenuSlotWire::from_menu_slot(&slot),
            request_id,
        });
    }

    fn menu_slot_has_stack(&self, slot: petramond_world::gui_state::MenuSlot) -> bool {
        use petramond_world::gui_state::MenuSlot;
        match slot {
            MenuSlot::Inventory(i) => self.replica.self_view.inventory.slot(i).is_some(),
            MenuSlot::OffHand => self.replica.self_view.inventory.off_hand().is_some(),
            MenuSlot::CraftResult => self.replica.menu_view.craft_output.is_some(),
            MenuSlot::Container(i) => self
                .replica
                .menu_view
                .container
                .as_ref()
                .and_then(|container| container.slots.get(i).copied().flatten())
                .is_some(),
            MenuSlot::Widget(_) => false,
        }
    }
}
