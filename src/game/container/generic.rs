//! Container slot behavior for the open GUI session — ONE implementation for
//! every slot-bearing target. The engine owns the mechanics (click/place/
//! split, take-only outputs, shift-routing by the [`SlotSpec`] item tags,
//! gather double-clicks); what the slots MEAN stays with the container's
//! owner — engine machine state like the furnace's, or the opening mod's tick
//! logic. The chest and furnace ride the same path as mod documents: their
//! semantics are the engine-owned spec sets below, not hardcoded roles.

use super::{ContainerMenu, ContainerTarget};
use crate::container::{Container, SlotSpec};
use crate::controls::PointerButton;
use crate::furnace::{SLOT_FUEL, SLOT_INPUT, SLOT_OUTPUT};
use crate::gui::{ChestView, ContainerView};
use crate::inventory::{merge_stack, stack_onto_cursor, Inventory};
use crate::item::ItemTag;
use crate::mathh::IVec3;
use crate::world::chest::CHEST_SLOTS;
use crate::world::World;
use std::sync::{Arc, OnceLock};

/// The chest's semantics in the same [`SlotSpec`] language mod documents
/// speak: plain storage cells — no filters, no outputs.
fn chest_slot_specs() -> Arc<Vec<SlotSpec>> {
    static SPECS: OnceLock<Arc<Vec<SlotSpec>>> = OnceLock::new();
    SPECS
        .get_or_init(|| Arc::new(vec![SlotSpec::default(); CHEST_SLOTS]))
        .clone()
}

/// The furnace's semantics: a smeltable-filtered input, a fuel-filtered fuel
/// slot, and a take-only output, in the `SLOT_INPUT`/`SLOT_FUEL`/`SLOT_OUTPUT`
/// index convention.
fn furnace_slot_specs() -> Arc<Vec<SlotSpec>> {
    static SPECS: OnceLock<Arc<Vec<SlotSpec>>> = OnceLock::new();
    SPECS
        .get_or_init(|| {
            let mut specs = vec![SlotSpec::default(); crate::furnace::FURNACE_SLOTS];
            specs[SLOT_INPUT].accepts = vec![ItemTag::SMELTABLE];
            specs[SLOT_FUEL].accepts = vec![ItemTag::FUEL];
            specs[SLOT_OUTPUT].take_only = true;
            Arc::new(specs)
        })
        .clone()
}

impl ContainerMenu {
    /// The open mod GUI's container slots for the render view, or `None` when
    /// the session is not a mod GUI opened from a block with storage. (The
    /// chest and furnace draw through their own views.)
    pub(in crate::game) fn open_container_view(&self, world: &World) -> Option<ContainerView> {
        if !matches!(self.target, ContainerTarget::ModGui { .. }) {
            return None;
        }
        let pos = self.container_pos()?;
        Some(ContainerView {
            slots: world.container_at(pos)?.slots.clone(),
        })
    }

    /// The open chest's slots for the render view, or `None` when no chest is
    /// open.
    pub(in crate::game) fn open_chest_view(&self, world: &World) -> Option<ChestView> {
        let ContainerTarget::Chest(pos) = self.target else {
            return None;
        };
        let container = world.container_at(pos)?;
        let mut slots = [None; CHEST_SLOTS];
        for (dst, src) in slots.iter_mut().zip(&container.slots) {
            *dst = *src;
        }
        Some(ChestView { slots })
    }

    /// The open session's container position: the chest/furnace block, or the
    /// mod GUI's opening block (`None` for a programmatic open or a screen
    /// with no block-entity slots).
    fn container_pos(&self) -> Option<IVec3> {
        match self.target {
            ContainerTarget::Chest(pos) | ContainerTarget::Furnace(pos) => Some(pos),
            ContainerTarget::ModGui { pos, .. } => pos,
            _ => None,
        }
    }

    /// The open session's slot semantics (empty when no slot-bearing GUI is
    /// up): the engine sets for the chest/furnace, the document's for a mod
    /// GUI.
    fn slot_specs(&self) -> Arc<Vec<SlotSpec>> {
        match self.target {
            ContainerTarget::Chest(_) => chest_slot_specs(),
            ContainerTarget::Furnace(_) => furnace_slot_specs(),
            ContainerTarget::ModGui { kind, .. } => {
                crate::gui::documents::container_slot_specs(kind)
            }
            _ => Arc::default(),
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
            self.container_click_slot(world, inv, i, button == PointerButton::Secondary);
        }
    }

    fn container_click_slot(
        &self,
        world: &mut World,
        inv: &mut Inventory,
        i: usize,
        secondary: bool,
    ) {
        let specs = self.slot_specs();
        self.edit_open_container(world, inv, |inv, c| {
            let Some(slot) = c.slots.get_mut(i) else {
                return;
            };
            // A take-only output never accepts the cursor stack: any click
            // just moves the product out (the furnace-output read).
            if specs.get(i).is_some_and(|s| s.take_only) {
                if let Some(out) = *slot {
                    if stack_onto_cursor(inv.cursor_mut(), out) {
                        *slot = None;
                    }
                }
            } else if secondary {
                inv.right_click_external_slot(slot);
            } else {
                inv.click_external_slot(slot);
            }
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
        if !specs.iter().any(|s| s.routes(item)) {
            inv.shift_move_slot(i);
            return;
        }
        if let Some(container) = world.container_at_mut(pos) {
            let Some(src) = inv.slot_mut(i) else {
                return;
            };
            let by_filter = (0..container.slots.len())
                .filter(|&s| specs.get(s).is_some_and(|spec| spec.routes_by_filter(item)));
            let open = (0..container.slots.len()).filter(|&s| {
                specs
                    .get(s)
                    .is_some_and(|spec| !spec.routes_by_filter(item) && spec.routes(item))
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
