use mod_sdk::*;

#[derive(Clone, Default)]
pub struct Fluids(Vec<u64>);

impl Fluids {
    pub fn resolve() -> Self {
        let mut fluids = Self::default();
        for (id, info) in registered_blocks() {
            if info.fluid.is_some() {
                fluids.insert(id);
            }
        }
        fluids
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
