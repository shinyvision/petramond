use crate::chunk::{ChunkPos, SectionPos, SECTION_MIN_CY};

use super::World;

impl World {
    #[inline]
    pub(in crate::world) fn column_cy_bit(cy: i32) -> u32 {
        debug_assert!(Self::column_section_range().contains(&cy));
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
        *self.section_column_cys.entry(pos.chunk_pos()).or_insert(0) |=
            Self::column_cy_bit(pos.cy);
        // A bulk section load bypasses the per-edit bake trigger, so mark any
        // Layer-3 custom-shape cells for a (re)bake now (a chair restored from
        // disk must rebuild its geometry, not sit on the static fallback).
        self.scan_section_custom_bakes(pos);
    }

    #[inline]
    pub(in crate::world) fn note_section_unloaded(&mut self, pos: SectionPos) {
        let column = pos.chunk_pos();
        let Some(bits) = self.section_column_cys.get_mut(&column) else {
            return;
        };
        *bits &= !Self::column_cy_bit(pos.cy);
        if *bits == 0 {
            self.section_column_cys.remove(&column);
        }
    }

    #[inline]
    pub(in crate::world) fn clear_section_column_index(&mut self, pos: ChunkPos) {
        self.section_column_cys.remove(&pos);
    }

    /// Track a newly pending section gen/disk-primary request.
    #[inline]
    pub(in crate::world) fn insert_pending_section(&mut self, sp: SectionPos) -> bool {
        if self.pending_sections.insert(sp) {
            *self.pending_section_columns.entry(sp.chunk_pos()).or_insert(0) += 1;
            true
        } else {
            false
        }
    }

    /// Clear a pending section; returns whether it was pending.
    #[inline]
    pub(in crate::world) fn remove_pending_section(&mut self, sp: SectionPos) -> bool {
        if !self.pending_sections.remove(&sp) {
            return false;
        }
        let column = sp.chunk_pos();
        let Some(count) = self.pending_section_columns.get_mut(&column) else {
            return true;
        };
        *count = count.saturating_sub(1);
        if *count == 0 {
            self.pending_section_columns.remove(&column);
        }
        true
    }

    #[inline]
    pub(in crate::world) fn column_has_pending_section(&self, pos: ChunkPos) -> bool {
        self.pending_section_columns.contains_key(&pos)
    }

    #[inline]
    pub(in crate::world) fn clear_pending_sections_for_column(&mut self, pos: ChunkPos) {
        self.pending_sections.retain(|sp| sp.chunk_pos() != pos);
        self.pending_section_columns.remove(&pos);
    }

    #[inline]
    pub(in crate::world) fn clear_all_pending_sections(&mut self) {
        self.pending_sections.clear();
        self.pending_section_columns.clear();
    }
}
