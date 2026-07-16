//! First-class worldgen biome modules.
//!
//! A game-facing [`Biome`](crate::biome::Biome) is only identity: id, name, and
//! render colours live in `src/biome`. Generation behavior lives here. Each
//! biome module owns its surface rule, tree placement, and ground-cover
//! decoration.

pub(crate) mod climate;
pub(crate) mod surface_table;
pub(crate) mod surfaces;
pub(crate) mod trees;

mod beach;
mod deep_ocean;
mod desert;
mod desert_lakes;
mod foothills;
mod forest;
mod grove;
mod meadow;
mod mountain_edge;
mod mountains;
mod ocean;
mod old_growth_taiga;
mod plains;
mod redwood_forest;
mod river;
mod savanna;
mod snowy_peaks;
mod snowy_slopes;
mod snowy_taiga;
mod snowy_tundra;
mod stony_peaks;
mod swamp;
mod taiga;
mod wetland;
mod windswept_hills;
mod wooded_hills;

use crate::biome::{Biome, BIOME_COUNT};
use crate::block::Block;
use crate::worldgen::feature::ConfiguredFeature;
use crate::worldgen::rng::FeatureRng;
use crate::worldgen::surface::rule::SurfaceRule;

pub(crate) type TreePicker = fn(&mut FeatureRng) -> &'static ConfiguredFeature;
pub(crate) type PlantPicker = fn(&mut FeatureRng) -> Option<Block>;

#[derive(Copy, Clone)]
pub(crate) enum TreeSupport {
    None,
    RedwoodBase,
}

#[derive(Copy, Clone)]
pub(crate) struct TreeProfile {
    pub density: f32,
    pub spacing_radius: i32,
    pub height_clearance: i32,
    pub support: TreeSupport,
    pub picker: TreePicker,
}

impl TreeProfile {
    pub const NONE: Self = Self {
        density: 0.0,
        spacing_radius: 3,
        height_clearance: 14,
        support: TreeSupport::None,
        picker: trees::oak_small,
    };

    pub const fn new(density: f32, picker: TreePicker) -> Self {
        Self {
            density,
            spacing_radius: 3,
            height_clearance: 14,
            support: TreeSupport::None,
            picker,
        }
    }

    pub const fn with_spacing(mut self, spacing_radius: i32) -> Self {
        self.spacing_radius = spacing_radius;
        self
    }

    pub const fn with_height_clearance(mut self, height_clearance: i32) -> Self {
        self.height_clearance = height_clearance;
        self
    }

    pub const fn with_support(mut self, support: TreeSupport) -> Self {
        self.support = support;
        self
    }
}

/// Clustering for podzol/grass GROUND COVER (ferns, tufts). When set, cover only
/// appears where a smooth low-frequency field is below `coverage`, so ferns form
/// `period`-sized patches with bare ground between, instead of an even per-column
/// sprinkle. Same blobby value-noise the flower patches use.
#[derive(Copy, Clone)]
pub(crate) struct CoverCluster {
    pub salt: u64,
    pub period: f32,
    pub coverage: f32,
}

#[derive(Copy, Clone)]
pub(crate) struct VegetationProfile {
    pub sand_cover: Option<PlantPicker>,
    pub podzol_cover: Option<PlantPicker>,
    pub grass_cover: Option<PlantPicker>,
    /// Optional clustering applied to `podzol_cover` / `grass_cover`.
    pub cover_cluster: Option<CoverCluster>,
    pub flower_palette: &'static [Block],
    pub flower_coverage: f32,
    pub flower_density: f32,
    pub grass_tuft: Block,
    pub grass_density: f32,
}

impl VegetationProfile {
    pub const NONE: Self = Self {
        sand_cover: None,
        podzol_cover: None,
        grass_cover: None,
        cover_cluster: None,
        flower_palette: &[],
        flower_coverage: 0.0,
        flower_density: 0.0,
        grass_tuft: Block::ShortGrass,
        grass_density: 0.0,
    };

