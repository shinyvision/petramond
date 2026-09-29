use crate::world::WorldData;
use crate::world::{ServerWorld, World, WorldSide};
use petramond_world::chunk::{ChunkPos, SectionPos, SECTION_MIN_CY};

#[inline]
pub(crate) fn column_cy_bit(cy: i32) -> u32 {
    debug_assert!(WorldData::column_section_range().contains(&cy));
    1u32 << (cy - SECTION_MIN_CY) as u32
}

#[inline]
pub(crate) fn for_each_column_cy(bits: u32, mut f: impl FnMut(i32)) {
    let mut b = bits;
    while b != 0 {
        let i = b.trailing_zeros() as i32;
        f(SECTION_MIN_CY + i);
        b &= b - 1;
    }
}

impl<S: WorldSide> World<S> {
    pub(in crate::world) fn note_section_loaded(&mut self, pos: SectionPos) {
        *self
            .data
            .section_column_cys
            .entry(pos.chunk_pos())
            .or_insert(0) |= column_cy_bit(pos.cy);
        self.data.random_tick_dirty.insert(pos);
        self.note_send_event(pos);
        self.scan_loaded_section(pos);
    }

    /// A section's sendability to connections may have changed: every server send plan
    /// re-evaluates it on the next streaming pump.
    #[inline]
    pub(in crate::world) fn note_send_event(&mut self, pos: SectionPos) {
        if let Some(server) = self.side.server_mut() {
            server.replication.send_events.push(pos);
        }
    }

    #[inline]
    pub(in crate::world) fn note_section_unloaded(&mut self, pos: SectionPos) {
        self.note_send_event(pos);
        let column = pos.chunk_pos();
        let Some(bits) = self.data.section_column_cys.get_mut(&column) else {
            return;
        };
        *bits &= !column_cy_bit(pos.cy);
        if *bits == 0 {
            self.data.section_column_cys.remove(&column);
        }
        self.data.random_tick_dirty.remove(&pos);
        self.clear_random_tick_bit(pos);
    }

    #[inline]
    pub(in crate::world) fn clear_section_column_index(&mut self, pos: ChunkPos) {
        self.data.section_column_cys.remove(&pos);
        self.data.random_tick_index.columns.remove(&pos);
    }

    #[inline]
    fn clear_random_tick_bit(&mut self, pos: SectionPos) {
        self.data.random_tick_index.note(pos, None);
    }

    pub(in crate::world) fn repair_random_tick_index(&mut self) {
        if self.data.random_tick_dirty.is_empty() {
            return;
        }
        let dirty = std::mem::take(&mut self.data.random_tick_dirty);
        for pos in &dirty {
            let section = self.data.sections.get(pos).map(|s| &**s);
            self.data.random_tick_index.note(*pos, section);
            self.data.persist_candidates.insert(*pos);
        }
        let mut dirty = dirty;
        dirty.clear();
        self.data.random_tick_dirty = dirty;
    }
}

impl ServerWorld {
    #[inline]
    pub(in crate::world) fn insert_pending_section(&mut self, sp: SectionPos) -> bool {
        if self.side.gen.pending_sections.insert(sp) {
            self.note_stream_nonfinal(sp);
            *self
                .side
                .gen
                .pending_section_columns
                .entry(sp.chunk_pos())
                .or_insert(0) += 1;
            true
        } else {
            false
        }
    }

    #[inline]
    pub(in crate::world) fn remove_pending_section(&mut self, sp: SectionPos) -> bool {
        if !self.side.gen.pending_sections.remove(&sp) {
            return false;
        }
        self.settle_stream_nonfinal(sp);
        let column = sp.chunk_pos();
        let Some(count) = self.side.gen.pending_section_columns.get_mut(&column) else {
            return true;
        };
        *count = count.saturating_sub(1);
        if *count == 0 {
            self.side.gen.pending_section_columns.remove(&column);
        }
        true
    }

    #[inline]
    pub(in crate::world) fn column_has_pending_section(&self, pos: ChunkPos) -> bool {
        self.side.gen.pending_section_columns.contains_key(&pos)
    }
}
