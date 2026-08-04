//! The in-memory index of which sections have on-disk save records.
//!
//! This is WORLD DATA, not save machinery: the sim consults it to answer
//! "may this absent section be trusted / materialized?" (see
//! `WorldData::saved_section_contains` and `WorldData::section_summary`),
//! so it lives on [`WorldData`](super::data::WorldData) rather than behind
//! the save handle. The save I/O layer (`crate::save`) BUILDS it at open
//! (region-header scan) and mutates it at the write/delete choke points it
//! already owns; nothing else writes it.

use rustc_hash::{FxHashMap, FxHashSet};

use crate::chunk::{ChunkPos, SectionPos};

/// Which sections have an on-disk record, split by store: AUTHORITATIVE
/// records are player-modified content (they shadow worldgen forever);
/// EXPLORED records are the pure-gen cache ("Optimize explored terrain") a
/// regen could always rebuild.
#[derive(Default)]
pub struct SavedIndex {
    authoritative: FxHashSet<SectionPos>,
    /// Authoritative sections per XZ column (`cy` list), so column planners
    /// iterate real records instead of probing the vertical range.
    columns: FxHashMap<ChunkPos, Vec<i32>>,
    explored: FxHashSet<SectionPos>,
}

impl SavedIndex {
    pub fn from_scan(
        authoritative: FxHashSet<SectionPos>,
        explored: FxHashSet<SectionPos>,
    ) -> SavedIndex {
        let mut columns: FxHashMap<ChunkPos, Vec<i32>> = FxHashMap::default();
        for pos in &authoritative {
            columns.entry(pos.chunk_pos()).or_default().push(pos.cy);
        }
        SavedIndex {
            authoritative,
            columns,
            explored,
        }
    }

    /// A record exists in EITHER store.
    #[inline]
    pub fn contains(&self, pos: SectionPos) -> bool {
        self.authoritative.contains(&pos) || self.explored.contains(&pos)
    }

    #[inline]
    pub fn authoritative_contains(&self, pos: SectionPos) -> bool {
        self.authoritative.contains(&pos)
    }

    #[inline]
    pub fn explored_contains(&self, pos: SectionPos) -> bool {
        self.explored.contains(&pos)
    }

    /// The authoritative record `cy`s of column `pos` (empty slice if none).
    pub fn sections_in_column(&self, pos: ChunkPos) -> &[i32] {
        self.columns.get(&pos).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Record `pos` as having an authoritative on-disk record.
    pub fn insert_authoritative(&mut self, pos: SectionPos) {
        if self.authoritative.insert(pos) {
            self.columns.entry(pos.chunk_pos()).or_default().push(pos.cy);
        }
    }

    /// Record `pos` as having an explored-cache record.
    pub fn insert_explored(&mut self, pos: SectionPos) {
        self.explored.insert(pos);
    }

    /// Drop `pos`'s authoritative record (missing/corrupt on read-back).
    pub fn remove_authoritative(&mut self, pos: SectionPos) {
        if self.authoritative.remove(&pos) {
            if let Some(cys) = self.columns.get_mut(&pos.chunk_pos()) {
                cys.retain(|&cy| cy != pos.cy);
                if cys.is_empty() {
                    self.columns.remove(&pos.chunk_pos());
                }
            }
        }
    }

    /// Drop `pos`'s explored-cache record (missing/corrupt on read-back).
    pub fn remove_explored(&mut self, pos: SectionPos) {
        self.explored.remove(&pos);
    }
}
