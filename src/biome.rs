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

/// Radius, in blocks, used when blending the above-water sky/fog colour across
/// neighbouring biome columns.
pub const SKY_FOG_BLEND_SPAN_BLOCKS: i32 = 10;

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

/// Blend above-water sky/fog colour from nearby biome columns.
///
/// The sample kernel fades smoothly to zero at `SKY_FOG_BLEND_SPAN_BLOCKS`, so a
/// single border crossfades gradually and multi-biome intersections naturally
/// become a weighted mix of every biome near the camera.
pub fn blended_fog_color(
    x: f32,
    z: f32,
    mut biome_at_column: impl FnMut(i32, i32) -> Biome,
) -> [f32; 3] {
    let center_x = x.floor() as i32;
    let center_z = z.floor() as i32;
    let radius_i = SKY_FOG_BLEND_SPAN_BLOCKS;
    let radius = SKY_FOG_BLEND_SPAN_BLOCKS as f32;
    let radius2 = radius * radius;

    let mut sum = [0.0f32; 3];
    let mut total = 0.0f32;

    for wz in center_z - radius_i..=center_z + radius_i {
        for wx in center_x - radius_i..=center_x + radius_i {
            let dx = wx as f32 + 0.5 - x;
            let dz = wz as f32 + 0.5 - z;
            let dist2 = dx * dx + dz * dz;
            if dist2 > radius2 {
                continue;
            }

            let t = 1.0 - (dist2.sqrt() / radius);
            let weight = t * t * (3.0 - 2.0 * t);
            if weight <= 0.0 {
                continue;
            }

            let color = biome_at_column(wx, wz).fog_color();
            sum[0] += color[0] * weight;
            sum[1] += color[1] * weight;
            sum[2] += color[2] * weight;
            total += weight;
        }
    }

    if total > 0.0 {
        [sum[0] / total, sum[1] / total, sum[2] / total]
    } else {
        biome_at_column(center_x, center_z).fog_color()
    }
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
    use super::{biome_at, blended_fog_color, data, Biome, Climate, SKY_FOG_BLEND_SPAN_BLOCKS};

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

    fn assert_color_close(actual: [f32; 3], expected: [f32; 3]) {
        for i in 0..3 {
            assert!(
                (actual[i] - expected[i]).abs() < 1e-5,
                "channel {i}: got {}, expected {}",
                actual[i],
                expected[i]
            );
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
        assert_eq!(Biome::Beach.fog_color(), [0.97, 0.90, 0.72]);
        assert_eq!(Biome::Forest.grass_color(), [0.32, 0.78, 0.22]);
        assert_eq!(Biome::Swamp.water_color(), [0.16, 0.38, 0.48]);
        assert_eq!(Biome::SnowyPeaks.foliage_color(), [0.72, 0.84, 0.70]);
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

    #[test]
    fn blended_fog_color_is_exact_in_uniform_biome_area() {
        assert_color_close(
            blended_fog_color(12.25, -4.75, |_, _| Biome::Forest),
            Biome::Forest.fog_color(),
        );
    }

    #[test]
    fn blended_fog_color_uses_ten_block_border_window() {
        let boundary = |wx: i32, _wz: i32| {
            if wx < 0 {
                Biome::Plains
            } else {
                Biome::Desert
            }
        };

        assert_color_close(
            blended_fog_color(-(SKY_FOG_BLEND_SPAN_BLOCKS as f32) - 1.0, 0.5, boundary),
            Biome::Plains.fog_color(),
        );
        assert_color_close(
            blended_fog_color(SKY_FOG_BLEND_SPAN_BLOCKS as f32 + 1.0, 0.5, boundary),
            Biome::Desert.fog_color(),
        );

        let midpoint = blended_fog_color(0.0, 0.5, boundary);
        let plains = Biome::Plains.fog_color();
        let desert = Biome::Desert.fog_color();
        assert_color_close(
            midpoint,
            [
                (plains[0] + desert[0]) * 0.5,
                (plains[1] + desert[1]) * 0.5,
                (plains[2] + desert[2]) * 0.5,
            ],
        );
    }

    #[test]
    fn blended_fog_color_handles_multi_biome_intersections() {
        let quadrant = |wx: i32, wz: i32| match (wx >= 0, wz >= 0) {
            (false, false) => Biome::Plains,
            (true, false) => Biome::Desert,
            (false, true) => Biome::Swamp,
            (true, true) => Biome::SnowyTundra,
        };
        let actual = blended_fog_color(0.0, 0.0, quadrant);
        let colors = [
            Biome::Plains.fog_color(),
            Biome::Desert.fog_color(),
            Biome::Swamp.fog_color(),
            Biome::SnowyTundra.fog_color(),
        ];
        let expected = [
            colors.iter().map(|c| c[0]).sum::<f32>() * 0.25,
            colors.iter().map(|c| c[1]).sum::<f32>() * 0.25,
            colors.iter().map(|c| c[2]).sum::<f32>() * 0.25,
        ];

        assert_color_close(actual, expected);
    }
}
