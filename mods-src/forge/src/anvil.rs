//! The anvil: tool AUGMENTS — smithing, not casting.
//!
//! You put the tool in, you put the RAW MATERIAL in — diamonds, gold ingots,
//! a hushjaw's teeth — and the anvil fits the augment. There is no
//! intermediate "diamond tip" craftable anywhere: the tip exists only as the
//! augment's overlay art and its record on the tool. That is the pack's own
//! economy rule again (the mould, the head and the tool are one object at
//! three stages): the material, the mark on the sprite, and the augmented
//! tool are one object at three stages, and no craftable middle step can
//! leak out of it.
//!
//! SOCKETS. A tool has one augment socket by construction and its row may
//! declare more as LOCKABLE (`forge:augment_slots` `{"family"?, "lockable"?}`).
//! Locked sockets are carved open PER TOOL at the anvil by consuming a
//! socket material (any item carrying `forge:socket_key` — the petramond
//! gem): drop one into a socket cell and the next locked socket opens on the
//! machine's step, no button. The carved count rides the tool's own record,
//! so each tool is an artifact you invested in, not a stat the player owns.
//!
//! An augmented tool is INSTANCE DATA on the ordinary tool stack, never a new
//! item row: the anvil stamps three keys and the engine does the rest.
//!
//! - `forge:augments` — this pack's own record of carved sockets and
//!   installed augments. The format, and everything that reads or writes it,
//!   belongs to [`crate::augments`]; the machine only stamps it.
//! - `petramond:tool` — the ENGINE's per-stack tool override, stated as
//!   ABSOLUTES: the harvest gate goes to the material at the cutting edge
//!   (max tier across base and every augment), speed and damage are the base
//!   tool's resolved values times every augment's multipliers. Every apply
//!   RECOMPUTES the whole override from the base row plus the full record —
//!   never incrementally — so results are independent of assembly order and
//!   a rebalanced ladder re-stamps correctly on the next anvil visit.
//! - `petramond:overlay` — the overlay ART item names, composited in order
//!   over the tool's sprite at every render site; art is authored IN
//!   POSITION on its own transparent 16×16 and no coordinate crosses any
//!   boundary. Recomputed with the rest, so renamed or re-familied art heals
//!   on the next apply.
//!
//! WHICH TOOLS TAKE WHICH AUGMENTS IS DATA, twice over. A tool item carries
//! `forge:augment_slots`; an augment MATERIAL carries `forge:augment` — a
//! LIST of fits (tool kind, edge tier, multipliers, material cost, overlay
//! item, optional `gentle` behaviour grant), one per tool kind it augments.
//! Another pack extends any side with rows alone — its own socket material
//! included. Both tables are read in [`rows`].
//!
//! APPLICATION IS STAGED, not automatic: valid materials sitting in open
//! socket cells publish a result PREVIEW, and only the panel's Augment
//! button — a widget click recorded by [`AnvilSpec::request_apply`] and
//! honoured on the next step — consumes them and transforms the tool.
//! Socket CARVING is the deliberate exception (it commits on the drop):
//! the gesture Rachel specified is "put a petramond on a locked slot", and
//! a gem whose only use is carving needs no second confirmation.

mod gestures;
mod panel;
mod rows;
mod workstation;

use std::collections::{HashMap, HashSet};

use mod_sdk::*;

use machine_core::{write_changed_slots, Caches, Machine, MachineSpec, Presentation, StepCtx};

use crate::augments::{Record, NONDESTRUCTIVE_KEY, SOCKET_ITEM_KEY};
use rows::{Fit, ToolSlots, ToolStats};

pub(crate) use rows::WearOn;

const STATE_KEY: &str = "forge:anvil_state";

const SLOT_TOOL: usize = 0;
/// Socket cells: container indices `1..=SOCKETS`, socket index = cell − 1.
const SOCKETS: usize = 4;
const SLOTS: usize = 1 + SOCKETS;
/// Every augmentable tool has this many sockets before any carving.
const BASE_SOCKETS: u8 = 1;

/// The engine's per-value instance-data cap, mirrored here because the ABI
/// does not expose it: an over-cap value degrades the whole stack to plain
/// data, silently shedding every augment, so the anvil REFUSES an apply
/// whose record would cross it (the button just stays disabled — reachable
/// only far past the shipped augment set).
const VALUE_CAP: usize = 128;

/// The panel's Augment button (`pack/ui/documents/anvil.gui.json`).
pub const WIDGET_AUGMENT: &str = "augment";

/// Seeded RNG stream for augment wear rolls.
const WEAR_STREAM: &str = "anvil_wear";

