//! Anvil: tool AUGMENTS, not casting.
//!
//! Tool goes in, raw material goes in (diamonds, gold ingots, a hushjaw's tooth), anvil fits the
//! augment. No "diamond tip" item ever exists - the tip is just overlay art plus a record on the
//! tool. Same deal as mould/head/tool: material, sprite mark, and augmented tool are one object at
//! three stages. No craftable middle step leaks out.
//!
//! SOCKETS. A tool has one socket by construction, row can declare more as lockable
//! (`forge:augment_slots` `{"family"?, "lockable"?}`). Locked sockets carve open per tool by
//! dropping a socket material (anything with `forge:socket_key` - the petramond gem) into a socket
//! cell. Next locked one opens on the machine's step, no button. Carved count lives on the tool's
//! own record - an investment in that tool, not something the player carries around.
//!
//! Augmented tool is just instance data on the ordinary stack, never a new item row. Anvil stamps
//! three keys and lets the engine do the rest.
//!
//! - `forge:augments` - our record of carved sockets and installed augments. Format and read/write
//!   logic live in [`crate::augments`]; the machine just stamps it.
//! - `petramond:tool` - engine's per-stack override, stated as absolutes: harvest gate is max tier
//!   across base and every augment, speed and damage are base tool's values times every augment's
//!   multiplier. Every apply recomputes the whole override from scratch, never incrementally, so
//!   order doesn't matter and a rebalanced ladder re-stamps correctly next visit.
//! - `petramond:overlay` - overlay art item names, composited in order over the tool sprite
//!   everywhere it's rendered. Art is authored in position on its own transparent 16x16, no
//!   coordinate crosses a boundary. Recomputed with the rest, so renamed or re-familied art heals
//!   on next apply.
//!
//! Which tools take which augments is data on both sides. Tool item carries `forge:augment_slots`;
//! augment material carries `forge:augment` - a list of fits (tool kind, edge tier, multipliers,
//! material cost, overlay item, optional gentle behaviour grant), one per tool kind it augments.
//! Another pack can extend either side with rows alone, including its own socket material. Both
//! tables get read in [`rows`].
//!
//! Application is staged, not automatic: valid materials sitting in open socket cells publish a
//! preview, but only clicking the panel's Augment button (recorded by [`AnvilSpec::request_apply`],
//! honoured next step) actually consumes them and transforms the tool. Socket carving is the
//! exception: dropping a gem on a locked slot carves it immediately, without a second confirmation.

mod gestures;
mod panel;
mod rows;
mod workstation;

use std::collections::{HashMap, HashSet};

use mod_sdk::*;

use machine_core::{write_changed_slots, Caches, Machine, MachineSpec, Presentation, StepCtx};

use crate::augments::Record;
use crate::keys;
use rows::{Fit, ToolSlots, ToolStats};

pub(crate) use rows::WearOn;

const STATE_KEY: &str = "forge:anvil_state";

const SLOT_TOOL: usize = 0;
const SOCKETS: usize = 4;
const _: () = assert!(keys::anvil::SOCKET_CELLS.len() == SOCKETS);
const SLOTS: usize = 1 + SOCKETS;
/// Sockets before any carving happens.
const BASE_SOCKETS: u8 = 1;

const VALUE_CAP: usize = 128;

const WEAR_STREAM: &str = "anvil_wear";

use crate::keys::{SOUND_AUGMENT, SOUND_SOCKET};

/// Socket-cell accepts masks (`bind.accepts` on the panel's socket slots): bits over the cells'
/// authored filter list, `[forge:augment, forge:socket_key]` in document order. The mask makes a
/// cell's state an admission rule on both mirrors. Occupied or absent sockets refuse inserts as if
/// the slot weren't there, and locked ones take only the carving gem, so items never land and sit
/// inert. Nothing at runtime checks that the document's filter order matches these bits;
/// `tests::the_panels_socket_cells_author_the_filters_these_bits_index` guards it.
const ACC_NONE: i32 = 0;
const ACC_AUGMENT: i32 = 1 << 0;
const ACC_SOCKET: i32 = 1 << 1;

enum CellState<'a> {
    Occupied(&'a str),
    Open,
    Locked,
    Absent,
}

pub type Anvil = Machine<AnvilSpec>;

#[derive(Default)]
pub struct AnvilSpec {
    augments: HashMap<String, Vec<Fit>>,
    by_identity: HashMap<String, Vec<Fit>>,
    material_of: HashMap<String, String>,
    names: HashMap<String, String>,
    tools: HashMap<String, (ToolSlots, ToolStats)>,
    socket_items: HashSet<String>,
    nondestructive: HashSet<String>,
    pending: HashSet<[i32; 3]>,
    watched: HashMap<[i32; 3], Vec<PlayerId>>,
}

impl AnvilSpec {
    pub fn request_apply(&mut self, pos: [i32; 3]) {
        self.pending.insert(pos);
    }
}

