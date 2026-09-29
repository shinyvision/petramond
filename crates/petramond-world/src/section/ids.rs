use super::BlockCube;

const LOW_IDS: usize = 512;
const WORDS: usize = LOW_IDS / 64;

/// A superset of the block ids a section may hold: one bit per low id, one flag for anything
/// above. Every block write adds its id; only a full recount ([`super::Section::recompute_opaque_count`])
/// narrows it again, and a raw buffer borrow widens it to [`IdSet::ANY`] — so "not in the set"
/// is always a true negative and a per-section scan for a class of blocks (refining shapes,
/// custom bakes) can be skipped in O(1) for the great majority of sections.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct IdSet {
    bits: [u64; WORDS],
    high: bool,
}

impl IdSet {
    pub const EMPTY: Self = Self {
        bits: [0; WORDS],
        high: false,
    };

    pub const ANY: Self = Self {
        bits: [u64::MAX; WORDS],
        high: true,
    };

    #[inline]
    pub fn insert(&mut self, id: u16) {
        if (id as usize) < LOW_IDS {
            self.bits[(id >> 6) as usize] |= 1u64 << (id & 63);
        } else {
            self.high = true;
        }
    }

    pub fn from_ids(ids: impl IntoIterator<Item = u16>) -> Self {
        let mut set = Self::EMPTY;
        for id in ids {
            set.insert(id);
        }
        set
    }

    pub fn of_cube(cube: &BlockCube) -> Self {
        let mut set = Self::EMPTY;
        cube.for_each_id(|id| set.insert(id));
        set
    }

    /// Whether the two sets can share an id.
    #[inline]
    pub fn intersects(&self, other: &IdSet) -> bool {
        (self.high && other.high) || self.bits.iter().zip(&other.bits).any(|(a, b)| a & b != 0)
    }
}

impl Default for IdSet {
    fn default() -> Self {
        Self::EMPTY
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn low_ids_are_exact_and_high_ids_fold_into_one_flag() {
        let mut set = IdSet::EMPTY;
        set.insert(3);
        set.insert(511);
        let other = IdSet::from_ids([3]);
        assert!(set.intersects(&other));
        assert!(!set.intersects(&IdSet::from_ids([4, 510])));
        assert!(!set.intersects(&IdSet::from_ids([4000])), "no high id yet");
        set.insert(4000);
        assert!(
            set.intersects(&IdSet::from_ids([9000])),
            "any high id matches"
        );
        assert!(IdSet::ANY.intersects(&other));
        assert!(!IdSet::EMPTY.intersects(&IdSet::ANY));

        let mut cube = BlockCube::uniform(0);
        cube.set(7, 200);
        cube.set(9, 700);
        let of = IdSet::of_cube(&cube);
        assert!(of.intersects(&IdSet::from_ids([200])));
        assert!(of.intersects(&IdSet::from_ids([1000])));
        assert!(!of.intersects(&IdSet::from_ids([1])));
    }
}
