use super::ItemType;

const WORDS: usize = crate::registry::WIDE_ID_CAP / 64;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ItemSet([u64; WORDS]);

impl Default for ItemSet {
    fn default() -> Self {
        ItemSet::EMPTY
    }
}

impl ItemSet {
    pub const EMPTY: ItemSet = ItemSet([0; WORDS]);

    #[inline]
    pub fn insert(&mut self, item: ItemType) -> bool {
        let id = item.id() as usize;
        let bit = 1 << (id % 64);
        let word = &mut self.0[id / 64];
        let fresh = *word & bit == 0;
        *word |= bit;
        fresh
    }

    #[inline]
    pub fn intersects(&self, other: &ItemSet) -> bool {
        self.0.iter().zip(other.0.iter()).any(|(a, b)| a & b != 0)
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.0.iter().all(|w| *w == 0)
    }

    pub fn iter(&self) -> impl Iterator<Item = ItemType> + '_ {
        self.0.iter().enumerate().flat_map(|(w, word)| {
            let mut bits = *word;
            std::iter::from_fn(move || {
                (bits != 0).then(|| {
                    let bit = bits.trailing_zeros() as usize;
                    bits &= bits - 1;
                    ItemType((w * 64 + bit) as u16)
                })
            })
        })
    }
}

impl FromIterator<ItemType> for ItemSet {
    fn from_iter<I: IntoIterator<Item = ItemType>>(iter: I) -> Self {
        let mut set = ItemSet::EMPTY;
        for item in iter {
            set.insert(item);
        }
        set
    }
}
