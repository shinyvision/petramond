use super::slots::slot_specs_for_kind;
use super::{ContainerMenu, ContainerTarget, MenuAnchor};
use crate::world::ServerWorld;
use petramond_world::container::{Container, SlotSpec};
use petramond_world::gui_state::ContainerView;
use petramond_world::gui_state::PointerButton;
use petramond_world::inventory::Inventory;
use std::sync::Arc;

impl ContainerMenu {
    pub fn open_container_view(&self, world: &ServerWorld) -> Option<ContainerView> {
        Some(ContainerView {
            slots: self.open_container(world)?.slots.clone(),
        })
    }

    pub(super) fn container_anchor(&self) -> Option<MenuAnchor> {
        match self.target {
            ContainerTarget::Gui { kind, anchor } if ContainerTarget::kind_anchor_backed(kind) => {
                anchor
            }
            _ => None,
        }
    }

    pub(super) fn open_container<'a>(&self, world: &'a ServerWorld) -> Option<&'a Container> {
        self.container_anchor()?.container(world)
    }

    pub(super) fn edit_open_container<R>(
        &self,
        world: &mut ServerWorld,
        edit: impl FnOnce(&mut Container) -> R,
    ) -> Option<R> {
        self.container_anchor()?.edit_container(world, edit)
    }

    pub(super) fn slot_specs(&self) -> Arc<Vec<SlotSpec>> {
        match self.target.kind() {
            Some(kind) => slot_specs_for_kind(kind),
            None => Arc::default(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn container_slot_interaction(
        &self,
        world: &mut ServerWorld,
        inv: &mut Inventory,
        gui: Option<&petramond_world::gui_state::GuiStateMap>,
        i: usize,
        button: PointerButton,
        shift: bool,
        gather: bool,
    ) {
        if shift {
            self.container_shift_slot(world, inv, i);
        } else if gather {
            self.collect_to_cursor_in_container(world, inv);
        } else {
            self.container_click_slot(world, inv, gui, i, button == PointerButton::Secondary);
        }
    }

    fn container_click_slot(
        &self,
        world: &mut ServerWorld,
        inv: &mut Inventory,
        gui: Option<&petramond_world::gui_state::GuiStateMap>,
        i: usize,
        secondary: bool,
    ) {
        let specs = self.slot_specs();
        self.edit_open_container(world, |c| {
            let Some(slot) = c.slots.get_mut(i) else {
                return;
            };
            inv.click_container_cell(specs.get(i), gui, slot, secondary);
        });
    }

    fn container_shift_slot(&self, world: &mut ServerWorld, inv: &mut Inventory, i: usize) {
        self.edit_open_container(world, |c| {
            if let Some(slot) = c.slots.get_mut(i) {
                inv.pull_from(slot);
            }
        });
    }

    pub(super) fn collect_to_cursor(&self, world: &mut ServerWorld, inv: &mut Inventory) {
        if self.container_anchor().is_some() {
            self.collect_to_cursor_in_container(world, inv);
        } else {
            inv.collect_to_cursor();
        }
    }

    fn collect_to_cursor_in_container(&self, world: &mut ServerWorld, inv: &mut Inventory) {
        self.edit_open_container(world, |c| inv.collect_to_cursor_including(&mut c.slots));
    }

    /// Shift-click of slot `i` with a container open: routes into filter-matching slots first (fuel
    /// goes to the fuel slot even past an open storage cell), then unfiltered storage slots, in
    /// document order. Tops up matching stacks before opening an empty slot, so it merges instead
    /// of fragmenting. An item that no slot routes falls back to the normal hotbar/grid move.
    /// Take-only outputs are never targets.
    pub(super) fn container_shift_from_inventory(
        &self,
        world: &mut ServerWorld,
        inv: &mut Inventory,
        gui: Option<&petramond_world::gui_state::GuiStateMap>,
        i: usize,
    ) {
        if self.container_anchor().is_none() {
            inv.shift_move_slot(i);
            return;
        }
        let Some(item) = inv.slot(i).map(|s| s.item) else {
            return;
        };
        let specs = self.slot_specs();
        if !specs.iter().any(|s| s.routes(item, s.accepts_mask(gui))) {
            inv.shift_move_slot(i);
            return;
        }
        self.edit_open_container(world, |container| {
            if let Some(src) = inv.slot_mut(i) {
                petramond_world::container::route_into(src, &mut container.slots, &specs, gui);
            }
        });
    }
}
