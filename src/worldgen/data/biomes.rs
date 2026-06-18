//! Per-biome definitions.
//!
//! Strata P2: each `BiomeDef` carries the column's surface composition (top
//! rule + subsurface block). The top rules + subsurface blocks here reproduce
//! the original `surface_block`/`subsurface_block` EXACTLY (byte-parity); the
//! richer, layered surface stacks land in P4. P3 adds the `features` slice and
//! P4 adds tints + climate-placement points, so a biome ends up fully defined
//! in this one table.

use crate::biome::Biome;
use crate::block::Block;
use crate::worldgen::surface::rule::{SurfaceCond, SurfaceRule};

pub struct BiomeDef {
    pub biome: Biome,
    /// Surface (top) block rule, evaluated at depth 0. Mirrors `surface_block`
    /// minus the global river/beach pre-pass (which `SurfaceSystem` applies).
    pub surface_top: &'static SurfaceRule,
    /// Subsurface band block. Mirrors `subsurface_block` (altitude-independent).
    pub subsurface: Block,
}

// Shared top rules.
static GRASS_TOP: SurfaceRule = SurfaceRule::Block(Block::Grass);
static SAND_TOP: SurfaceRule = SurfaceRule::Block(Block::Sand);
static SNOW_TOP: SurfaceRule = SurfaceRule::Block(Block::Snow);

// Mountains: the original `>95 -> Snow, >78 -> Stone, else Grass` colour bands,
// now expressed as data (gen.rs `surface_block` Mountains arm).
static MOUNTAIN_TOP: SurfaceRule = SurfaceRule::Sequence(&[
    SurfaceRule::Condition { when: SurfaceCond::AboveY(95), then: &SurfaceRule::Block(Block::Snow) },
    SurfaceRule::Condition { when: SurfaceCond::AboveY(78), then: &SurfaceRule::Block(Block::Stone) },
    SurfaceRule::Block(Block::Grass),
]);

pub static BIOME_DEFS: &[BiomeDef] = &[
    BiomeDef { biome: Biome::Ocean,       surface_top: &SAND_TOP,     subsurface: Block::Dirt },
    BiomeDef { biome: Biome::Beach,       surface_top: &SAND_TOP,     subsurface: Block::Sand },
    BiomeDef { biome: Biome::River,       surface_top: &SAND_TOP,     subsurface: Block::Dirt },
    BiomeDef { biome: Biome::Desert,      surface_top: &SAND_TOP,     subsurface: Block::Sand },
    BiomeDef { biome: Biome::Plains,      surface_top: &GRASS_TOP,    subsurface: Block::Dirt },
    BiomeDef { biome: Biome::Savanna,     surface_top: &GRASS_TOP,    subsurface: Block::Dirt },
    BiomeDef { biome: Biome::Forest,      surface_top: &GRASS_TOP,    subsurface: Block::Dirt },
    BiomeDef { biome: Biome::BirchForest, surface_top: &GRASS_TOP,    subsurface: Block::Dirt },
    BiomeDef { biome: Biome::Swamp,       surface_top: &GRASS_TOP,    subsurface: Block::Dirt },
    BiomeDef { biome: Biome::Taiga,       surface_top: &GRASS_TOP,    subsurface: Block::Dirt },
    BiomeDef { biome: Biome::SnowyTundra, surface_top: &SNOW_TOP,     subsurface: Block::Dirt },
    BiomeDef { biome: Biome::SnowyTaiga,  surface_top: &SNOW_TOP,     subsurface: Block::Dirt },
    BiomeDef { biome: Biome::Mountains,   surface_top: &MOUNTAIN_TOP, subsurface: Block::Stone },
    BiomeDef { biome: Biome::SnowyPeaks,  surface_top: &SNOW_TOP,     subsurface: Block::Stone },
];

/// Look up a biome's definition. Falls back to the first entry (Ocean) for any
/// biome missing from the table — which a debug assertion forbids.
pub fn def(b: Biome) -> &'static BiomeDef {
    debug_assert!(
        BIOME_DEFS.iter().any(|d| d.biome == b),
        "biome missing from BIOME_DEFS",
    );
    BIOME_DEFS.iter().find(|d| d.biome == b).unwrap_or(&BIOME_DEFS[0])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_biome_has_a_def() {
        // Mirrors the Biome enum (ids 0..=13).
        for id in 0u8..=13 {
            let b = Biome::from_id(id);
            assert!(BIOME_DEFS.iter().any(|d| d.biome == b), "missing {b:?}");
        }
    }
}
