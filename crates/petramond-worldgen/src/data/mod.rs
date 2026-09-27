pub mod biome_gen;
pub mod bounds;
pub mod climate_table;
pub mod excavations;
pub mod features;
pub mod ores;
pub mod terrain;
pub mod tree_profiles;
pub mod underground;

use petramond_world::content::Stage;

pub fn content_stages() -> [&'static dyn Stage; 8] {
    [
        &terrain::RECIPE,
        &features::CATALOG,
        &underground::TABLE,
        &excavations::TABLE,
        &ores::TABLE,
        &climate_table::TABLE,
        &biome_gen::SPECS,
        &tree_profiles::TABLE,
    ]
}

pub(crate) struct Fingerprint(u64);

impl Fingerprint {
    pub(crate) fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }

    pub(crate) fn eat(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = (self.0 ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }

    pub(crate) fn finish(self) -> u64 {
        self.0
    }
}
