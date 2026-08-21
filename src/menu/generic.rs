//! Container slot behavior for the open GUI session — ONE implementation for
//! every slot-bearing target. The engine owns the mechanics (click/place/
//! split, take-only outputs, shift-routing by the [`SlotSpec`] item tags,
//! gather double-clicks); what the slots MEAN stays with the container's
//! owner — engine machine state like the furnace's, or the opening mod's tick
//! logic. The chest rides the same path as a pack document — its slot
//! semantics come from its own GUI document, not from a hardcoded role — and
//! only the furnace keeps an engine-owned spec set, because its filters are
//! machine state rather than authored layout.

use super::{ContainerMenu, ContainerTarget};
use crate::world::World;
use petramond_math::math::IVec3;
use petramond_world::container::{Container, SlotSpec};
use petramond_world::furnace::{SLOT_FUEL, SLOT_INPUT, SLOT_OUTPUT};
use petramond_world::gui_state::ContainerView;
use petramond_world::gui_state::PointerButton;
use petramond_world::inventory::{merge_stack, Inventory};
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

impl ContainerMenu {
    /// The open session's container slots for the render view, or `None` when
    /// no block-backed container is open. The engine chest and a pack's own
    /// container publish through this ONE view — the chest is not a kind the
    /// render path knows by name. (The furnace still draws its own view; it
    /// carries cook/burn gauges the plain slot view has no room for.)
    pub fn open_container_view(&self, world: &World) -> Option<ContainerView> {
        let pos = self.container_pos()?;
        Some(ContainerView {
            slots: world.container_at(pos)?.slots.clone(),
        })
    }

    /// The open session's container position: the block a block-backed kind's
    /// session is anchored on (`None` for a programmatic open or a transient
    /// station with no block-entity slots).
    pub(super) fn container_pos(&self) -> Option<IVec3> {
        match self.target {
            ContainerTarget::Gui { kind, pos } if ContainerTarget::kind_block_backed(kind) => pos,
            _ => None,
        }
    }

    /// The open session's slot semantics (empty when no slot-bearing GUI is
    /// up). Every container — the engine chest included — derives them from
    /// its own GUI DOCUMENT's `container` slots, exactly as a pack's does;
    /// only the furnace keeps an engine-owned set, because its filters are
    /// machine state (smeltable/fuel/output) rather than authored layout.
    pub(super) fn slot_specs(&self) -> Arc<Vec<SlotSpec>> {
        match self.target.kind() {
            Some(petramond_world::gui_state::GuiKind::Furnace) => furnace_slot_specs(),
            Some(kind) => crate::gui::documents::container_slot_specs(kind),
            None => Arc::default(),
        }
    }

    fn edit_open_container(
        &self,
        world: &mut World,
        inv: &mut Inventory,
        edit: impl FnOnce(&mut Inventory, &mut Container),
    ) {
        let Some(pos) = self.container_pos() else {
            return;
        };
        if let Some(container) = world.container_at_mut(pos) {
            edit(inv, container);
        }
        world.mark_chunk_modified(pos);
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
        self.edit_open_container(world, inv, |inv, c| {
            let Some(slot) = c.slots.get_mut(i) else {
                return;
            };
            inv.click_container_cell(specs.get(i), gui, slot, secondary);
        });
    }

    fn container_shift_slot(&self, world: &mut World, inv: &mut Inventory, i: usize) {
        self.edit_open_container(world, inv, |inv, c| {
            if let Some(slot) = c.slots.get_mut(i) {
                inv.pull_from(slot);
            }
        });
    }

    /// The gather a double-click performs: sweep matching items from the open
    /// container's slots AND the inventory onto the cursor — or the inventory
    /// alone when no block-entity container is open.
    pub(super) fn collect_to_cursor(&self, world: &mut World, inv: &mut Inventory) {
        if self.container_pos().is_some() {
            self.collect_to_cursor_in_container(world, inv);
        } else {
            inv.collect_to_cursor();
        }
    }

    fn collect_to_cursor_in_container(&self, world: &mut World, inv: &mut Inventory) {
        self.edit_open_container(world, inv, |inv, c| {
            inv.collect_to_cursor_including(&mut c.slots)
        });
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
        let Some(pos) = self.container_pos() else {
            inv.shift_move_slot(i);
            return;
        };
        let Some(item) = inv.slot(i).map(|s| s.item) else {
            return;
        };
        let specs = self.slot_specs();
        if !specs.iter().any(|s| s.routes(item, s.accepts_mask(gui))) {
            inv.shift_move_slot(i);
            return;
        }
        if let Some(container) = world.container_at_mut(pos) {
            let Some(src) = inv.slot_mut(i) else {
                return;
            };
            let by_filter = (0..container.slots.len()).filter(|&s| {
                specs
                    .get(s)
                    .is_some_and(|spec| spec.routes_by_filter(item, spec.accepts_mask(gui)))
            });
            let open = (0..container.slots.len()).filter(|&s| {
                specs.get(s).is_some_and(|spec| {
                    let mask = spec.accepts_mask(gui);
                    !spec.routes_by_filter(item, mask) && spec.routes(item, mask)
                })
            });
            let routed: Vec<usize> = by_filter.chain(open).collect();
            // Merge-then-fill over the routed order (the inventory's
            // `insert_into_slots` discipline): top up matching stacks first,
            // then open empties.
            for &s in &routed {
                if src.is_none() {
                    break;
                }
                if container.slots[s].is_some() {
                    merge_stack(src, &mut container.slots[s]);
                }
            }
            for &s in &routed {
                if src.is_none() {
                    break;
                }
                if container.slots[s].is_none() {
                    merge_stack(src, &mut container.slots[s]);
                }
            }
        }
        world.mark_chunk_modified(pos);
    }
}
