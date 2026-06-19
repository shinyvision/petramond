use super::definition::{BiomeDef, HumidityBand};
use super::Biome;

pub(super) const DEEP_OCEAN_MAX_Y: i32 = 46;
pub(super) const OCEAN_MAX_Y: i32 = 61;
pub(super) const BEACH_MAX_Y: i32 = 64;
pub(super) const BEACH_WEIRDNESS_MIN: f32 = -0.05;
pub(super) const BEACH_TEMP_MIN: f32 = 0.30;
pub(super) const MOUNTAIN_MIN_Y: i32 = 100;
pub(super) const FOOTHILLS_MIN_Y: i32 = 88;
pub(super) const SNOWY_PEAK_TEMP_MAX: f32 = 0.30;
pub(super) const WETLAND_MAX_ABOVE_SEA: i32 = 6;
pub(super) const WETLAND_HUMIDITY_MIN: f32 = 0.60;
pub(super) const SWAMP_HUMIDITY_MIN: f32 = 0.74;
pub(super) const COLD_TEMP_MAX: f32 = 0.30;
pub(super) const HOT_TEMP_MIN: f32 = 0.70;
pub(super) const HUMID_HUMIDITY_MIN: f32 = 0.58;
pub(super) const MESIC_HUMIDITY_MIN: f32 = 0.40;
pub(super) const TAIGA_TEMP_MAX: f32 = 0.38;
pub(super) const BIRCH_TEMP_MIN: f32 = 0.62;
pub(super) const TEMPERATE_DRY_DEFAULT: Biome = Biome::Plains;

pub(super) const COLD_LOWLAND_BANDS: &[HumidityBand] = &[
    HumidityBand {
        max_humidity: 0.42,
        biome: Biome::SnowyTundra,
    },
    HumidityBand {
        max_humidity: f32::INFINITY,
        biome: Biome::SnowyTaiga,
    },
];

pub(super) const HOT_LOWLAND_BANDS: &[HumidityBand] = &[
    HumidityBand {
        max_humidity: 0.32,
        biome: Biome::Desert,
    },
    HumidityBand {
        max_humidity: 0.55,
        biome: Biome::Savanna,
    },
    HumidityBand {
        max_humidity: f32::INFINITY,
        biome: Biome::Forest,
    },
];

