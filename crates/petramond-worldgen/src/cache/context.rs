use crate::data::excavations::{self, Excavations};
use crate::data::underground::{self, UndergroundBiomes};
use crate::data::{climate_table, terrain};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct GenContext {
    seed: u32,
    habitats: u64,
    excavations: Option<u64>,
    climate: u64,
    terrain: u64,
}

impl GenContext {
    pub(crate) fn new(
        seed: u32,
        underground: &UndergroundBiomes,
        excavations: &Excavations,
    ) -> Self {
        Self {
            seed,
            habitats: underground.fingerprint,
            excavations: Some(excavations.fingerprint),
            climate: climate_table::table().fingerprint,
            terrain: terrain::recipe().fingerprint,
        }
    }

    pub fn installed(seed: u32) -> Self {
        Self::new(seed, underground::table(), excavations::table())
    }

    pub fn seed(self) -> u32 {
        self.seed
    }

    pub fn tables(self) -> u64 {
        self.habitats.rotate_left(17)
            ^ self.excavations.unwrap_or(0)
            ^ self.climate.rotate_left(31)
            ^ self.terrain.rotate_left(47)
    }

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
