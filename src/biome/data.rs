use super::definition::BiomeDef;
use super::Biome;

/// The storybook biome palette (WIKI/visual-style.md). Curation rules, applied
/// 2026-07 with the worldgen stylization pass:
///
/// - GRASS/FOLIAGE greens are warm-shifted (toward yellow-green) and sit in a
///   few deliberate families — lush (plains/meadow), deep woodland (forest),
///   golden dry (savanna/desert scrub), cool sage (taiga/cold), muted alpine
///   (peaks) — instead of one neon green with per-biome noise.
/// - WATER is one turquoise family across the world, varied only slightly per
///   biome (murky in swamp/wetland, ink-deep in deep ocean), so water always
///   reads as inviting storybook water.
/// - FOG colours are airier (lighter, softer) than their biome mood suggests:
///   they feed the atmosphere haze and the sky horizon, and distance must
///   LIGHTEN. Only mood biomes (swamp, redwood) keep a denser tinted fog.
pub(super) const BIOME_DEFS: &[BiomeDef] = &[
    BiomeDef {
        biome: Biome::Ocean,
        name: "ocean",
        fog_color: [0.52, 0.70, 0.94],
        grass_color: [0.52, 0.74, 0.40],
        foliage_color: [0.46, 0.70, 0.36],
        water_color: [0.10, 0.44, 0.80],
    },
    BiomeDef {
        biome: Biome::Beach,
        name: "beach",
        fog_color: [0.94, 0.89, 0.74],
        grass_color: [0.72, 0.78, 0.42],
        foliage_color: [0.66, 0.74, 0.38],
        water_color: [0.14, 0.54, 0.84],
    },
    BiomeDef {
        biome: Biome::River,
        name: "river",
        fog_color: [0.62, 0.78, 0.95],
        grass_color: [0.50, 0.76, 0.38],
        foliage_color: [0.44, 0.72, 0.34],
        water_color: [0.14, 0.52, 0.82],
    },
    BiomeDef {
        biome: Biome::Desert,
        name: "desert",
        fog_color: [0.97, 0.87, 0.66],
        grass_color: [0.85, 0.74, 0.36],
        foliage_color: [0.79, 0.68, 0.32],
        water_color: [0.16, 0.56, 0.82],
    },
    BiomeDef {
        biome: Biome::Plains,
        name: "plains",
        fog_color: [0.68, 0.84, 0.98],
        grass_color: [0.48, 0.78, 0.30],
        foliage_color: [0.40, 0.72, 0.26],
        water_color: [0.13, 0.52, 0.82],
    },
    BiomeDef {
        biome: Biome::Savanna,
        name: "savanna",
        fog_color: [0.84, 0.82, 0.70],
        grass_color: [0.80, 0.72, 0.34],
        foliage_color: [0.72, 0.66, 0.30],
        water_color: [0.16, 0.52, 0.78],
    },
    BiomeDef {
        biome: Biome::Forest,
        name: "forest",
        fog_color: [0.58, 0.78, 0.96],
        grass_color: [0.30, 0.66, 0.24],
        foliage_color: [0.34, 0.70, 0.24],
        water_color: [0.11, 0.46, 0.78],
    },
    BiomeDef {
        biome: Biome::Swamp,
        name: "swamp",
        fog_color: [0.56, 0.66, 0.62],
        grass_color: [0.36, 0.56, 0.30],
        foliage_color: [0.32, 0.52, 0.26],
        water_color: [0.20, 0.42, 0.44],
    },
    BiomeDef {
        biome: Biome::Taiga,
        name: "taiga",
        fog_color: [0.66, 0.76, 0.84],
        grass_color: [0.44, 0.66, 0.42],
        foliage_color: [0.38, 0.62, 0.38],
        water_color: [0.13, 0.46, 0.74],
    },
    BiomeDef {
        biome: Biome::SnowyTundra,
        name: "snowy_tundra",
        fog_color: [0.82, 0.86, 0.92],
        grass_color: [0.62, 0.72, 0.58],
        foliage_color: [0.56, 0.70, 0.54],
        water_color: [0.18, 0.52, 0.80],
    },
    BiomeDef {
        biome: Biome::SnowyTaiga,
        name: "snowy_taiga",
        fog_color: [0.78, 0.83, 0.90],
        grass_color: [0.50, 0.68, 0.50],
        foliage_color: [0.44, 0.64, 0.46],
        water_color: [0.16, 0.48, 0.76],
    },
    BiomeDef {
        biome: Biome::Mountains,
        name: "mountains",
        fog_color: [0.70, 0.78, 0.88],
        grass_color: [0.50, 0.70, 0.42],
        foliage_color: [0.44, 0.66, 0.38],
        water_color: [0.13, 0.48, 0.78],
    },
    BiomeDef {
        biome: Biome::SnowyPeaks,
        name: "snowy_peaks",
        fog_color: [0.84, 0.87, 0.93],
        grass_color: [0.76, 0.84, 0.76],
        foliage_color: [0.68, 0.80, 0.68],
        water_color: [0.20, 0.54, 0.82],
    },
    BiomeDef {
        biome: Biome::DeepOcean,
        name: "deep_ocean",
        fog_color: [0.46, 0.66, 0.94],
        grass_color: [0.48, 0.70, 0.38],
        foliage_color: [0.42, 0.66, 0.34],
        water_color: [0.05, 0.26, 0.62],
    },
    BiomeDef {
        biome: Biome::Foothills,
        name: "foothills",
        fog_color: [0.66, 0.80, 0.92],
        grass_color: [0.52, 0.74, 0.40],
        foliage_color: [0.46, 0.70, 0.36],
        water_color: [0.13, 0.48, 0.80],
    },
    BiomeDef {
        biome: Biome::Wetland,
        name: "wetland",
        fog_color: [0.62, 0.72, 0.72],
        grass_color: [0.38, 0.62, 0.30],
        foliage_color: [0.34, 0.58, 0.26],
        water_color: [0.18, 0.44, 0.54],
    },
    BiomeDef {
        biome: Biome::RedwoodForest,
        name: "redwood_forest",
        fog_color: [0.52, 0.68, 0.62],
        grass_color: [0.32, 0.56, 0.28],
        foliage_color: [0.28, 0.52, 0.24],
        water_color: [0.13, 0.42, 0.60],
    },
    BiomeDef {
        biome: Biome::OldGrowthTaiga,
        name: "old_growth_taiga",
        fog_color: [0.60, 0.70, 0.68],
        grass_color: [0.36, 0.60, 0.32],
        foliage_color: [0.32, 0.56, 0.30],
        water_color: [0.13, 0.44, 0.64],
    },
    BiomeDef {
        biome: Biome::CherryGrove,
        name: "cherry_grove",
        fog_color: [0.94, 0.84, 0.88],
        grass_color: [0.46, 0.78, 0.42],
        foliage_color: [0.56, 0.80, 0.48],
        water_color: [0.20, 0.60, 0.86],
    },
    BiomeDef {
        biome: Biome::Meadow,
        name: "meadow",
        fog_color: [0.72, 0.86, 0.96],
        grass_color: [0.44, 0.80, 0.34],
        foliage_color: [0.38, 0.74, 0.30],
        water_color: [0.16, 0.54, 0.84],
    },
    BiomeDef {
        biome: Biome::Grove,
        name: "grove",
        fog_color: [0.80, 0.86, 0.92],
        grass_color: [0.52, 0.68, 0.52],
        foliage_color: [0.46, 0.64, 0.46],
        water_color: [0.16, 0.50, 0.80],
    },
    BiomeDef {
        biome: Biome::SnowySlopes,
        name: "snowy_slopes",
        fog_color: [0.85, 0.88, 0.94],
        grass_color: [0.68, 0.78, 0.72],
        foliage_color: [0.62, 0.76, 0.68],
        water_color: [0.18, 0.52, 0.82],
    },
    BiomeDef {
        biome: Biome::WindsweptHills,
        name: "windswept_hills",
        fog_color: [0.68, 0.77, 0.86],
        grass_color: [0.48, 0.66, 0.44],
        foliage_color: [0.42, 0.62, 0.40],
        water_color: [0.13, 0.48, 0.78],
    },
    BiomeDef {
        biome: Biome::StonyPeaks,
        name: "stony_peaks",
        fog_color: [0.78, 0.80, 0.83],
        grass_color: [0.56, 0.64, 0.54],
        foliage_color: [0.50, 0.60, 0.50],
        water_color: [0.16, 0.52, 0.80],
    },
    BiomeDef {
        biome: Biome::WoodedHills,
        name: "wooded_hills",
        fog_color: [0.58, 0.78, 0.94],
        grass_color: [0.32, 0.66, 0.24],
        foliage_color: [0.34, 0.68, 0.26],
        water_color: [0.12, 0.46, 0.78],
    },
    BiomeDef {
        biome: Biome::MountainEdge,
        name: "mountain_edge",
        fog_color: [0.68, 0.80, 0.92],
        grass_color: [0.50, 0.72, 0.40],
        foliage_color: [0.44, 0.68, 0.36],
        water_color: [0.13, 0.48, 0.80],
    },
    BiomeDef {
        biome: Biome::DesertLakes,
        name: "desert_lakes",
        fog_color: [0.96, 0.85, 0.64],
        grass_color: [0.83, 0.72, 0.36],
        foliage_color: [0.77, 0.66, 0.32],
        water_color: [0.18, 0.58, 0.84],
    },
];

#[inline]
pub(super) fn from_id(id: u8) -> Biome {
    BIOME_DEFS
        .get(id.saturating_sub(1) as usize)
        .map_or(Biome::Ocean, |d| d.biome)
}

#[inline]
pub(super) fn def(biome: Biome) -> &'static BiomeDef {
    &BIOME_DEFS[(biome.id() - 1) as usize]
}
