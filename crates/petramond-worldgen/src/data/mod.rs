//! Layered worldgen content catalogs: the rows a pack layers over, parsed
//! into the tables generation reads — including the generation rules
//! (`biome_gen`) and tree placement (`tree_profiles`) of each biome row, the
//! climate → biome placement table (`climate_table`), the underground vein
//! table (`ores`) and the terrain density recipe (`terrain`).

pub mod biome_gen;
pub mod bounds;
pub mod climate_table;
pub mod excavations;
pub mod features;
pub mod ores;
pub mod terrain;
pub mod tree_profiles;
pub mod underground;

/// FNV-1a over a loaded table's resolved content: the fingerprint a table
/// stamps into the column-gen cache so a pack that changes it is never served
/// stale cached columns.
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
