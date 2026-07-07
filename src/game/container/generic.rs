//! Mod container slot behavior for the open mod GUI session.
//!
//! The engine owns the mechanics — click/place/split, take-only outputs,
//! shift-routing by the document's `accepts` item tags — while what the slots
//! MEAN stays with the owning mod's tick logic. Mirrors `container::chest`
//! plus the furnace's semantic touches, driven by the document's
//! [`SlotSpec`]s instead of hardcoded roles.

use super::{ContainerMenu, ContainerTarget};
use crate::furnace::{merge_stack, stack_onto_cursor};
use crate::gui::ContainerView;
use crate::inventory::Inventory;
use crate::mathh::IVec3;
use crate::container::{Container, SlotSpec};
use crate::world::World;
use std::sync::Arc;

impl ContainerMenu {
    /// The open mod GUI's container slots for the render view, or `None` when
    /// the session is not a mod GUI opened from a block with storage.
    pub(in crate::game) fn open_container_view(&self, world: &World) -> Option<ContainerView> {
        let pos = self.container_pos()?;
        Some(ContainerView {
            slots: world.container_at(pos)?.slots.clone(),
        })
    }

    /// The open mod GUI's container position (`None` for a programmatic open
    /// with no block, or when no mod GUI is up).
    fn container_pos(&self) -> Option<IVec3> {
        match self.target {
            ContainerTarget::ModGui { pos, .. } => pos,
            _ => None,
        }
    }

    /// The open mod GUI document's slot semantics (empty when none is up).
    fn slot_specs(&self) -> Arc<Vec<SlotSpec>> {
        match self.target {
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

    pub(super) fn container_click_slot(
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

    pub(super) fn container_shift_slot(&self, world: &mut World, inv: &mut Inventory, i: usize) {
        self.edit_open_container(world, inv, |inv, c| {
            if let Some(slot) = c.slots.get_mut(i) {
                inv.pull_from(slot);
            }
        });
    }

    /// Shift-click of inventory slot `i` with a mod GUI open: route the stack
    /// into the container's slots — filter-matching slots first (a fuel goes
    /// to the fuel-filtered slot even past an open storage cell), then
    /// unfiltered storage slots, in document order. An item no slot routes
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
            let by_filter: Vec<usize> = (0..container.slots.len())
                .filter(|&s| specs.get(s).is_some_and(|spec| spec.routes_by_filter(item)))
                .collect();
            let open: Vec<usize> = (0..container.slots.len())
                .filter(|&s| {
                    specs
                        .get(s)
                        .is_some_and(|spec| !spec.routes_by_filter(item) && spec.routes(item))
                })
                .collect();
            for s in by_filter.into_iter().chain(open) {
                if src.is_none() {
                    break;
                }
                merge_stack(src, &mut container.slots[s]);
            }
        }
        world.mark_chunk_modified(pos);
    }
}
