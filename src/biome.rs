//! Biome definitions + selection from climate (6 parameters).

mod data;
mod definition;

#[repr(u8)]
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Biome {
    Ocean,
    Beach,
    River,
    Desert,
    Plains,
    Savanna,
    Forest,
    BirchForest,
    Swamp,
    Taiga,
    SnowyTundra,
    SnowyTaiga,
    Mountains,
    SnowyPeaks,
    DeepOcean,
    Foothills,
    Wetland,
}

/// 6-parameter climate sample, each in [-1, 1].
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Climate {
    pub temperature: f32,
    pub humidity: f32,
    pub continentalness: f32,
    pub erosion: f32,
    pub weirdness: f32,
    pub depth: f32,
}

impl Climate {
    /// Helper convenience: temperature 0..1.
    pub fn temp01(self) -> f32 {
        (self.temperature * 0.5 + 0.5).clamp(0.0, 1.0)
    }
    pub fn humid01(self) -> f32 {
        (self.humidity * 0.5 + 0.5).clamp(0.0, 1.0)
    }
    pub fn cont01(self) -> f32 {
        (self.continentalness * 0.5 + 0.5).clamp(0.0, 1.0)
    }
    pub fn erode01(self) -> f32 {
        (self.erosion * 0.5 + 0.5).clamp(0.0, 1.0)
    }
}

/// Pick biome from climate + surface height. Ordered cascade: oceans (depth-led),
/// shore, high-altitude (foothills/mountains), then a temperature×humidity grid
/// for the lowlands. Transition bands (Beach/Foothills/Wetland) are jittered by
/// `weirdness` so they appear as ragged intermittent patches, never closed rings.
pub fn biome_at(c: Climate, surf_y: i32) -> Biome {
    use Biome::*;
    let sea = crate::chunk::SEA_LEVEL; // 64
    let t = c.temp01();
    let h = c.humid01();
    // Edge dither: weirdness is an independent fbm, so the band edges wander.
    let ey = (c.weirdness * 14.0) as i32; // ~±3 block altitude jitter

    // ---- Oceans (depth-led; floors sit well below sea level) ----
    if surf_y <= data::DEEP_OCEAN_MAX_Y + ey / 2 {
        return DeepOcean;
    }
    if surf_y <= data::OCEAN_MAX_Y + ey / 2 {
        return Ocean;
    }

    // ---- Beach: a thin shore slab, but only on scattered stretches — gated on an
    // independent noise so it does NOT form a closed ring around every coast.
    // Where it doesn't form, the coast falls through to grass / wetland down to the
    // waterline (varied shores). Cold shores stay non-sandy.
    if surf_y <= data::BEACH_MAX_Y + ey
        && c.weirdness > data::BEACH_WEIRDNESS_MIN
        && t > data::BEACH_TEMP_MIN
    {
        return Beach;
    }

    // ---- High altitude: mountains + their foothill transition ----
    if surf_y > data::MOUNTAIN_MIN_Y + ey {
        return if t < data::SNOWY_PEAK_TEMP_MAX {
            SnowyPeaks
        } else {
            Mountains
        };
    }
    if surf_y > data::FOOTHILLS_MIN_Y + ey {
        return Foothills;
    }

    // ---- Wetland / Swamp: humid low land near the waterline ----
    if surf_y <= sea + data::WETLAND_MAX_ABOVE_SEA + ey && h > data::WETLAND_HUMIDITY_MIN {
        if h > data::SWAMP_HUMIDITY_MIN {
            return Swamp;
        }
        return Wetland;
    }

    // ---- Lowland temperature × humidity grid ----
    if t < data::COLD_TEMP_MAX {
        return data::select_humidity_band(data::COLD_LOWLAND_BANDS, h);
    }
    if t > data::HOT_TEMP_MIN {
        return data::select_humidity_band(data::HOT_LOWLAND_BANDS, h);
    }
    if h > data::HUMID_HUMIDITY_MIN {
        if t < data::TAIGA_TEMP_MAX {
            return Taiga;
        }
        return Forest;
    }
    if h > data::MESIC_HUMIDITY_MIN {
        if t < data::TAIGA_TEMP_MAX {
            return Taiga;
        }
        return if t > data::BIRCH_TEMP_MIN {
            BirchForest
        } else {
            Forest
        };
    }
    data::TEMPERATE_DRY_DEFAULT
}

impl Biome {
    #[inline]
    pub fn fog_color(self) -> [f32; 3] {
        self.def().fog_color
    }

    #[inline]
    pub fn name(self) -> &'static str {
        self.def().name
    }

    #[inline]
    pub fn from_id(id: u8) -> Biome {
        data::from_id(id)
    }

    #[inline]
    pub fn id(self) -> u8 {
        self as u8
    }

    /// Grass-block top tint colour (linear sRGB 0..1) for biome. Forest/Plains are
    /// a normal saturated green; Foothills/Mountains are desaturated (R≈G); Desert
    /// is a deadish yellow, Savanna a yellow-green, Wetland dark green, Swamp darker.
    #[inline]
    pub fn grass_color(self) -> [f32; 3] {
        self.def().grass_color
    }

    /// Foliage tint (leaves) for biome.
    #[inline]
    pub fn foliage_color(self) -> [f32; 3] {
        self.def().foliage_color
    }

    /// Water tint for biome. Ocean is a normal blue, DeepOcean a much darker blue,
    /// Swamp/Wetland a murky green-blue.
    #[inline]
    pub fn water_color(self) -> [f32; 3] {
        self.def().water_color
    }

    #[inline]
    fn def(self) -> &'static definition::BiomeDef {
        data::def(self)
    }
}

