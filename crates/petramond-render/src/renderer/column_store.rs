use std::collections::HashMap;

use petramond_world::chunk::ChunkPos;

use crate::resources::GpuColumnMesh;

pub(super) type ColumnSlot = u32;

#[derive(Default)]
pub(super) struct ColumnStore {
    slab: Vec<GpuColumnMesh>,
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

    pub(super) fn iter(&self) -> impl Iterator<Item = (ChunkPos, ColumnSlot, &GpuColumnMesh)> {
        self.slots
            .iter()
            .map(|(&pos, &slot)| (pos, slot, &self.slab[slot as usize]))
    }

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
        if (slot as usize) < self.slab.len() {
            let moved = self.slab[slot as usize].column_pos();
            self.slots.insert(moved, slot);
        }
        Some(column)
    }

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
