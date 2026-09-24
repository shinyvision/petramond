//! The packed terrain columns, in a dense slab addressed by SLOT.
//!
//! A frame visits the same column many times: the draw planner resolves it to
//! cull and walk its sections, then four encode loops resolve it again for
//! every draw they record. Keyed by [`ChunkPos`] that was a hashed probe into
//! roughly a megabyte of column records per visit, thousands of times a frame
//! — and the probe, not the work it guarded, dominated both the plan and the
//! encode.
//!
//! So the columns live in a `Vec` and the map answers only "which slot". The
//! plan resolves a column ONCE per frame, records its slot in the draw order,
//! and every later visit indexes the slab directly. A slot is stable while the
//! column set is: removal back-fills from the end and repairs the moved
//! column's index entry, and both operations bump `gpu_revision`, which is
//! what the planner keys its cached cull index (and so its recorded slots) on.

use std::collections::HashMap;

use petramond_world::chunk::ChunkPos;

use crate::resources::GpuColumnMesh;

/// A column's index in [`ColumnStore`]'s slab. Only valid against the store it
/// came from, and only until the column set changes.
pub(super) type ColumnSlot = u32;

#[derive(Default)]
pub(super) struct ColumnStore {
    slab: Vec<GpuColumnMesh>,
    /// `ChunkPos` → slab index. The slab's own order is an implementation
    /// detail — nothing may depend on it, because a removal permutes it.
    slots: HashMap<ChunkPos, ColumnSlot>,
}

impl ColumnStore {
    pub(super) fn len(&self) -> usize {
        self.slab.len()
    }

    #[cfg(test)]
    pub(super) fn is_empty(&self) -> bool {
        self.slab.is_empty()
    }

    pub(super) fn clear(&mut self) {
        self.slab.clear();
        self.slots.clear();
    }

    #[inline]
    pub(super) fn slot(&self, pos: &ChunkPos) -> Option<ColumnSlot> {
        self.slots.get(pos).copied()
    }

    #[inline]
    pub(super) fn at(&self, slot: ColumnSlot) -> &GpuColumnMesh {
        &self.slab[slot as usize]
    }

    #[inline]
    pub(super) fn at_mut(&mut self, slot: ColumnSlot) -> &mut GpuColumnMesh {
        &mut self.slab[slot as usize]
    }

    #[inline]
    pub(super) fn get(&self, pos: &ChunkPos) -> Option<&GpuColumnMesh> {
        self.slot(pos).map(|s| self.at(s))
    }

    pub(super) fn values(&self) -> impl Iterator<Item = &GpuColumnMesh> {
        self.slab.iter()
    }

    /// `(position, slot, column)` for every installed column.
    pub(super) fn iter(&self) -> impl Iterator<Item = (ChunkPos, ColumnSlot, &GpuColumnMesh)> {
        self.slots
            .iter()
            .map(|(&pos, &slot)| (pos, slot, &self.slab[slot as usize]))
    }

    /// Install or replace `pos`'s column. Replacing KEEPS the existing slot, so
    /// a repack in place does not permute anything.
    pub(super) fn insert(&mut self, pos: ChunkPos, column: GpuColumnMesh) {
        match self.slots.get(&pos) {
            Some(&slot) => self.slab[slot as usize] = column,
            None => {
                let slot = self.slab.len() as ColumnSlot;
                self.slab.push(column);
                self.slots.insert(pos, slot);
            }
        }
    }

    pub(super) fn remove(&mut self, pos: &ChunkPos) -> Option<GpuColumnMesh> {
        let slot = self.slots.remove(pos)?;
        let column = self.slab.swap_remove(slot as usize);
        // `swap_remove` moved the last column into the hole (unless the hole
        // WAS the last): repoint it, or the map would name a stranger.
        if (slot as usize) < self.slab.len() {
            let moved = self.slab[slot as usize].column_pos();
            self.slots.insert(moved, slot);
        }
        Some(column)
    }

    /// Keep only the columns `keep` accepts, by position.
    pub(super) fn retain(&mut self, keep: impl Fn(ChunkPos) -> bool) {
        let doomed: Vec<ChunkPos> = self
            .slots
            .keys()
            .copied()
            .filter(|&pos| !keep(pos))
            .collect();
        for pos in doomed {
            self.remove(&pos);
        }
    }
}
