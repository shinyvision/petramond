use std::borrow::Cow;
use std::collections::BTreeMap;

use petramond_world::block::ShapeState;
use petramond_world::section::CellMap;

use super::cell_state::{self, Stored};
use super::SectionSnapshot;
use crate::save::container::KeptSlots;
use crate::save::palette::Palette;
use crate::save::wire::{from_bytes, to_bytes, wire_struct};

pub(super) const KEPT_BLOCK_KEY: &str = "petramond:kept_block";
pub(super) const KEPT_SLOTS_KEY: &str = "petramond:kept_slots";

type CellKv = CellMap<BTreeMap<String, Vec<u8>>>;

#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct KeptBlock {
    pub(super) disk: u16,
    pub(super) state: Vec<u8>,
}
wire_struct!(KeptBlock { disk, state });

#[derive(Default)]
pub(super) struct KeptCells {
    pub(super) blocks: CellMap<KeptBlock>,
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
