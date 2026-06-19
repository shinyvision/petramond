//! Per-biome surface definitions.
//!
//! Each `BiomeDef` carries a layered `SurfaceRule` stack resolved against a real
//! `depth_from_top` (the driver's skin pass walks each contiguous solid run top
//! down). `DepthFromTop(n)` matches `depth <= n`, so a stack reads as cumulative
//! bands: e.g. Grass at depth 0, Dirt down to depth N, Stone below. `AboveY(n)`
//! gates by world Y for altitude bands (mountain rock + snow caps). This is what
//! makes mountains show the mandated 1 grass + 2 dirt + stone strata on their
//! faces, deserts/beaches deep sand, and ocean floors their own material.

use crate::biome::Biome;
use crate::block::Block;
use crate::worldgen::surface::rule::{SurfaceCond, SurfaceRule};

pub struct BiomeDef {
    pub biome: Biome,
    /// Layered top→subsurface→core rule, resolved per solid voxel by depth.
    pub surface: &'static SurfaceRule,
}

// --- Layered surface stacks (shared across biomes where identical) ---

// Normal land: grass cap, a few dirt, stone core.
static PLAINS_TOP: SurfaceRule = SurfaceRule::Sequence(&[
    SurfaceRule::Condition { when: SurfaceCond::DepthFromTop(0), then: &SurfaceRule::Block(Block::Grass) },
    SurfaceRule::Condition { when: SurfaceCond::DepthFromTop(3), then: &SurfaceRule::Block(Block::Dirt) },
    SurfaceRule::Block(Block::Stone),
]);

// Foothills / lower mountain: thin soil so stone shows on steeper faces.
static FOOTHILLS_TOP: SurfaceRule = SurfaceRule::Sequence(&[
    SurfaceRule::Condition { when: SurfaceCond::DepthFromTop(0), then: &SurfaceRule::Block(Block::Grass) },
    SurfaceRule::Condition { when: SurfaceCond::DepthFromTop(2), then: &SurfaceRule::Block(Block::Dirt) },
    SurfaceRule::Block(Block::Stone),
]);

// Snow cap over a stone core (a column whose surface is above the snow line).
static SNOW_CAP: SurfaceRule = SurfaceRule::Sequence(&[
    SurfaceRule::Condition { when: SurfaceCond::DepthFromTop(0), then: &SurfaceRule::Block(Block::Snow) },
    SurfaceRule::Block(Block::Stone),
]);
// Mountains: the surface band is chosen by the COLUMN'S height (not per-voxel Y),
// so a peak is snow-over-stone, a shoulder is bare rock, and lower slopes are
// low-sat green grass over thin soil exposing the mandated 1 grass + 2 dirt +
// stone strata on steep faces. Because the bands key off surf_y, overhang
// undersides (depth > 0) correctly resolve to stone and never to snow.
static MOUNTAIN_TOP: SurfaceRule = SurfaceRule::Sequence(&[
    SurfaceRule::Condition { when: SurfaceCond::SurfaceAboveY(143), then: &SNOW_CAP },
    SurfaceRule::Condition { when: SurfaceCond::SurfaceAboveY(135), then: &SurfaceRule::Block(Block::Stone) },
    SurfaceRule::Condition { when: SurfaceCond::DepthFromTop(0), then: &SurfaceRule::Block(Block::Grass) },
    SurfaceRule::Condition { when: SurfaceCond::DepthFromTop(2), then: &SurfaceRule::Block(Block::Dirt) },
    SurfaceRule::Block(Block::Stone),
]);

// Snowy biomes / peaks: snow cap over stone.
static SNOW_TOP: SurfaceRule = SurfaceRule::Sequence(&[
    SurfaceRule::Condition { when: SurfaceCond::DepthFromTop(1), then: &SurfaceRule::Block(Block::Snow) },
    SurfaceRule::Block(Block::Stone),
]);

// Desert / beach: deep sand over stone.
static SAND_DEEP: SurfaceRule = SurfaceRule::Sequence(&[
    SurfaceRule::Condition { when: SurfaceCond::DepthFromTop(4), then: &SurfaceRule::Block(Block::Sand) },
    SurfaceRule::Block(Block::Stone),
]);

