use rustc_hash::{FxHashMap, FxHashSet};

use crate::chunk::{ChunkPos, SectionPos};

#[derive(Default)]
pub struct SavedIndex {
    authoritative: FxHashSet<SectionPos>,
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

    pub fn sections_in_column(&self, pos: ChunkPos) -> &[i32] {
        self.columns.get(&pos).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn insert_authoritative(&mut self, pos: SectionPos) {
        if self.authoritative.insert(pos) {
            self.columns
                .entry(pos.chunk_pos())
                .or_default()
                .push(pos.cy);
        }
    }

    pub fn insert_explored(&mut self, pos: SectionPos) {
        self.explored.insert(pos);
    }

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

    pub fn remove_explored(&mut self, pos: SectionPos) {
        self.explored.remove(&pos);
    }
}
