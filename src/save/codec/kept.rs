//! Blocks and container slots this build cannot resolve, kept in their
//! cell's KV.
//!
//! A cell whose disk block id the save palette cannot resolve (a removed
//! mod's block, or one disabled for the world) loads as AIR, and the cell's
//! KV gains [`KEPT_BLOCK_KEY`]: the disk id and the cell's state record as
//! stored. A container slot whose item cannot be resolved loads EMPTY, and
//! the container cell's KV gains [`KEPT_SLOTS_KEY`]. Encoding writes each
//! back — the block into its cell while the cell is still air, a slot into
//! its slot while that slot is still empty — and keeps both keys out of the
//! stored KV (the next load derives them again). Any block write to the cell
//! clears its KV (`Section::set_block`), so building there replaces the
//! kept block, and breaking a container drops its kept slots with it.

use std::borrow::Cow;
use std::collections::BTreeMap;

use petramond_world::block::ShapeState;
use petramond_world::section::CellMap;

use super::cell_state::{self, Stored};
use super::SectionSnapshot;
use crate::save::container::KeptSlots;
use crate::save::palette::Palette;
use crate::save::wire::{from_bytes, to_bytes, wire_struct};

/// The cell KV key of a kept block (value: [`KeptBlock`]).
pub(super) const KEPT_BLOCK_KEY: &str = "petramond:kept_block";
/// The cell KV key of a container's kept slots (value: [`KeptSlots`]).
pub(super) const KEPT_SLOTS_KEY: &str = "petramond:kept_slots";

type CellKv = CellMap<BTreeMap<String, Vec<u8>>>;

/// A block kept in disk form: its disk id and its cell-state record as
/// stored (empty when it had none).
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct KeptBlock {
    pub(super) disk: u16,
    pub(super) state: Vec<u8>,
}
wire_struct!(KeptBlock { disk, state });

/// What a snapshot's cell KV keeps, as the encoder needs it.
#[derive(Default)]
pub(super) struct KeptCells {
    /// By cell; only cells still air.
    pub(super) blocks: CellMap<KeptBlock>,
    /// By container cell; only cells still holding a container.
    pub(super) slots: CellMap<KeptSlots>,
}

impl KeptCells {
    pub(super) fn of(s: &SectionSnapshot) -> Self {
        let mut kept = Self::default();
        for (&idx, map) in &s.cell_kv {
            let block = map
                .get(KEPT_BLOCK_KEY)
                .and_then(|v| from_bytes::<KeptBlock>(v))
                .filter(|_| s.blocks.get(usize::from(idx)) == 0);
            if let Some(block) = block {
                kept.blocks.insert(idx, block);
            }
            let slots = map
                .get(KEPT_SLOTS_KEY)
                .and_then(|v| from_bytes::<KeptSlots>(v))
                .filter(|_| s.containers.contains_key(&idx));
            if let Some(slots) = slots {
                kept.slots.insert(idx, slots);
            }
        }
        kept
    }

    /// The cell-state records to write: every live state, and each kept
    /// block's stored record in its cell.
    pub(super) fn cell_states<'a>(&'a self, live: &'a CellMap<ShapeState>) -> CellMap<Stored<'a>> {
        let mut states: CellMap<Stored<'a>> = live
            .iter()
            .filter(|(idx, _)| !self.blocks.contains_key(*idx))
            .map(|(&idx, state)| (idx, Stored::Live(state)))
            .collect();
        for (&idx, block) in &self.blocks {
            if !block.state.is_empty() {
                states.insert(idx, Stored::Kept(&block.state));
            }
        }
        states
    }
}

fn is_kept_key(key: &str) -> bool {
    key == KEPT_BLOCK_KEY || key == KEPT_SLOTS_KEY
}

/// The cell KV as stored: without the kept keys (borrowed when there are
/// none, the common case).
pub(super) fn stored_kv(kv: &CellKv) -> Cow<'_, CellKv> {
    if !kv.values().any(|map| map.keys().any(|k| is_kept_key(k))) {
        return Cow::Borrowed(kv);
    }
    Cow::Owned(
        kv.iter()
            .filter_map(|(&idx, map)| {
                let map: BTreeMap<String, Vec<u8>> = map
                    .iter()
                    .filter(|(k, _)| !is_kept_key(k))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                (!map.is_empty()).then_some((idx, map))
            })
            .collect(),
    )
}

/// The decoded cells' live states and KV: `unknown_cells` (`(cell, disk
/// id)` of every unresolvable block) keep their disk id and stored state
/// record under [`KEPT_BLOCK_KEY`], `kept_slots` go under [`KEPT_SLOTS_KEY`],
/// and every other stored state translates to live.
pub(super) fn restore_cells(
    unknown_cells: Vec<(u16, u16)>,
    mut stored_states: CellMap<Vec<u8>>,
    mut cell_kv: CellKv,
    kept_slots: CellMap<KeptSlots>,
    pal: &Palette,
) -> (CellMap<ShapeState>, CellKv) {
    for (idx, disk) in unknown_cells {
        let block = KeptBlock {
            disk,
            state: stored_states.remove(&idx).unwrap_or_default(),
        };
        cell_kv
            .entry(idx)
            .or_default()
            .insert(KEPT_BLOCK_KEY.to_owned(), to_bytes(&block));
    }
    for (idx, slots) in kept_slots {
        cell_kv
            .entry(idx)
            .or_default()
            .insert(KEPT_SLOTS_KEY.to_owned(), to_bytes(&slots));
    }
    let states = stored_states
        .iter()
        .map(|(&idx, record)| (idx, cell_state::translate(record, pal)))
        .collect();
    (states, cell_kv)
}
