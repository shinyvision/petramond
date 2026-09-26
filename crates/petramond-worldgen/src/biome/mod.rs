//! Worldgen biome behaviour.
//!
//! A game-facing [`Biome`] is identity plus its row in `assets/biomes.json`.
//! That one row is the whole definition: render colours and ambience (read by
//! `petramond-world`), tree placement (`trees`, see [`trees`]) and the
//! generation rules this module serves (`generation`: surface rule stack,
//! ground cover, snow and behaviour flags — parsed by
//! [`crate::data::biome_gen`]). There is no per-biome Rust: a pack retunes a
//! biome's ground by overriding its row, or adds a biome with a row of its
//! own and places it in climate space with rows in `climate_table.json`
//! (see [`crate::data::climate_table`]).

pub mod climate;
#[cfg(test)]
pub mod surface_table;
pub mod trees;

use crate::rng::FeatureRng;
use crate::surface::rule::SurfaceRule;
use petramond_world::biome::Biome;
use petramond_world::block::Block;
use serde::Deserialize;

/// A ground-cover roll: `chance` that the column gets a plant at all, then
/// one `0..=99` draw picks the first entry whose bound it is below. The last
/// bound is 100, so a column that passes the chance always gets a plant.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoverRoll {
    pub chance: f32,
    pub roll: Vec<(i32, Block)>,
}

impl CoverRoll {
    /// The plant for a column, drawing from its positional stream: exactly
    /// one chance draw, then one pick draw only when the chance passes.
    pub fn pick(&self, rng: &mut FeatureRng) -> Option<Block> {
        if !rng.chance(self.chance) {
            return None;
        }
        let r = rng.next_i32(0, 99);
        self.roll
            .iter()
            .find(|(below, _)| r < *below)
            .map(|&(_, block)| block)
    }

    /// Bounds strictly rising, inside `1..=100`, and ending at 100.
    pub(crate) fn validate(&self) -> Result<(), String> {
        let bounds: Vec<i32> = self.roll.iter().map(|(below, _)| *below).collect();
        let rising = bounds.windows(2).all(|w| w[0] < w[1]);
        if !self.chance.is_finite()
            || !(0.0..=1.0).contains(&self.chance)
            || bounds.first().is_none_or(|&b| b < 1)
            || !rising
            || bounds.last() != Some(&100)
        {
            return Err(format!(
                "a cover roll needs a chance in 0..=1 and rising bounds ending at 100, got {bounds:?}"
            ));
        }
        Ok(())
    }
}

/// Clustering for podzol/grass GROUND COVER (ferns, tufts). When set, cover only
/// appears where a smooth low-frequency field is below `coverage`, so ferns form
/// `period`-sized patches with bare ground between, instead of an even per-column
/// sprinkle. Same blobby value-noise the flower patches use.
#[derive(Copy, Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoverCluster {
    pub salt: u64,
    pub period: f32,
    pub coverage: f32,
}

/// Ground cover for surfaces the fixed slots of [`VegetationProfile`] do not
/// name — mycelium, or a pack's own ground block: a column whose bare ground
/// is one of `on` makes `roll` (masked by the biome's cover cluster when
/// `clustered`). Checked before the fixed slots, so an entry may also take
/// over sand, podzol or grass.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GroundCover {
    pub on: Vec<Block>,
    pub roll: CoverRoll,
    #[serde(default)]
    pub clustered: bool,
}

/// A biome's ground vegetation: what the vegetation pass may put on each kind
/// of bare ground.
#[derive(Copy, Clone, Debug)]
pub struct VegetationProfile {
    /// Covers keyed by surface block, checked first.
    pub covers: &'static [GroundCover],
    pub sand_cover: Option<&'static CoverRoll>,
    pub podzol_cover: Option<&'static CoverRoll>,
    pub grass_cover: Option<&'static CoverRoll>,
    /// Optional clustering applied to `podzol_cover` / `grass_cover`.
    pub cover_cluster: Option<CoverCluster>,
    pub flower_palette: &'static [Block],
    pub flower_coverage: f32,
    pub flower_density: f32,
    pub grass_tuft: Block,
    pub grass_density: f32,
    pub hemp_anchor_chance: f32,
}

/// Where a biome lays a snow layer on the bare ground (one cell above the
/// column's post-cave surface, placed by the ground-vegetation pass). The
/// grass underneath renders its snowy sides while the layer sits on it.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum SnowCover {
    /// Never — the default for temperate biomes.
    None,
    /// Every dry land column — the snowy biomes.
    Always,
    /// Only columns whose bare-ground surface is strictly above this Y — the
    /// altitude snow caps (mountains). Keep the line in lockstep with the
    /// biome's `surface_above_y` cap band so the cap material and the layer
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

/// What other stages ask about a biome, stated on its row instead of in
/// per-stage lists of biome names.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct BiomeFlags {
    /// Open sea: a beach forms on low land near it.
    pub ocean: bool,
    /// The biome may turn into beach on low land near an ocean.
    pub beach_base: bool,
    /// Standing water belongs here (the audits do not flag it as a leak).
    pub wet: bool,
    /// Hill and mountain terrain (the audits' relief checks).
    pub mountain: bool,
}

pub struct BiomeSpec {
    pub biome: Biome,
    pub surface: &'static SurfaceRule,
    pub vegetation: VegetationProfile,
    pub snow_cover: SnowCover,
    pub flags: BiomeFlags,
}

/// The loaded generation rules of `biome`.
#[inline]
pub fn spec(biome: Biome) -> &'static BiomeSpec {
    crate::data::biome_gen::spec(biome)
}

/// Every biome's generation rules, in id order.
pub fn specs() -> &'static [BiomeSpec] {
    crate::data::biome_gen::specs()
}
