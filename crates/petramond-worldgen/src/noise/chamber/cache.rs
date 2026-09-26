use super::{Excavation, Room, UndergroundBiomes};
use crate::cache::{inline, CacheBudget, Memo, MemoSpec, MemoStats, Scaling};

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct Key {
    seed: u32,
    table: usize,
    excavation: usize,
    cell: [i32; 2],
}

impl Key {
    pub(super) fn new(
        table: &UndergroundBiomes,
        excavation: &Excavation,
        seed: u32,
        cell: [i32; 2],
    ) -> Self {
        Self {
            seed,
            table: table as *const _ as usize,
            excavation: excavation as *const _ as usize,
            cell,
        }
    }
}

/// Memoizes position-only admission, including rejected candidates. Query-box
/// culling happens afterward; it must never enter a cached result. Admission
/// is a pure function of the key and the natural geometry source, so every
/// generator of a world shares its world's instance (`crate::cache`); a test
/// that substitutes its own geometry oracle builds its own.
pub(in crate::noise) struct CandidateCache(Memo<Key, Option<Room>>);

impl Default for CandidateCache {
    fn default() -> Self {
        Self::new(CacheBudget::REFERENCE)
    }
}

impl CandidateCache {
    pub(in crate::noise) fn new(budget: CacheBudget) -> Self {
        Self(Memo::new(
            MemoSpec {
                name: "cave.chamber_candidates",
                capacity: 4096,
                scaling: Scaling::Frontier,
                weigh: inline,
            },
            budget,
        ))
    }

    pub(in crate::noise) fn stats(&self) -> MemoStats {
        self.0.stats()
    }

    pub(in crate::noise) fn clear(&self) {
        self.0.clear();
    }

    pub(super) fn get_or_compute(
        &self,
        key: Key,
        compute: impl FnOnce() -> Option<Room>,
    ) -> Option<Room> {
        self.0.get_or_insert(key, compute)
    }
}
