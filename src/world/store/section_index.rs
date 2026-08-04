use crate::world::WorldData;
use petramond_world::chunk::{ChunkPos, SectionPos, SECTION_MIN_CY};

use super::World;

impl World {
    #[inline]
    pub(in crate::world) fn column_cy_bit(cy: i32) -> u32 {
        debug_assert!(WorldData::column_section_range().contains(&cy));
        1u32 << (cy - SECTION_MIN_CY) as u32
    }

    /// Iterate set bits of a per-column `cy` bitset.
    #[inline]
    pub(in crate::world) fn for_each_column_cy(bits: u32, mut f: impl FnMut(i32)) {
        let mut b = bits;
        while b != 0 {
            let i = b.trailing_zeros() as i32;
            f(SECTION_MIN_CY + i);
            b &= b - 1;
        }
    }

    pub(in crate::world) fn note_section_loaded(&mut self, pos: SectionPos) {
        *self
            .data
            .section_column_cys
            .entry(pos.chunk_pos())
            .or_insert(0) |= Self::column_cy_bit(pos.cy);
        self.random_tick_dirty.insert(pos);
        // A bulk section load bypasses the per-edit bake trigger, so mark any
        // custom-shape cells for a (re)bake now (a chair restored from
        // disk must rebuild its geometry, not sit on the static fallback).
        self.scan_section_custom_bakes(pos);
        // ...and re-refine the section's refining cells, so stored refined
        // state placed under an older vocabulary heals instead of rendering
        // stale forever (authoritative sides only — see the sweep's doc).
        self.refine_section_shapes(pos);
    }

    #[inline]
    pub(in crate::world) fn note_section_unloaded(&mut self, pos: SectionPos) {
        let column = pos.chunk_pos();
        let Some(bits) = self.data.section_column_cys.get_mut(&column) else {
            return;
        };
        *bits &= !Self::column_cy_bit(pos.cy);
        if *bits == 0 {
            self.data.section_column_cys.remove(&column);
        }
        self.random_tick_dirty.remove(&pos);
        self.clear_random_tick_bit(pos);
    }

    #[inline]
    pub(in crate::world) fn clear_section_column_index(&mut self, pos: ChunkPos) {
        self.data.section_column_cys.remove(&pos);
        self.data.section_column_rt.remove(&pos);
    }

    /// Clear one section's random-tickable bit, dropping the column entry when
    /// nothing tickable is left in it.
    #[inline]
    fn clear_random_tick_bit(&mut self, pos: SectionPos) {
        let column = pos.chunk_pos();
        if let Some(bits) = self.data.section_column_rt.get_mut(&column) {
            *bits &= !Self::column_cy_bit(pos.cy);
            if *bits == 0 {
                self.data.section_column_rt.remove(&column);
            }
        }
    }

    /// Re-derive the random-tickable bit of every section marked stale (see
    /// [`World::random_tick_dirty`]). The scan calls this once per tick before
    /// it walks; the cost is one map read per EDITED section, not per loaded
    /// one.
    pub(in crate::world) fn repair_random_tick_index(&mut self) {
        if self.random_tick_dirty.is_empty() {
            return;
        }
        let dirty = std::mem::take(&mut self.random_tick_dirty);
        for pos in &dirty {
            let tickable = self
                .sections
                .get(pos)
                .is_some_and(|s| s.has_random_tickable());
            if tickable {
                *self
                    .data
                    .section_column_rt
                    .entry(pos.chunk_pos())
                    .or_insert(0) |= Self::column_cy_bit(pos.cy);
            } else {
                self.clear_random_tick_bit(*pos);
            }
        }
        // Reuse the drained allocation rather than the fresh empty one.
        let mut dirty = dirty;
        dirty.clear();
        self.random_tick_dirty = dirty;
    }

    /// Track a newly pending section gen/disk-primary request.
    #[inline]
    pub(in crate::world) fn insert_pending_section(&mut self, sp: SectionPos) -> bool {
        if self.gen.pending_sections.insert(sp) {
            self.note_stream_nonfinal(sp);
            *self
                .gen
                .pending_section_columns
                .entry(sp.chunk_pos())
                .or_insert(0) += 1;
            true
        } else {
            false
        }
    }

    /// Clear a pending section; returns whether it was pending.
    #[inline]
    pub(in crate::world) fn remove_pending_section(&mut self, sp: SectionPos) -> bool {
        if !self.gen.pending_sections.remove(&sp) {
            return false;
        }
        self.settle_stream_nonfinal(sp);
        let column = sp.chunk_pos();
        let Some(count) = self.gen.pending_section_columns.get_mut(&column) else {
            return true;
        };
        *count = count.saturating_sub(1);
        if *count == 0 {
            self.gen.pending_section_columns.remove(&column);
        }
        true
    }

    #[inline]
    pub(in crate::world) fn column_has_pending_section(&self, pos: ChunkPos) -> bool {
        self.gen.pending_section_columns.contains_key(&pos)
    }

    #[inline]
    pub(in crate::world) fn clear_pending_sections_for_column(&mut self, pos: ChunkPos) {
        self.gen.pending_sections.retain(|sp| sp.chunk_pos() != pos);
        self.gen.pending_section_columns.remove(&pos);
        self.rebuild_stream_nonfinal();
    }

    #[inline]
    pub(in crate::world) fn clear_all_pending_sections(&mut self) {
        self.gen.pending_sections.clear();
        self.gen.pending_section_columns.clear();
        self.rebuild_stream_nonfinal();
    }
}