    pub const fn grass(grass_tuft: Block, grass_density: f32) -> Self {
        Self {
            grass_tuft,
            grass_density,
            ..Self::NONE
        }
    }

    pub const fn with_flowers(
        mut self,
        palette: &'static [Block],
        coverage: f32,
        density: f32,
    ) -> Self {
        self.flower_palette = palette;
        self.flower_coverage = coverage;
        self.flower_density = density;
        self
    }

    pub const fn with_sand_cover(mut self, picker: PlantPicker) -> Self {
        self.sand_cover = Some(picker);
        self
    }

    pub const fn with_podzol_cover(mut self, picker: PlantPicker) -> Self {
        self.podzol_cover = Some(picker);
        self
    }

    pub const fn with_grass_cover(mut self, picker: PlantPicker) -> Self {
        self.grass_cover = Some(picker);
        self
    }

    pub const fn with_cover_cluster(mut self, cluster: CoverCluster) -> Self {
        self.cover_cluster = Some(cluster);
        self
    }
}

/// Where a biome lays a snow layer on the bare ground (one cell above the
/// column's post-cave surface, placed by the ground-vegetation pass). The
/// grass underneath renders its snowy sides while the layer sits on it.
#[derive(Copy, Clone)]
pub(crate) enum SnowCover {
    /// Never — the default for temperate biomes.
    None,
    /// Every dry land column — the snowy biomes.
    Always,
    /// Only columns whose bare-ground surface is strictly above this Y — the
    /// altitude snow caps (mountains). Keep the line in lockstep with the
    /// biome's `SurfaceAboveY` cap band so the cap material and the layer
    /// appear together.
    AboveSurfaceY(i32),
}

impl SnowCover {
    /// Whether a column whose bare-ground surface sits at `surf_y` is covered.
    #[inline]
    pub fn covers(self, surf_y: i32) -> bool {
        match self {
            SnowCover::None => false,
            SnowCover::Always => true,
            SnowCover::AboveSurfaceY(line) => surf_y > line,
        }
    }
}

pub(crate) struct BiomeSpec {
    pub biome: Biome,
    pub surface: &'static SurfaceRule,
    pub trees: TreeProfile,
    pub vegetation: VegetationProfile,
    pub snow_cover: SnowCover,
}

pub(crate) const MAX_TREE_SPACING_RADIUS: i32 = 10;

pub(crate) static SPECS: [&BiomeSpec; BIOME_COUNT] = [
    &ocean::SPEC,
    &beach::SPEC,
    &river::SPEC,
    &desert::SPEC,
    &plains::SPEC,
    &savanna::SPEC,
    &forest::SPEC,
    &swamp::SPEC,
    &taiga::SPEC,
    &snowy_tundra::SPEC,
    &snowy_taiga::SPEC,
    &mountains::SPEC,
    &snowy_peaks::SPEC,
    &deep_ocean::SPEC,
    &foothills::SPEC,
    &wetland::SPEC,
    &redwood_forest::SPEC,
    &old_growth_taiga::SPEC,
    &meadow::SPEC,
    &grove::SPEC,
    &snowy_slopes::SPEC,
    &windswept_hills::SPEC,
    &stony_peaks::SPEC,
    &wooded_hills::SPEC,
    &mountain_edge::SPEC,
    &desert_lakes::SPEC,
];

#[inline]
pub(crate) fn spec(biome: Biome) -> &'static BiomeSpec {
    let i = biome.id() as usize - 1;
    debug_assert!(
        i < SPECS.len() && SPECS[i].biome == biome,
        "worldgen biome specs are not id-ordered"
    );
    SPECS[i.min(SPECS.len() - 1)]
}

#[cfg(all(test, feature = "worldgen-tests"))]
mod tests {
    use super::*;

    #[test]
    fn specs_are_one_to_one_with_game_biomes() {
        for (i, spec) in SPECS.iter().enumerate() {
            assert_eq!(spec.biome.id() as usize, i + 1);
        }
    }
}