// Shallow ocean floor: sandy, then dirt, then stone.
static OCEAN_FLOOR: SurfaceRule = SurfaceRule::Sequence(&[
    SurfaceRule::Condition { when: SurfaceCond::DepthFromTop(2), then: &SurfaceRule::Block(Block::Sand) },
    SurfaceRule::Condition { when: SurfaceCond::DepthFromTop(4), then: &SurfaceRule::Block(Block::Dirt) },
    SurfaceRule::Block(Block::Stone),
]);

// Deep ocean floor: dark muddy dirt over stone (reads distinct under dark tint).
static DEEP_OCEAN_FLOOR: SurfaceRule = SurfaceRule::Sequence(&[
    SurfaceRule::Condition { when: SurfaceCond::DepthFromTop(2), then: &SurfaceRule::Block(Block::Dirt) },
    SurfaceRule::Block(Block::Stone),
]);

// Wetland / swamp: sand at/under the waterline, grass + dirt above.
static WETLAND_TOP: SurfaceRule = SurfaceRule::Sequence(&[
    SurfaceRule::Condition { when: SurfaceCond::Underwater, then: &SurfaceRule::Block(Block::Sand) },
    SurfaceRule::Condition { when: SurfaceCond::DepthFromTop(0), then: &SurfaceRule::Block(Block::Grass) },
    SurfaceRule::Condition { when: SurfaceCond::DepthFromTop(3), then: &SurfaceRule::Block(Block::Dirt) },
    SurfaceRule::Block(Block::Stone),
]);

pub static BIOME_DEFS: &[BiomeDef] = &[
    BiomeDef { biome: Biome::Ocean,       surface: &OCEAN_FLOOR },
    BiomeDef { biome: Biome::Beach,       surface: &SAND_DEEP },
    BiomeDef { biome: Biome::River,       surface: &OCEAN_FLOOR },
    BiomeDef { biome: Biome::Desert,      surface: &SAND_DEEP },
    BiomeDef { biome: Biome::Plains,      surface: &PLAINS_TOP },
    BiomeDef { biome: Biome::Savanna,     surface: &PLAINS_TOP },
    BiomeDef { biome: Biome::Forest,      surface: &PLAINS_TOP },
    BiomeDef { biome: Biome::BirchForest, surface: &PLAINS_TOP },
    BiomeDef { biome: Biome::Swamp,       surface: &WETLAND_TOP },
    BiomeDef { biome: Biome::Taiga,       surface: &PLAINS_TOP },
    BiomeDef { biome: Biome::SnowyTundra, surface: &SNOW_TOP },
    BiomeDef { biome: Biome::SnowyTaiga,  surface: &SNOW_TOP },
    BiomeDef { biome: Biome::Mountains,   surface: &MOUNTAIN_TOP },
    BiomeDef { biome: Biome::SnowyPeaks,  surface: &SNOW_TOP },
    BiomeDef { biome: Biome::DeepOcean,   surface: &DEEP_OCEAN_FLOOR },
    BiomeDef { biome: Biome::Foothills,   surface: &FOOTHILLS_TOP },
    BiomeDef { biome: Biome::Wetland,     surface: &WETLAND_TOP },
];

/// Look up a biome's definition by id — O(1), because `BIOME_DEFS` is ordered to
/// match `Biome::id()` (asserted by `defs_are_id_ordered`).
#[inline]
pub fn def(b: Biome) -> &'static BiomeDef {
    let i = b.id() as usize;
    debug_assert!(i < BIOME_DEFS.len() && BIOME_DEFS[i].biome == b, "BIOME_DEFS not id-ordered");
    &BIOME_DEFS[i.min(BIOME_DEFS.len() - 1)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defs_are_id_ordered() {
        // O(1) `def()` relies on row i holding the biome whose id == i.
        for id in 0u8..=16 {
            let b = Biome::from_id(id);
            assert_eq!(BIOME_DEFS[id as usize].biome, b, "row {id} != {b:?}");
        }
        assert_eq!(BIOME_DEFS.len(), 17);
    }
}
