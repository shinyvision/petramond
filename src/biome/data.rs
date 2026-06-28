use crate::registry::{self, RegistryKey, TableEntry};

use super::definition::BiomeDef;
use super::Biome;

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
        grass_color: [0.32, 0.78, 0.22],
        foliage_color: [0.24, 0.72, 0.18],
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
        grass_color: [0.12, 0.58, 0.02],
        foliage_color: [0.24, 0.72, 0.18],
        water_color: [0.10, 0.42, 0.82],
    },
    BiomeDef {
        biome: Biome::BirchForest,
        name: "birch_forest",
        fog_color: [0.52, 0.78, 1.00],
        grass_color: [0.12, 0.58, 0.02],
        foliage_color: [0.24, 0.72, 0.18],
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
        grass_color: [0.12, 0.58, 0.02],
        foliage_color: [0.24, 0.72, 0.18],
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
    BiomeDef {
        biome: Biome::Jungle,
        name: "jungle",
        fog_color: [0.46, 0.74, 0.62],
        grass_color: [0.22, 0.74, 0.10],
        foliage_color: [0.18, 0.66, 0.06],
        water_color: [0.14, 0.52, 0.74],
    },
    BiomeDef {
        biome: Biome::Badlands,
        name: "badlands",
        fog_color: [0.86, 0.66, 0.45],
        grass_color: [0.62, 0.50, 0.18],
        foliage_color: [0.58, 0.46, 0.16],
        water_color: [0.14, 0.46, 0.74],
    },
    BiomeDef {
        biome: Biome::DarkForest,
        name: "dark_forest",
        fog_color: [0.42, 0.58, 0.62],
        grass_color: [0.10, 0.50, 0.12],
        foliage_color: [0.24, 0.72, 0.18],
        water_color: [0.10, 0.40, 0.66],
    },
    BiomeDef {
        biome: Biome::OldGrowthTaiga,
        name: "old_growth_taiga",
        fog_color: [0.56, 0.66, 0.66],
        grass_color: [0.34, 0.58, 0.30],
        foliage_color: [0.30, 0.54, 0.28],
        water_color: [0.12, 0.42, 0.66],
    },
    BiomeDef {
        biome: Biome::CherryGrove,
        name: "cherry_grove",
        fog_color: [0.92, 0.80, 0.86],
        grass_color: [0.42, 0.78, 0.40],
        foliage_color: [0.52, 0.80, 0.46],
        water_color: [0.20, 0.60, 0.86],
    },
    BiomeDef {
        biome: Biome::Meadow,
        name: "meadow",
        fog_color: [0.66, 0.84, 0.96],
        grass_color: [0.36, 0.80, 0.30],
        foliage_color: [0.32, 0.74, 0.26],
        water_color: [0.16, 0.52, 0.86],
    },
    BiomeDef {
        biome: Biome::Grove,
        name: "grove",
        fog_color: [0.78, 0.84, 0.90],
        grass_color: [0.50, 0.66, 0.50],
        foliage_color: [0.42, 0.62, 0.44],
        water_color: [0.16, 0.48, 0.80],
    },
    BiomeDef {
        biome: Biome::SnowySlopes,
        name: "snowy_slopes",
        fog_color: [0.82, 0.86, 0.92],
        grass_color: [0.70, 0.80, 0.74],
        foliage_color: [0.64, 0.78, 0.70],
        water_color: [0.18, 0.50, 0.82],
    },
    BiomeDef {
        biome: Biome::IceSpikes,
        name: "ice_spikes",
        fog_color: [0.80, 0.88, 0.94],
        grass_color: [0.70, 0.82, 0.82],
        foliage_color: [0.64, 0.80, 0.80],
        water_color: [0.22, 0.56, 0.86],
    },
    BiomeDef {
        biome: Biome::MushroomFields,
        name: "mushroom_fields",
        fog_color: [0.66, 0.58, 0.66],
        grass_color: [0.56, 0.42, 0.54],
        foliage_color: [0.50, 0.38, 0.50],
        water_color: [0.14, 0.46, 0.74],
    },
    BiomeDef {
        biome: Biome::WindsweptHills,
        name: "windswept_hills",
        fog_color: [0.64, 0.74, 0.82],
        grass_color: [0.46, 0.64, 0.42],
        foliage_color: [0.40, 0.60, 0.38],
        water_color: [0.12, 0.46, 0.80],
    },
    BiomeDef {
        biome: Biome::StonyPeaks,
        name: "stony_peaks",
        fog_color: [0.74, 0.76, 0.78],
        grass_color: [0.56, 0.62, 0.54],
        foliage_color: [0.50, 0.58, 0.50],
        water_color: [0.16, 0.50, 0.82],
    },
];

impl RegistryKey for Biome {
    #[inline]
    fn to_id(self) -> u8 {
        self.id()
    }
}

impl TableEntry for BiomeDef {
    type Key = Biome;
    #[inline]
    fn key(&self) -> Biome {
        self.biome
    }
}

#[inline]
pub(super) fn from_id(id: u8) -> Biome {
    registry::from_id(BIOME_DEFS, id, Biome::Ocean)
}

#[inline]
pub(super) fn def(biome: Biome) -> &'static BiomeDef {
    registry::def(BIOME_DEFS, biome)
}
