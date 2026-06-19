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
        fog_color: [0.30, 0.45, 0.85],
        grass_color: [0.48, 0.68, 0.40],
        foliage_color: [0.44, 0.64, 0.36],
        water_color: [0.16, 0.34, 0.74],
    },
    BiomeDef {
        biome: Biome::Beach,
        name: "beach",
        fog_color: [0.93, 0.88, 0.70],
        grass_color: [0.66, 0.72, 0.42],
        foliage_color: [0.60, 0.68, 0.38],
        water_color: [0.20, 0.40, 0.74],
    },
    BiomeDef {
        biome: Biome::River,
        name: "river",
        fog_color: [0.55, 0.66, 0.78],
        grass_color: [0.48, 0.70, 0.42],
        foliage_color: [0.44, 0.66, 0.38],
        water_color: [0.20, 0.42, 0.66],
    },
    BiomeDef {
        biome: Biome::Desert,
        name: "desert",
        fog_color: [0.93, 0.88, 0.70],
        grass_color: [0.80, 0.72, 0.34],
        foliage_color: [0.74, 0.66, 0.30],
        water_color: [0.24, 0.44, 0.72],
    },
    BiomeDef {
        biome: Biome::Plains,
        name: "plains",
        fog_color: [0.62, 0.78, 0.95],
        grass_color: [0.50, 0.73, 0.34],
        foliage_color: [0.46, 0.70, 0.30],
        water_color: [0.20, 0.40, 0.74],
    },
    BiomeDef {
        biome: Biome::Savanna,
        name: "savanna",
        fog_color: [0.62, 0.78, 0.95],
        grass_color: [0.69, 0.69, 0.31],
        foliage_color: [0.62, 0.62, 0.28],
        water_color: [0.26, 0.44, 0.68],
    },
    BiomeDef {
        biome: Biome::Forest,
        name: "forest",
        fog_color: [0.62, 0.78, 0.95],
        grass_color: [0.40, 0.66, 0.30],
        foliage_color: [0.34, 0.60, 0.24],
        water_color: [0.18, 0.36, 0.66],
    },
    BiomeDef {
        biome: Biome::BirchForest,
        name: "birch_forest",
        fog_color: [0.62, 0.78, 0.95],
        grass_color: [0.56, 0.72, 0.40],
        foliage_color: [0.58, 0.74, 0.40],
        water_color: [0.22, 0.42, 0.64],
    },
    BiomeDef {
        biome: Biome::Swamp,
        name: "swamp",
        fog_color: [0.44, 0.54, 0.58],
        grass_color: [0.30, 0.44, 0.24],
        foliage_color: [0.26, 0.40, 0.20],
        water_color: [0.24, 0.36, 0.30],
    },
    BiomeDef {
        biome: Biome::Taiga,
        name: "taiga",
        fog_color: [0.62, 0.78, 0.95],
        grass_color: [0.44, 0.60, 0.40],
        foliage_color: [0.40, 0.58, 0.36],
        water_color: [0.20, 0.38, 0.56],
    },
    BiomeDef {
        biome: Biome::SnowyTundra,
        name: "snowy_tundra",
        fog_color: [0.85, 0.90, 0.98],
        grass_color: [0.62, 0.72, 0.58],
        foliage_color: [0.58, 0.70, 0.56],
        water_color: [0.30, 0.46, 0.66],
    },
    BiomeDef {
        biome: Biome::SnowyTaiga,
        name: "snowy_taiga",
        fog_color: [0.85, 0.90, 0.98],
        grass_color: [0.52, 0.66, 0.50],
        foliage_color: [0.48, 0.64, 0.48],
        water_color: [0.28, 0.44, 0.60],
    },
    BiomeDef {
        biome: Biome::Mountains,
        name: "mountains",
        fog_color: [0.65, 0.77, 0.92],
        grass_color: [0.50, 0.62, 0.42],
        foliage_color: [0.46, 0.58, 0.38],
        water_color: [0.22, 0.42, 0.64],
    },
    BiomeDef {
        biome: Biome::SnowyPeaks,
        name: "snowy_peaks",
        fog_color: [0.85, 0.90, 0.98],
        grass_color: [0.80, 0.86, 0.82],
        foliage_color: [0.74, 0.82, 0.74],
        water_color: [0.34, 0.50, 0.68],
    },
    BiomeDef {
        biome: Biome::DeepOcean,
        name: "deep_ocean",
        fog_color: [0.16, 0.28, 0.62],
        grass_color: [0.44, 0.64, 0.38],
        foliage_color: [0.40, 0.60, 0.34],
        water_color: [0.07, 0.18, 0.50],
    },
    BiomeDef {
        biome: Biome::Foothills,
        name: "foothills",
        fog_color: [0.65, 0.77, 0.92],
        grass_color: [0.52, 0.64, 0.44],
        foliage_color: [0.48, 0.60, 0.40],
        water_color: [0.20, 0.40, 0.66],
    },
    BiomeDef {
        biome: Biome::Wetland,
        name: "wetland",
        fog_color: [0.50, 0.60, 0.62],
        grass_color: [0.34, 0.52, 0.28],
        foliage_color: [0.30, 0.48, 0.24],
        water_color: [0.26, 0.40, 0.40],
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