pub(super) const BIOME_DEFS: &[BiomeDef] = &[
    BiomeDef {
        biome: Biome::Ocean,
        name: "ocean",
        fog_color: [0.38, 0.58, 0.94],
        grass_color: [0.50, 0.72, 0.38],
        foliage_color: [0.46, 0.68, 0.34],
        water_color: [0.12, 0.40, 0.86],
    },
    BiomeDef {
        biome: Biome::Beach,
        name: "beach",
        fog_color: [0.97, 0.90, 0.72],
        grass_color: [0.70, 0.78, 0.38],
        foliage_color: [0.64, 0.74, 0.34],
        water_color: [0.14, 0.48, 0.88],
    },
    BiomeDef {
        biome: Biome::River,
        name: "river",
        fog_color: [0.56, 0.74, 0.95],
        grass_color: [0.48, 0.76, 0.36],
        foliage_color: [0.42, 0.72, 0.32],
        water_color: [0.12, 0.46, 0.84],
    },
    BiomeDef {
        biome: Biome::Desert,
        name: "desert",
        fog_color: [0.98, 0.88, 0.64],
        grass_color: [0.84, 0.76, 0.30],
        foliage_color: [0.78, 0.70, 0.26],
        water_color: [0.14, 0.50, 0.86],
    },
    BiomeDef {
        biome: Biome::Plains,
        name: "plains",
        fog_color: [0.60, 0.82, 1.00],
        grass_color: [0.48, 0.80, 0.28],
        foliage_color: [0.42, 0.76, 0.24],
        water_color: [0.12, 0.46, 0.86],
    },
    BiomeDef {
        biome: Biome::Savanna,
        name: "savanna",
        fog_color: [0.66, 0.82, 0.98],
        grass_color: [0.72, 0.74, 0.26],
        foliage_color: [0.66, 0.68, 0.24],
        water_color: [0.16, 0.48, 0.82],
    },
    BiomeDef {
        biome: Biome::Forest,
        name: "forest",
        fog_color: [0.48, 0.76, 1.00],
        grass_color: [0.32, 0.78, 0.22],
        foliage_color: [0.24, 0.72, 0.18],
        water_color: [0.10, 0.42, 0.82],
    },
    BiomeDef {
        biome: Biome::BirchForest,
        name: "birch_forest",
        fog_color: [0.52, 0.78, 1.00],
        grass_color: [0.50, 0.82, 0.32],
        foliage_color: [0.48, 0.84, 0.30],
        water_color: [0.12, 0.46, 0.82],
    },
    BiomeDef {
        biome: Biome::Swamp,
        name: "swamp",
        fog_color: [0.54, 0.66, 0.70],
        grass_color: [0.26, 0.56, 0.22],
        foliage_color: [0.22, 0.52, 0.18],
        water_color: [0.16, 0.38, 0.48],
    },
    BiomeDef {
        biome: Biome::Taiga,
        name: "taiga",
        fog_color: [0.62, 0.72, 0.82],
        grass_color: [0.40, 0.68, 0.36],
        foliage_color: [0.34, 0.66, 0.32],
        water_color: [0.12, 0.42, 0.74],
    },
    BiomeDef {
        biome: Biome::SnowyTundra,
        name: "snowy_tundra",
        fog_color: [0.78, 0.82, 0.88],
        grass_color: [0.64, 0.76, 0.56],
        foliage_color: [0.58, 0.74, 0.54],
        water_color: [0.18, 0.50, 0.82],
    },
    BiomeDef {
        biome: Biome::SnowyTaiga,
        name: "snowy_taiga",
        fog_color: [0.74, 0.80, 0.86],
        grass_color: [0.48, 0.72, 0.46],
        foliage_color: [0.42, 0.70, 0.42],
        water_color: [0.16, 0.48, 0.78],
    },
    BiomeDef {
        biome: Biome::Mountains,
        name: "mountains",
        fog_color: [0.66, 0.74, 0.82],
        grass_color: [0.48, 0.70, 0.38],
        foliage_color: [0.42, 0.66, 0.34],
        water_color: [0.12, 0.46, 0.80],
    },
    BiomeDef {
        biome: Biome::SnowyPeaks,
        name: "snowy_peaks",
        fog_color: [0.80, 0.84, 0.88],
        grass_color: [0.80, 0.88, 0.80],
        foliage_color: [0.72, 0.84, 0.70],
        water_color: [0.20, 0.54, 0.84],
    },
    BiomeDef {
        biome: Biome::DeepOcean,
        name: "deep_ocean",
        fog_color: [0.22, 0.38, 0.76],
        grass_color: [0.46, 0.68, 0.36],
        foliage_color: [0.42, 0.64, 0.32],
        water_color: [0.04, 0.22, 0.64],
    },
    BiomeDef {
        biome: Biome::Foothills,
        name: "foothills",
        fog_color: [0.62, 0.76, 0.90],
        grass_color: [0.50, 0.72, 0.38],
        foliage_color: [0.44, 0.68, 0.34],
        water_color: [0.12, 0.46, 0.82],
    },
    BiomeDef {
        biome: Biome::Wetland,
        name: "wetland",
        fog_color: [0.58, 0.70, 0.74],
        grass_color: [0.30, 0.62, 0.24],
        foliage_color: [0.26, 0.58, 0.20],
        water_color: [0.14, 0.42, 0.58],
    },
];

#[inline]
pub(super) fn from_id(id: u8) -> Biome {
    BIOME_DEFS
        .get(id as usize)
        .map_or(Biome::Ocean, |def| def.biome)
}

#[inline]
pub(super) fn def(biome: Biome) -> &'static BiomeDef {
    let index = biome.id() as usize;
    debug_assert!(
        index < BIOME_DEFS.len() && BIOME_DEFS[index].biome == biome,
        "BIOME_DEFS must be ordered by Biome::id()"
    );
    &BIOME_DEFS[index]
}

#[inline]
pub(super) fn select_humidity_band(bands: &[HumidityBand], humidity: f32) -> Biome {
    for band in bands {
        if humidity < band.max_humidity {
            return band.biome;
        }
    }

    debug_assert!(!bands.is_empty(), "humidity band table must not be empty");
    bands
        .last()
        .map_or(TEMPERATE_DRY_DEFAULT, |band| band.biome)
}