/// Played when an Augment click actually installs something — after the
/// gate, like the furnace lever's, so a click that fits nothing stays
/// silent.
const SOUND_AUGMENT: &str = "forge:augment_fit";

/// The PETRAMOND gesture: a socket carved open or a mount level raised.
/// One row for both because they are one player action — a gem dropped on
/// a socket cell — whose outcome depends only on whether that cell was
/// occupied. Repair (the augment's own material) is not this sound.
const SOUND_SOCKET: &str = "forge:augment_unlock";

/// Socket-cell accepts masks (`bind.accepts` on the panel's socket slots):
/// bits over the cells' AUTHORED filter list, which is
/// `[forge:augment, forge:socket_key]` in document order. The mask is what
/// makes a cell's state an ADMISSION rule on both mirrors — an occupied or
/// absent socket refuses inserts as if the slot did not exist, a locked one
/// takes only the carving gem — instead of items landing and sitting inert.
/// The document's filter ORDER is what these bits mean, and nothing at
/// runtime can check it: `tests::the_panels_socket_cells_author_the_filters_these_bits_index`
/// is the guard.
const ACC_NONE: i32 = 0;
const ACC_AUGMENT: i32 = 1 << 0;
const ACC_SOCKET: i32 = 1 << 1;

/// What a socket cell would do right now: the ghost of the decision table
/// the panel paints and `step` acts on.
enum CellState<'a> {
    /// Occupied by an installed augment (its identity, for the ghost icon).
    Occupied(&'a str),
    /// Open; a valid material may be staged here.
    Open,
    /// Carvable but not yet carved — the lock glyph, and the drop target
    /// for a socket material.
    Locked,
    /// This tool has no such socket (or no tool is in).
    Absent,
}

pub type Anvil = Machine<AnvilSpec>;

#[derive(Default)]
pub struct AnvilSpec {
    /// augment material item name -> its fits.
    augments: HashMap<String, Vec<Fit>>,
    /// installed-augment IDENTITY -> its fits (one per tool KIND — several
    /// kinds may share one identity when the art is kind-agnostic, like a
    /// handle inlay), for reading a tool's record back into fits.
    by_identity: HashMap<String, Vec<Fit>>,
    /// identity -> the MATERIAL item it is applied from (the panel's grayed
    /// socket icon shows the material that went in).
    material_of: HashMap<String, String>,
    /// identity -> its display name (the socket tooltip's first line;
    /// resolved from the art item's row at init).
    names: HashMap<String, String>,
    /// augmentable tool item name -> (its socket row + family, resolved stats).
    tools: HashMap<String, (ToolSlots, ToolStats)>,
    /// Items that carve a locked socket open (`forge:socket_key` rows).
    socket_items: HashSet<String>,
    /// Tools whose row grants gentle mining innately (`forge:nondestructive`)
    /// — a `gentle` fit on one of these is no fit.
    nondestructive: HashSet<String>,
    /// Anchors whose panel's Augment button was clicked, honoured (and
    /// cleared) on that machine's next step.
    pending: HashSet<[i32; 3]>,
    /// Who was watching each anvil last step. The anvil is a WORKSTATION,
    /// not a container: when its last viewer leaves, everything in it goes
    /// back to that player, so the machine never keeps anything while
    /// closed. In-memory like `pending` — the transition it detects only
    /// matters within a live session, and stranded contents (a crash while
    /// open) drain to the world the moment the machine steps unwatched.
    watched: HashMap<[i32; 3], Vec<PlayerId>>,
}

impl AnvilSpec {
    /// The panel's Augment button was clicked at `pos`; apply on the next
    /// step, where the driver hands this machine its slots.
    pub fn request_apply(&mut self, pos: [i32; 3]) {
        self.pending.insert(pos);
    }
}

