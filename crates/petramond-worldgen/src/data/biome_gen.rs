//! The `generation` field of a biome row (`assets/biomes.json`), parsed into
//! a [`BiomeSpec`]: the surface rule stack the terrain fill skins columns
//! with, the ground cover the vegetation pass plants, where snow lies, and the
//! behaviour flags other stages ask about.
//!
//! The biome layer carries the object verbatim (`Biome::generation`); this
//! module owns its vocabulary:
//!
//! ```json
//! "generation": {
//!   "surface": [{"if": {"depth_from_top": 0}, "then": "petramond:grass"},
//!               {"if": {"depth_from_top": 3}, "then": "petramond:dirt"},
//!               "petramond:stone"],
//!   "vegetation": {"grass": {"tuft": "petramond:short_grass", "density": 0.14},
//!                  "flowers": {"palette": ["petramond:poppy"], "coverage": 0.1,
//!                              "density": 0.15},
//!                  "hemp": 0.0045},
//!   "snow": "always",
//!   "flags": ["no_beach"]
//! }
//! ```
//!
//! A surface rule is a block name, a list (the first entry that yields a
//! block wins) or `{"if": condition, "then": rule}`. Conditions:
//! `"underwater"`, `{"depth_from_top": n}`, `{"surface_above_y": y}` and
//! `{"cluster_noise_below": {"salt", "threshold", "period"}}`. Ground cover
//! rolls are `{"chance": p, "roll": [[bound, block], ...]}` (see
//! [`CoverRoll`]): the fixed `sand_cover` / `podzol_cover` / `grass_cover`
//! slots, plus `covers` for any other ground —
//! `[{"on": ["petramond:mycelium"], "roll": {...}, "clustered": false}]`
//! (see [`GroundCover`]). Every row must state its generation: a row without one
//! would generate as nothing.

use std::sync::LazyLock;

use petramond_world::biome::Biome;
use petramond_world::block::Block;
use serde::Deserialize;

use crate::biome::{
    BiomeFlags, BiomeSpec, CoverCluster, CoverRoll, GroundCover, SnowCover, VegetationProfile,
};
use crate::surface::rule::{SurfaceCond, SurfaceRule};

