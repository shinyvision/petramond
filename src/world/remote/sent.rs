use rustc_hash::{FxHashMap, FxHashSet};

use crate::world::store::{column_cy_bit, for_each_column_cy};
use petramond_world::chunk::{ChunkPos, SectionPos};

/// The sections one connection holds, indexed per column as a `cy` bitset so
/// the send diff works a column at a time: a column whose loaded sections
/// are all sent costs one lookup, not one per section.
#[derive(Default)]
pub struct SentSections {
    sections: FxHashSet<SectionPos>,
    by_column: FxHashMap<ChunkPos, u32>,
}

impl SentSections {
    pub fn contains(&self, sp: SectionPos) -> bool {
        self.sections.contains(&sp)
    }

    /// `true` when `sp` was not already held.
    pub fn insert(&mut self, sp: SectionPos) -> bool {
        let fresh = self.sections.insert(sp);
        if fresh {
            *self.by_column.entry(sp.chunk_pos()).or_insert(0) |= column_cy_bit(sp.cy);
        }
        fresh
    }

    /// `true` when `sp` was held.
    pub fn remove(&mut self, sp: SectionPos) -> bool {
        if !self.sections.remove(&sp) {
            return false;
        }
        let cp = sp.chunk_pos();
        if let Some(bits) = self.by_column.get_mut(&cp) {
            *bits &= !column_cy_bit(sp.cy);
            if *bits == 0 {
                self.by_column.remove(&cp);
            }
        }
        true
    }

    /// Remove and return every held section of `cp`, cy-ascending — the order
    /// the client parks unloads in.
    pub fn take_column(&mut self, cp: ChunkPos) -> Vec<SectionPos> {
        let bits = self.by_column.remove(&cp).unwrap_or(0);
        let mut out = Vec::with_capacity(bits.count_ones() as usize);
        for_each_column_cy(bits, |cy| {
            let sp = SectionPos::new(cp.cx, cy, cp.cz);
            self.sections.remove(&sp);
            out.push(sp);
        });
        out
    }

    /// The held `cy` bitset of `cp`.
    pub(in crate::world) fn column_bits(&self, cp: ChunkPos) -> u32 {
        self.by_column.get(&cp).copied().unwrap_or(0)
    }

    pub(in crate::world) fn columns(&self) -> impl Iterator<Item = (ChunkPos, u32)> + '_ {
        self.by_column.iter().map(|(&cp, &bits)| (cp, bits))
    }
}