#[cfg(test)]
mod tests {
    use super::{biome_at, data, Biome, Climate};

    const EXPECTED_BIOMES: [Biome; 17] = [
        Biome::Ocean,
        Biome::Beach,
        Biome::River,
        Biome::Desert,
        Biome::Plains,
        Biome::Savanna,
        Biome::Forest,
        Biome::BirchForest,
        Biome::Swamp,
        Biome::Taiga,
        Biome::SnowyTundra,
        Biome::SnowyTaiga,
        Biome::Mountains,
        Biome::SnowyPeaks,
        Biome::DeepOcean,
        Biome::Foothills,
        Biome::Wetland,
    ];

    fn climate(temp01: f32, humid01: f32, weirdness: f32) -> Climate {
        Climate {
            temperature: temp01 * 2.0 - 1.0,
            humidity: humid01 * 2.0 - 1.0,
            continentalness: 0.0,
            erosion: 0.0,
            weirdness,
            depth: 0.0,
        }
    }

    #[test]
    fn ids_are_stable_and_append_only() {
        for (id, biome) in EXPECTED_BIOMES.into_iter().enumerate() {
            assert_eq!(biome.id(), id as u8);
            assert_eq!(Biome::from_id(id as u8), biome);
        }
        assert_eq!(Biome::from_id(u8::MAX), Biome::Ocean);
    }

    #[test]
    fn definitions_are_id_ordered() {
        assert_eq!(data::BIOME_DEFS.len(), EXPECTED_BIOMES.len());
        for def in data::BIOME_DEFS {
            assert_eq!(Biome::from_id(def.biome.id()), def.biome);
            assert_eq!(data::BIOME_DEFS[def.biome.id() as usize].biome, def.biome);
        }
    }

    #[test]
    fn metadata_methods_read_definition_rows() {
        for def in data::BIOME_DEFS {
            assert_eq!(def.biome.name(), def.name);
            assert_eq!(def.biome.fog_color(), def.fog_color);
            assert_eq!(def.biome.grass_color(), def.grass_color);
            assert_eq!(def.biome.foliage_color(), def.foliage_color);
            assert_eq!(def.biome.water_color(), def.water_color);
        }

        assert_eq!(Biome::Beach.name(), "beach");
        assert_eq!(Biome::Beach.fog_color(), [0.93, 0.88, 0.70]);
        assert_eq!(Biome::Forest.grass_color(), [0.40, 0.66, 0.30]);
        assert_eq!(Biome::Swamp.water_color(), [0.24, 0.36, 0.30]);
        assert_eq!(Biome::SnowyPeaks.foliage_color(), [0.74, 0.82, 0.74]);
    }

    #[test]
    fn biome_at_preserves_ordered_selection_behavior() {
        assert_eq!(biome_at(climate(0.50, 0.50, 0.0), 46), Biome::DeepOcean);
        assert_eq!(biome_at(climate(0.50, 0.50, 0.0), 61), Biome::Ocean);
        assert_eq!(biome_at(climate(0.50, 0.50, 0.0), 64), Biome::Beach);
        assert_eq!(biome_at(climate(0.20, 0.50, 0.0), 101), Biome::SnowyPeaks);
        assert_eq!(biome_at(climate(0.50, 0.50, 0.0), 101), Biome::Mountains);
        assert_eq!(biome_at(climate(0.50, 0.50, 0.0), 89), Biome::Foothills);
        assert_eq!(biome_at(climate(0.50, 0.75, 0.0), 70), Biome::Swamp);
        assert_eq!(biome_at(climate(0.50, 0.61, 0.0), 70), Biome::Wetland);

        assert_eq!(biome_at(climate(0.20, 0.30, 0.0), 71), Biome::SnowyTundra);
        assert_eq!(biome_at(climate(0.20, 0.50, 0.0), 71), Biome::SnowyTaiga);
        assert_eq!(biome_at(climate(0.80, 0.20, 0.0), 71), Biome::Desert);
        assert_eq!(biome_at(climate(0.80, 0.40, 0.0), 71), Biome::Savanna);
        assert_eq!(biome_at(climate(0.80, 0.60, 0.0), 71), Biome::Forest);
        assert_eq!(biome_at(climate(0.35, 0.60, 0.0), 71), Biome::Taiga);
        assert_eq!(biome_at(climate(0.50, 0.60, 0.0), 71), Biome::Forest);
        assert_eq!(biome_at(climate(0.35, 0.50, 0.0), 71), Biome::Taiga);
        assert_eq!(biome_at(climate(0.65, 0.50, 0.0), 71), Biome::BirchForest);
        assert_eq!(biome_at(climate(0.50, 0.30, 0.0), 71), Biome::Plains);
    }
}
