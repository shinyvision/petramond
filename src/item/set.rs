//! A dense set of item kinds.
//!
//! Item ids are one byte, so a whole set of them is four words — small enough
//! to live on a player, cheap enough to intersect per event. Progression uses
//! it for "every item this player has ever held" and the recipe-unlock index
//! for "any item that satisfies this ingredient", which makes an ingredient
//! test one AND (see `crafting::unlock`).
//!
//! Ids are SESSION-SCOPED (packs register past the engine's frozen range), so
//! a persisted set travels as registry names, never as these bits.

use super::ItemType;

const WORDS: usize = 4;

/// A set of [`ItemType`]s, one bit per id.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct ItemSet([u64; WORDS]);

impl ItemSet {
    pub const EMPTY: ItemSet = ItemSet([0; WORDS]);

    /// Add `item`; `true` when it was not already present.
    #[inline]
    pub fn insert(&mut self, item: ItemType) -> bool {
        let id = item.id() as usize;
        let bit = 1 << (id % 64);
        let word = &mut self.0[id / 64];
        let fresh = *word & bit == 0;
        *word |= bit;
        fresh
    }

    /// Whether the two sets share any member — the ingredient test.
    #[inline]
    pub fn intersects(&self, other: &ItemSet) -> bool {
        self.0.iter().zip(other.0.iter()).any(|(a, b)| a & b != 0)
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.0.iter().all(|w| *w == 0)
    }

    /// Members in ascending id order.
    pub fn iter(&self) -> impl Iterator<Item = ItemType> + '_ {
        self.0.iter().enumerate().flat_map(|(w, word)| {
            let mut bits = *word;
            std::iter::from_fn(move || {
                (bits != 0).then(|| {
                    let bit = bits.trailing_zeros() as usize;
                    bits &= bits - 1;
                    ItemType((w * 64 + bit) as u8)
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
