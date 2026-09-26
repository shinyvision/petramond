//! [`GenContext`]: what a memoized generation fact derives from besides its
//! position, compared by value.
//!
//! Every shared memo key starts with one. A fact is a pure function of the
//! world seed, the engine's code and the data catalogs the pipeline loaded;
//! the context names the seed and the CONTENT of those catalogs (their
//! fingerprints), never where they live in memory. Two generators over equal
//! catalogs therefore share entries, and a reloaded or different pack set can
//! never be answered from another's — even if its tables land at an address
//! a freed table once had.
//!
//! Sub-catalog identities inside a key follow the same rule: an excavation
//! row is named by its salt (the hash of its unique name), a projection rule
//! by its index within that row, each covered by the catalog fingerprint.

use crate::data::excavations::{self, Excavations};
use crate::data::underground::{self, UndergroundBiomes};

/// The seed and catalog fingerprints a generation fact derives from. See the
/// module docs.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct GenContext {
    seed: u32,
    habitats: u64,
    /// `None` for a fact that is defined not to read excavations at all (the
    /// natural cave source sampled without their influence), so it can never
    /// be confused with the same fact under any excavation catalog.
    excavations: Option<u64>,
}

impl GenContext {
    /// The context of a field over the habitat and excavation catalogs.
    pub(crate) fn new(
        seed: u32,
        underground: &UndergroundBiomes,
        excavations: &Excavations,
    ) -> Self {
        Self {
            seed,
            habitats: underground.fingerprint,
            excavations: Some(excavations.fingerprint),
        }
    }

    /// The context of the process's loaded catalogs.
    pub fn installed(seed: u32) -> Self {
        Self::new(seed, underground::table(), excavations::table())
    }

    pub fn seed(self) -> u32 {
        self.seed
    }

    /// One fingerprint over every catalog the context names — what a
    /// persisted cache stamps beside the seed.
    pub fn tables(self) -> u64 {
        self.habitats.rotate_left(17) ^ self.excavations.unwrap_or(0)
    }

    /// This context for a fact that reads no excavation.
    pub(crate) fn without_excavations(self) -> Self {
        Self {
            excavations: None,
            ..self
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Equal catalogs loaded twice — at different addresses — are one
    /// context; a different seed or catalog content is another.
    #[test]
    fn contexts_compare_catalog_content_not_addresses() {
        let a = underground::test_table(&[]);
        let b = underground::test_table(&[]);
        assert!(!std::ptr::eq(a, b));
        let rows_a = excavations::test_table(&[], a);
        let rows_b = excavations::test_table(&[], b);
        assert_eq!(GenContext::new(7, a, rows_a), GenContext::new(7, b, rows_b));
        assert_ne!(GenContext::new(7, a, rows_a), GenContext::new(8, a, rows_a));

        let room = excavations::test_table(
            &[r#"{"excavations":[
             {"excavation":"test:room","placement":{"spacing":96,"y":[-48,32]},
              "chamber":{"radius":[8,12],"feather":9,"lobes":2,"lobe_spread":0.4,
                         "lobe_scale":[0.6,0.8],"tunnel":1}}
            ]}"#],
            a,
        );
        let plain = GenContext::new(7, a, rows_a);
        let rooms = GenContext::new(7, a, room);
        assert_ne!(plain, rooms);
        assert_ne!(plain.tables(), rooms.tables());
        assert_eq!(plain.without_excavations(), rooms.without_excavations());
        assert_ne!(plain.without_excavations(), plain);
    }
}
