//! Container slot behavior for the open GUI session — ONE implementation for
//! every slot-bearing target. The engine owns the mechanics (click/place/
//! split, take-only outputs, shift-routing by the [`SlotSpec`] item tags,
//! gather double-clicks); what the slots MEAN stays with the container's
//! owner — engine machine state like the furnace's, or the opening mod's tick
//! logic. The chest rides the same path as a pack document — its slot
//! semantics come from its own GUI document, not from a hardcoded role — and
//! only the furnace keeps an engine-owned spec set, because its filters are
//! machine state rather than authored layout.

use super::{ContainerMenu, ContainerTarget, MenuAnchor};
use crate::world::World;
use petramond_world::container::{Container, SlotSpec};
use petramond_world::furnace::{SLOT_FUEL, SLOT_INPUT, SLOT_OUTPUT};
use petramond_world::gui_state::ContainerView;
use petramond_world::gui_state::PointerButton;
use petramond_world::inventory::Inventory;
use petramond_world::item::ItemTag;
use std::sync::{Arc, OnceLock};

/// The furnace's semantics: a smeltable-filtered input, a fuel-filtered fuel
/// slot, and a take-only output, in the `SLOT_INPUT`/`SLOT_FUEL`/`SLOT_OUTPUT`
/// index convention.
fn furnace_slot_specs() -> Arc<Vec<SlotSpec>> {
    static SPECS: OnceLock<Arc<Vec<SlotSpec>>> = OnceLock::new();
    SPECS
        .get_or_init(|| {
            let mut specs = vec![SlotSpec::default(); petramond_world::furnace::FURNACE_SLOTS];
            specs[SLOT_INPUT].accepts = vec![petramond_world::container::SlotFilter::Tag(
                ItemTag::SMELTABLE,
            )];
            specs[SLOT_FUEL].accepts =
                vec![petramond_world::container::SlotFilter::Tag(ItemTag::FUEL)];
            specs[SLOT_OUTPUT].take_only = true;
            Arc::new(specs)
        })
        .clone()
}

/// Slot admission shared by player menus and automated container transfers.
pub fn slot_specs_for_kind(kind: petramond_world::gui_state::GuiKind) -> Arc<Vec<SlotSpec>> {
    if kind == petramond_world::gui_state::GuiKind::Furnace {
        furnace_slot_specs()
    } else {
        crate::gui::documents::container_slot_specs(kind)
    }
}

impl ContainerMenu {
    /// The open session's container slots for the render view, or `None` when
    /// no anchor-backed container is open. The engine chest and a pack's own
    /// container publish through this ONE view — the chest is not a kind the
    /// render path knows by name. (The furnace still draws its own view; it
    /// carries cook/burn gauges the plain slot view has no room for.)
    pub fn open_container_view(&self, world: &World) -> Option<ContainerView> {
        Some(ContainerView {
            slots: self.open_container(world)?.slots.clone(),
        })
    }

    /// What holds the open session's container slots: the block or mob an
    /// anchor-backed kind's session was opened on (`None` for an unanchored
    /// open or a transient station, whose stacks live on the menu).
    pub(super) fn container_anchor(&self) -> Option<MenuAnchor> {
        match self.target {
            ContainerTarget::Gui { kind, anchor } if ContainerTarget::kind_anchor_backed(kind) => {
                anchor
            }
            _ => None,
        }
    }

    /// The open session's backing container, read-only.
    pub(super) fn open_container<'a>(&self, world: &'a World) -> Option<&'a Container> {
        self.container_anchor()?.container(world)
    }

    /// The ONE write path to the open session's backing container, whatever
    /// it is anchored on.
    pub(super) fn edit_open_container<R>(
        &self,
        world: &mut World,
        edit: impl FnOnce(&mut Container) -> R,
    ) -> Option<R> {
        self.container_anchor()?.edit_container(world, edit)
    }

    /// The open session's slot semantics (empty when no slot-bearing GUI is
    /// up). Every container — the engine chest included — derives them from
    /// its own GUI DOCUMENT's `container` slots, exactly as a pack's does;
    /// only the furnace keeps an engine-owned set, because its filters are
    /// machine state (smeltable/fuel/output) rather than authored layout.
    pub(super) fn slot_specs(&self) -> Arc<Vec<SlotSpec>> {
        match self.target.kind() {
            Some(kind) => slot_specs_for_kind(kind),
            None => Arc::default(),
        }
    }

    /// One container slot's full click decode: shift quick-moves the slot to
    /// the inventory, a gather double-click sweeps matching items onto the
    /// cursor, otherwise a left/right click (take-only outputs only ever
    /// give). The single entry the dispatcher routes every chest, furnace,
    /// and mod container slot through.
    pub(super) fn container_slot_interaction(
        &self,
        world: &mut World,
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
        world: &mut World,
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

    fn container_shift_slot(&self, world: &mut World, inv: &mut Inventory, i: usize) {
        self.edit_open_container(world, |c| {
            if let Some(slot) = c.slots.get_mut(i) {
                inv.pull_from(slot);
            }
        });
    }

    /// The gather a double-click performs: sweep matching items from the open
    /// container's slots AND the inventory onto the cursor — or the inventory
    /// alone when no block-entity container is open.
    pub(super) fn collect_to_cursor(&self, world: &mut World, inv: &mut Inventory) {
        if self.container_anchor().is_some() {
            self.collect_to_cursor_in_container(world, inv);
        } else {
            inv.collect_to_cursor();
        }
    }

    fn collect_to_cursor_in_container(&self, world: &mut World, inv: &mut Inventory) {
        self.edit_open_container(world, |c| inv.collect_to_cursor_including(&mut c.slots));
    }

    /// Shift-click of inventory slot `i` with a container GUI open: route the
    /// stack into the container's slots — filter-matching slots first (a fuel
    /// goes to the fuel-filtered slot even past an open storage cell), then
    /// unfiltered storage slots, in document order. Within the routed order,
    /// matching stacks are topped up before an empty slot is opened, so a
    /// shifted stack merges instead of fragmenting. An item no slot routes
    /// falls back to the ordinary hotbar↔grid move; take-only outputs are
    /// never targets.
    pub(super) fn container_shift_from_inventory(
        &self,
        world: &mut World,
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
