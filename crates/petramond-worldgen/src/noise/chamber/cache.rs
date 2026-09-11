use super::{Excavation, Room, UndergroundBiomes};
use crate::memo::SharedMemo;
use std::sync::LazyLock;

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
/// generator in the process shares one instance ([`shared`](Self::shared));
/// a test that substitutes its own geometry oracle builds its own.
pub(in crate::noise) struct CandidateCache(SharedMemo<Key, Option<Room>>);

impl Default for CandidateCache {
    fn default() -> Self {
        Self(SharedMemo::new(4096))
    }
}

impl CandidateCache {
    pub(in crate::noise) fn shared() -> &'static CandidateCache {
        static SHARED: LazyLock<CandidateCache> = LazyLock::new(CandidateCache::default);
        &SHARED
    }

    pub(super) fn get_or_compute(
        &self,
        key: Key,
        compute: impl FnOnce() -> Option<Room>,
    ) -> Option<Room> {
        self.0.get_or_insert(key, compute)
    }
}
