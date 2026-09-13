//! Which block ids are fluids, answered once at init by the engine's rows —
//! never by naming the fluids a pack happens to know about.

use mod_sdk::*;

/// The session's fluid block ids, as a bitset.
#[derive(Clone, Default)]
pub struct Fluids(Vec<u64>);

impl Fluids {
    /// Every registered block whose row declares a fluid. Block ids are
    /// dense, so the registry is read a page at a time up to its first
    /// unregistered id.
    pub fn resolve() -> Self {
        const PAGE: u16 = 256;
        let mut fluids = Self::default();
        let mut first = 0u16;
        loop {
            let infos = block_infos((first..first.saturating_add(PAGE)).map(BlockId).collect());
            for (id, info) in (first..).zip(infos) {
                match info {
                    None => return fluids,
                    Some(info) if info.fluid.is_some() => fluids.insert(BlockId(id)),
                    Some(_) => {}
                }
            }
            first = match first.checked_add(PAGE) {
                Some(next) => next,
                None => return fluids,
            };
        }
    }

    #[inline]
    pub fn contains(&self, block: BlockId) -> bool {
        let id = usize::from(block.0);
        self.0
            .get(id / 64)
            .is_some_and(|word| word >> (id % 64) & 1 != 0)
    }

    fn insert(&mut self, block: BlockId) {
        let id = usize::from(block.0);
        if self.0.len() <= id / 64 {
            self.0.resize(id / 64 + 1, 0);
        }
        self.0[id / 64] |= 1 << (id % 64);
    }

    #[cfg(test)]
    pub fn of(ids: &[BlockId]) -> Self {
        let mut fluids = Self::default();
        for &id in ids {
            fluids.insert(id);
        }
        fluids
    }
}