impl MachineSpec for AnvilSpec {
    const KIND_KEY: &'static str = keys::ANVIL_GUI;
    const BLOCK_KEY: &'static str = keys::ANVIL;
    const VARIANT_KEYS: &'static [&'static str] = &[];
    const ANCHORS_KEY: &'static str = "forge:anvils";
    const STATE_KEY: &'static str = STATE_KEY;

    fn init(&mut self) {
        self.augments = rows::augment_fits();
        (self.by_identity, self.material_of) = rows::index_by_identity(&self.augments);
        self.names = rows::display_names(&self.by_identity);
        self.tools = rows::augmentable_tools();
        self.socket_items = rows::items_with(keys::SOCKET_KEY_DATA);
        self.nondestructive = rows::items_with(keys::NONDESTRUCTIVE_DATA);
        log(&format!(
            "forge: {} augment materials, {} augmentable tools, {} socket materials",
            self.augments.len(),
            self.tools.len(),
            self.socket_items.len()
        ));
    }

    fn step(
        &mut self,
        ctx: &StepCtx<'_>,
        _caches: &mut Caches,
        slots: Option<Vec<Option<ItemStackData>>>,
        _stored: &mut Vec<u8>,
        _out: &mut Presentation,
    ) {
        let mut slots = slots.unwrap_or_default();
        slots.resize(SLOTS, None);
        let before = slots.clone();

        let mut gem_worked = false;
        while let Some((carved, cell)) = self.carve(&slots) {
            slots[SLOT_TOOL] = Some(carved);
            workstation::take(&mut slots[cell], 1);
            gem_worked = true;
        }

        gem_worked |= self.tend_sockets(&mut slots);

        if gem_worked {
            emit_sound(SOUND_SOCKET, Some(at(ctx.pos)));
        }

        if self.pending.remove(&ctx.pos) {
            if let Some((fitted, consumes)) = self.apply_staged(&slots) {
                slots[SLOT_TOOL] = Some(fitted);
                for (cell, cost) in consumes {
                    workstation::take(&mut slots[cell], cost);
                }
                emit_sound(SOUND_AUGMENT, Some(at(ctx.pos)));
            }
        }

        if slots[SLOT_TOOL].is_none() {
            self.return_cells(ctx, &mut slots, 1..SLOTS);
        } else {
            let eject = self.cells_to_eject(&slots);
            self.return_cells(ctx, &mut slots, eject);
        }
        let leaver = self.watched.get(&ctx.pos).and_then(|w| w.last().copied());
        if ctx.viewers.is_empty() {
            if let Some(player) = leaver {
                self.deliver_cells(ctx, &mut slots, 0..SLOTS, Some(player));
            } else if slots.iter().any(|s| s.is_some()) {
                self.deliver_cells(ctx, &mut slots, 0..SLOTS, None);
            }
            self.watched.remove(&ctx.pos);
        } else {
            self.watched.insert(ctx.pos, ctx.viewers.to_vec());
        }

        write_changed_slots(ctx.pos, &before, &slots);

        if ctx.gui_open() {
            self.publish_stage(ctx, &slots);
        }
    }

    fn forget(&mut self, pos: [i32; 3]) {
        self.pending.remove(&pos);
        self.watched.remove(&pos);
    }
}

fn at(pos: [i32; 3]) -> [f64; 3] {
    [
        pos[0] as f64 + 0.5,
        pos[1] as f64 + 0.5,
        pos[2] as f64 + 0.5,
    ]
}

impl AnvilSpec {
    fn tool_in<'a>(
        &'a self,
        slots: &'a [Option<ItemStackData>],
    ) -> Option<(&'a ItemStackData, &'a ToolSlots, &'a ToolStats, Record)> {
        let tool = slots[SLOT_TOOL].as_ref().filter(|s| s.count > 0)?;
        let (tool_slots, stats) = self.tools.get(&tool.item)?;
        let rec = Record::of_stack(&tool.data)?;
        Some((tool, tool_slots, stats, rec))
    }

    fn capacity(tool_slots: &ToolSlots, rec: &Record) -> usize {
        (BASE_SOCKETS + rec.carved.min(tool_slots.lockable)) as usize
    }

    fn cell_state<'a>(tool_slots: &ToolSlots, rec: &'a Record, socket: usize) -> CellState<'a> {
        if let Some(id) = rec.id_at(socket) {
            return CellState::Occupied(id);
        }
        if socket < Self::capacity(tool_slots, rec) {
            CellState::Open
        } else if socket < (BASE_SOCKETS + tool_slots.lockable) as usize {
            CellState::Locked
        } else {
            CellState::Absent
        }
    }

    fn fit_of(&self, identity: &str, kind: &str) -> Option<&Fit> {
        self.by_identity
            .get(identity)?
            .iter()
            .find(|f| f.tool == kind)
    }
}

#[cfg(test)]
mod tests;