impl MachineSpec for AnvilSpec {
    const KIND_KEY: &'static str = "forge:anvil";
    const BLOCK_KEY: &'static str = "forge:anvil";
    const VARIANT_KEYS: &'static [&'static str] = &[];
    const ANCHORS_KEY: &'static str = "forge:anvils";
    const STATE_KEY: &'static str = STATE_KEY;

    fn init(&mut self) {
        self.augments = rows::augment_fits();
        (self.by_identity, self.material_of) = rows::index_by_identity(&self.augments);
        self.names = rows::display_names(&self.by_identity);
        self.tools = rows::augmentable_tools();
        self.socket_items = rows::items_with(SOCKET_ITEM_KEY);
        self.nondestructive = rows::items_with(NONDESTRUCTIVE_KEY);
        // The member counts are the only cheap signal the pack's data keys and
        // this module's constants are the same strings.
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

        // Carving commits on the drop: socket material sitting in the cells
        // of a tool with locked sockets left opens them (as many as the
        // material covers, in one step — a dropped stack must not drain
        // visibly over several ticks).
        let mut gem_worked = false;
        while let Some((carved, cell)) = self.carve(&slots) {
            slots[SLOT_TOOL] = Some(carved);
            workstation::take(&mut slots[cell], 1);
            gem_worked = true;
        }

        // Repair and mount upgrades commit on the drop too: the identity's
        // own material on its occupied socket restores condition at the
        // install rate, a socket material there raises the mount level.
        // Leftovers past full/Legendary are swept home below.
        gem_worked |= self.tend_sockets(&mut slots);

        // ONE emit per step, however many sockets the gems touched: a stack
        // carves its whole worth in a single step by design, and a step that
        // both carves and upgrades is still one gesture at one anvil. The
        // sounds live HERE and never inside the gestures, which unit tests
        // drive directly (off-wasm a host call is a panic).
        if gem_worked {
            emit_sound(SOUND_SOCKET, Some(at(ctx.pos)));
        }

        // Apply only on a recorded Augment click, and only for staged
        // materials that are valid AND affordable: they are consumed and the
        // tool transforms in place. A click with nothing valid staged clears
        // the request and changes nothing.
        if self.pending.remove(&ctx.pos) {
            if let Some((fitted, consumes)) = self.apply_staged(&slots) {
                slots[SLOT_TOOL] = Some(fitted);
                for (cell, cost) in consumes {
                    workstation::take(&mut slots[cell], cost);
                }
                emit_sound(SOUND_AUGMENT, Some(at(ctx.pos)));
            }
        }

        // THE ANVIL IS A WORKSTATION, NOT A CONTAINER. Nothing may rest in
        // it: pulling the tool out sends the staged materials after it, and
        // the last viewer closing the panel takes everything home.
        if slots[SLOT_TOOL].is_none() {
            self.return_cells(ctx, &mut slots, 1..SLOTS);
        } else {
            // The same rule when the tool is SWAPPED rather than pulled: a
            // stack resting in a cell the tool now in the slot does not
            // offer as an open socket — locked, absent, occupied, or the
            // whole tool unreadable — goes home exactly like a tool pull.
            // This also returns the unconsumed remainder an apply leaves in
            // a freshly occupied cell.
            let eject = self.cells_to_eject(&slots);
            self.return_cells(ctx, &mut slots, eject);
        }
        let leaver = self.watched.get(&ctx.pos).and_then(|w| w.last().copied());
        if ctx.viewers.is_empty() {
            if let Some(player) = leaver {
                self.deliver_cells(ctx, &mut slots, 0..SLOTS, Some(player));
            } else if slots.iter().any(|s| s.is_some()) {
                // Stranded contents (a crash while open): drain to the world.
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

/// The anvil's centre as a sound position — block precision is plenty
/// against the attenuation distances in `sounds.json`.
fn at(pos: [i32; 3]) -> [f32; 3] {
    [
        pos[0] as f32 + 0.5,
        pos[1] as f32 + 0.5,
        pos[2] as f32 + 0.5,
    ]
}

/// The questions every half of the machine asks of the slots: what tool is
/// in, how many of its sockets are open, what one cell would do, and which
/// fit an installed identity resolves to.
impl AnvilSpec {
    /// The tool in the tool slot, with its row + resolved stats + record —
    /// `None` when absent, unknown to this build, or carrying a record this
    /// build cannot reason about (refusing is the only answer that cannot
    /// corrupt a stack from a richer pack set).
    fn tool_in<'a>(
        &'a self,
        slots: &'a [Option<ItemStackData>],
    ) -> Option<(&'a ItemStackData, &'a ToolSlots, &'a ToolStats, Record)> {
        let tool = slots[SLOT_TOOL].as_ref().filter(|s| s.count > 0)?;
        let (tool_slots, stats) = self.tools.get(&tool.item)?;
        let rec = Record::of_stack(&tool.data)?;
        Some((tool, tool_slots, stats, rec))
    }

    /// How many socket cells are OPEN for this tool: the base socket plus
    /// every carved one (a rebalance that lowered `lockable` caps what
    /// carving already paid for — installed augments beyond it still count,
    /// they are just history).
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

    /// The fit `identity` resolves to for a tool of `kind`.
    fn fit_of(&self, identity: &str, kind: &str) -> Option<&Fit> {
        self.by_identity
            .get(identity)?
            .iter()
            .find(|f| f.tool == kind)
    }
}

#[cfg(test)]
mod tests;
