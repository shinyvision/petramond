use super::{Excavation, Room};
use crate::cache::{inline, CacheBudget, GenContext, Memo, MemoSpec, MemoStats, Scaling};

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct Key {
    context: GenContext,
    excavation: u64,
    cell: [i32; 2],
}

impl Key {
    pub(super) fn new(context: GenContext, excavation: &Excavation, cell: [i32; 2]) -> Self {
        Self {
            context,
            excavation: excavation.salt,
            cell,
        }
    }
}

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
