//! Bounded material reads for features that cross section boundaries.

use crate::{terrain_section_at, BlockId};
use std::collections::{HashMap, VecDeque};

pub struct TerrainCache {
    capacity: usize,
    /// Tile slot per section, the tiles themselves, and the free slots left
    /// by evictions; a planner reads thousands of cells from the same few
    /// tiles, so the slot of the last tile read answers most reads without a
    /// map lookup.
    slots: HashMap<[i32; 3], usize>,
    tiles: Vec<Option<Box<[BlockId]>>>,
    /// Each resident tile's distinct ids, sorted: what a feature asks before
    /// walking a tile for blocks it may not hold at all.
    ids: Vec<Vec<BlockId>>,
    free: Vec<usize>,
    order: VecDeque<[i32; 3]>,
    last: Option<([i32; 3], usize)>,
}

impl TerrainCache {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            slots: HashMap::new(),
            tiles: Vec::new(),
            ids: Vec::new(),
            free: Vec::new(),
            order: VecDeque::new(),
            last: None,
        }
    }

    /// Positional values belong to one world-generation input set.
    pub fn clear(&mut self) {
        self.slots.clear();
        self.tiles.clear();
        self.ids.clear();
        self.free.clear();
        self.order.clear();
        self.last = None;
    }

    pub fn block(&mut self, pos: [i32; 3]) -> Option<BlockId> {
        self.block_with(pos, terrain_section_at)
    }

    /// Whether the section holding `pos` contains any of `ids`, fetching the
    /// section if it is not resident.
    pub fn section_has_any(&mut self, pos: [i32; 3], ids: &[BlockId]) -> Option<bool> {
        self.section_has_any_with(pos, ids, terrain_section_at)
    }

    fn section_has_any_with(
        &mut self,
        pos: [i32; 3],
        ids: &[BlockId],
        query: impl FnOnce([i32; 3]) -> Vec<BlockId>,
    ) -> Option<bool> {
        let cell = pos.map(|v| v.div_euclid(16));
        let slot = match self.slots.get(&cell) {
            Some(&slot) => slot,
            None => self.fetch(cell, query)?,
        };
        let present = &self.ids[slot];
        Some(
            ids.iter()
                .any(|id| present.binary_search_by_key(&id.0, |b| b.0).is_ok()),
        )
    }

    /// `query` answers one section's 4,096 cells in section order.
    fn block_with(
        &mut self,
        pos: [i32; 3],
        query: impl FnOnce([i32; 3]) -> Vec<BlockId>,
    ) -> Option<BlockId> {
        let cell = pos.map(|v| v.div_euclid(16));
        let slot = match self.last {
            Some((last, slot)) if last == cell => slot,
            _ => {
                let slot = match self.slots.get(&cell) {
                    Some(&slot) => slot,
                    None => self.fetch(cell, query)?,
                };
                self.last = Some((cell, slot));
                slot
            }
        };
        let [x, y, z] = pos.map(|v| v.rem_euclid(16) as usize);
        Some(self.tiles[slot].as_ref().expect("resident tile")[(y * 16 + z) * 16 + x])
    }

    fn fetch(
        &mut self,
        cell: [i32; 3],
        query: impl FnOnce([i32; 3]) -> Vec<BlockId>,
    ) -> Option<usize> {
        let blocks = query(cell);
        if blocks.len() != 4096 {
            return None;
        }
        if self.slots.len() == self.capacity {
            let oldest = self.order.pop_front().expect("full terrain cache");
            if let Some(slot) = self.slots.remove(&oldest) {
                self.tiles[slot] = None;
                self.free.push(slot);
            }
            if self.last.is_some_and(|(last, _)| last == oldest) {
                self.last = None;
            }
        }
        let mut ids = blocks.clone();
        ids.sort_unstable_by_key(|b| b.0);
        ids.dedup();
        let slot = match self.free.pop() {
            Some(slot) => {
                self.tiles[slot] = Some(blocks.into_boxed_slice());
                self.ids[slot] = ids;
                slot
            }
            None => {
                self.tiles.push(Some(blocks.into_boxed_slice()));
                self.ids.push(ids);
                self.tiles.len() - 1
            }
        };
        self.order.push_back(cell);
        self.slots.insert(cell, slot);
        Some(slot)
    }
}

#[cfg(test)]
mod tests;