/// The `generation` object as written on a biome row.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawGeneration {
    surface: RawRule,
    #[serde(default)]
    vegetation: RawVegetation,
    #[serde(default)]
    snow: Option<RawSnow>,
    #[serde(default)]
    flags: Vec<RawFlag>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawRule {
    Block(Block),
    Sequence(Vec<RawRule>),
    Condition(RawCondition),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCondition {
    #[serde(rename = "if")]
    when: RawCond,
    then: Box<RawRule>,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
enum RawCond {
    Underwater,
    DepthFromTop(u32),
    SurfaceAboveY(i32),
    ClusterNoiseBelow {
        salt: u64,
        threshold: f32,
        period: f32,
    },
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawVegetation {
    grass: Option<RawGrass>,
    flowers: Option<RawFlowers>,
    hemp: f32,
    covers: Vec<GroundCover>,
    sand_cover: Option<CoverRoll>,
    podzol_cover: Option<CoverRoll>,
    grass_cover: Option<CoverRoll>,
    cover_cluster: Option<CoverCluster>,
}

/// The plain grass-ground scatter: `tuft` with `density` per column.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawGrass {
    tuft: Block,
    density: f32,
}

/// Single-species flower patches: where the patch field exceeds
/// `1 - coverage`, a column flowers with `density`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFlowers {
    palette: Vec<Block>,
    coverage: f32,
    density: f32,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum RawSnow {
    Always,
    AboveSurfaceY(i32),
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RawFlag {
    Ocean,
    NoBeach,
    Wet,
    Mountain,
}

impl RawRule {
    /// The rule, stored for the process: the table is built once.
    fn resolve(self) -> &'static SurfaceRule {
        Box::leak(Box::new(self.into_rule()))
    }

    fn into_rule(self) -> SurfaceRule {
        match self {
            RawRule::Block(block) => SurfaceRule::Block(block),
            RawRule::Sequence(rules) => SurfaceRule::Sequence(Box::leak(
                rules
                    .into_iter()
                    .map(RawRule::into_rule)
                    .collect::<Box<[SurfaceRule]>>(),
            )),
            RawRule::Condition(c) => SurfaceRule::Condition {
                when: match c.when {
                    RawCond::Underwater => SurfaceCond::Underwater,
                    RawCond::DepthFromTop(n) => SurfaceCond::DepthFromTop(n),
                    RawCond::SurfaceAboveY(y) => SurfaceCond::SurfaceAboveY(y),
                    RawCond::ClusterNoiseBelow {
                        salt,
                        threshold,
                        period,
                    } => SurfaceCond::ClusterNoiseBelow {
                        salt,
                        threshold,
                        period,
                    },
                },
                then: c.then.resolve(),
            },
        }
    }
}

/// A validated cover roll, stored for the process: the table is built once.
fn leak_roll(roll: Option<CoverRoll>, what: &str) -> Result<Option<&'static CoverRoll>, String> {
    let Some(roll) = roll else {
        return Ok(None);
    };
    roll.validate().map_err(|e| format!("{what}: {e}"))?;
    Ok(Some(Box::leak(Box::new(roll))))
}

impl RawVegetation {
    fn resolve(self) -> Result<VegetationProfile, String> {
        let (grass_tuft, grass_density) = self
            .grass
            .map_or((Block::ShortGrass, 0.0), |g| (g.tuft, g.density));
        let (flower_palette, flower_coverage, flower_density): (&'static [Block], f32, f32) =
            match self.flowers {
                Some(f) if f.palette.is_empty() => return Err("flowers: empty palette".into()),
                Some(f) => (f.palette.leak(), f.coverage, f.density),
                None => (&[], 0.0, 0.0),
            };
        for cover in &self.covers {
            if cover.on.is_empty() {
                return Err("covers: an entry needs at least one `on` block".into());
            }
            cover.roll.validate().map_err(|e| format!("covers: {e}"))?;
        }
        Ok(VegetationProfile {
            covers: self.covers.leak(),
            sand_cover: leak_roll(self.sand_cover, "sand_cover")?,
            podzol_cover: leak_roll(self.podzol_cover, "podzol_cover")?,
            grass_cover: leak_roll(self.grass_cover, "grass_cover")?,
            cover_cluster: self.cover_cluster,
            flower_palette,
            flower_coverage,
            flower_density,
            grass_tuft,
            grass_density,
            hemp_anchor_chance: self.hemp,
        })
    }
}

/// Parse one row's `generation` text for `biome`.
pub(crate) fn parse(biome: Biome, generation: Option<&str>) -> Result<BiomeSpec, String> {
    let text = generation.ok_or("no `generation` object")?;
    let raw: RawGeneration =
        serde_json::from_str(text).map_err(|e| format!("generation: {e}"))?;
    let mut flags = BiomeFlags {
        beach_base: true,
        ..BiomeFlags::default()
    };
    for flag in raw.flags {
        match flag {
            RawFlag::Ocean => flags.ocean = true,
            RawFlag::NoBeach => flags.beach_base = false,
            RawFlag::Wet => flags.wet = true,
            RawFlag::Mountain => flags.mountain = true,
        }
    }
    Ok(BiomeSpec {
        biome,
        surface: raw.surface.resolve(),
        vegetation: raw
            .vegetation
            .resolve()
            .map_err(|e| format!("generation: vegetation: {e}"))?,
        snow_cover: match raw.snow {
            None => SnowCover::None,
            Some(RawSnow::Always) => SnowCover::Always,
            Some(RawSnow::AboveSurfaceY(y)) => SnowCover::AboveSurfaceY(y),
        },
        flags,
    })
}

/// Every biome's generation rules, in id order.
pub(crate) fn specs() -> &'static [BiomeSpec] {
    static TABLE: LazyLock<Box<[BiomeSpec]>> = LazyLock::new(|| {
        Biome::all()
            .map(|biome| {
                parse(biome, biome.generation())
                    .unwrap_or_else(|e| panic!("biomes.json: biome '{}': {e}", biome.key()))
            })
            .collect()
    });
    &TABLE
}

/// The loaded generation rules of `biome`.
#[inline]
pub(crate) fn spec(biome: Biome) -> &'static BiomeSpec {
    &specs()[usize::from(biome.id()) - 1]
}

#[cfg(test)]
mod tests;
