pub mod climate;
#[cfg(test)]
pub mod surface_table;
pub mod trees;

use crate::rng::FeatureRng;
use crate::surface::rule::SurfaceRule;
use petramond_world::biome::Biome;
use petramond_world::block::Block;
use serde::Deserialize;

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoverRoll {
    pub chance: f32,
    pub roll: Vec<(i32, Block)>,
}

impl CoverRoll {
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

#[derive(Copy, Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoverCluster {
    pub salt: u64,
    pub period: f32,
    pub coverage: f32,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GroundCover {
    pub on: Vec<Block>,
    pub roll: CoverRoll,
    #[serde(default)]
    pub clustered: bool,
}

#[derive(Copy, Clone, Debug)]
pub struct VegetationProfile {
    pub covers: &'static [GroundCover],
    pub sand_cover: Option<&'static CoverRoll>,
    pub podzol_cover: Option<&'static CoverRoll>,
    pub grass_cover: Option<&'static CoverRoll>,
    pub cover_cluster: Option<CoverCluster>,
    pub flower_palette: &'static [Block],
    pub flower_coverage: f32,
    pub flower_density: f32,
    pub grass_tuft: Block,
    pub grass_density: f32,
    pub hemp_anchor_chance: f32,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum SnowCover {
    None,
    Always,
    AboveSurfaceY(i32),
}

impl SnowCover {
    #[inline]
    pub fn covers(self, surf_y: i32) -> bool {
        match self {
            SnowCover::None => false,
            SnowCover::Always => true,
            SnowCover::AboveSurfaceY(line) => surf_y > line,
        }
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct BiomeFlags {
    pub ocean: bool,
    pub beach_base: bool,
    pub wet: bool,
    pub mountain: bool,
}

pub struct BiomeSpec {
    pub biome: Biome,
    pub surface: &'static SurfaceRule,
    pub vegetation: VegetationProfile,
    pub snow_cover: SnowCover,
    pub flags: BiomeFlags,
}

#[inline]
pub fn spec(biome: Biome) -> &'static BiomeSpec {
    crate::data::biome_gen::spec(biome)
}

pub fn specs() -> &'static [BiomeSpec] {
    crate::data::biome_gen::specs()
}
