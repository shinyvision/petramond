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
    // --- appended (ids 17+): keep append-only; never reorder (biome ids are
    // serialized into chunk bytes). ---
    Jungle,
    Badlands,
    DarkForest,
    OldGrowthTaiga,
    CherryGrove,
    Meadow,
    Grove,
    SnowySlopes,
    IceSpikes,
    MushroomFields,
    WindsweptHills,
    StonyPeaks,
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
    pub fn depth01(self) -> f32 {
        (self.depth * 0.5 + 0.5).clamp(0.0, 1.0)
    }
}

/// Broad landform weights shared by biome selection and terrain shaping.
///
/// They deliberately come from climate-space parameters, not final surface
/// height, so lowland biomes can become hilly without being relabelled as
/// mountains by a local height spike.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct LandformWeights {
    pub mountain: f32,
    pub foothill: f32,
    pub rolling: f32,
    pub plateau: f32,
    pub wet_basin: f32,
}

pub(crate) fn landform_weights(c: Climate) -> LandformWeights {
    let expanded01 = |v: f32, scale: f32| (v * scale).clamp(-1.0, 1.0) * 0.5 + 0.5;
    let cont = expanded01(c.continentalness, 2.8);
    let temp = c.temp01();
    let humid = c.humid01();
    let erode = expanded01(c.erosion, 1.8);
    let rugged = 1.0 - erode;
    let depth = expanded01(c.depth, 3.0);
    let weird = c.weirdness.abs().clamp(0.0, 1.0);

    let inland = crate::mathh::smoothstep(0.46, 0.64, cont);
    let uplift = crate::mathh::smoothstep(0.50, 0.80, depth);
    let basin = crate::mathh::smoothstep(0.58, 0.86, 1.0 - depth);
    let arid = crate::mathh::smoothstep(0.58, 0.82, temp)
        * (1.0 - crate::mathh::smoothstep(0.36, 0.64, humid));
    let wet = crate::mathh::smoothstep(0.60, 0.86, humid);
    let smooth_lowland = crate::mathh::smoothstep(0.38, 0.76, erode);

    let mountain_score = uplift * 0.54 + rugged * 0.68 + weird * 0.10;
    let mountain =
        inland * crate::mathh::smoothstep(0.44, 0.78, mountain_score) * (1.0 - 0.55 * arid);
    let foothill_score = uplift * 0.46 + rugged * 0.40 + cont * 0.12;
    let foothill =
        inland * crate::mathh::smoothstep(0.34, 0.66, foothill_score) * (1.0 - 0.75 * mountain);
    let plateau = inland * arid * (0.25 + 0.75 * uplift) * (1.0 - 0.45 * mountain);
    let wet_basin = inland * wet * basin * smooth_lowland * (1.0 - 0.35 * arid);
    let rolling =
        inland * (0.35 + 0.65 * rugged) * (1.0 - 0.55 * mountain) * (1.0 - 0.30 * wet_basin);

    LandformWeights {
        mountain: mountain.clamp(0.0, 1.0),
        foothill: foothill.clamp(0.0, 1.0),
        rolling: rolling.clamp(0.0, 1.0),
        plateau: plateau.clamp(0.0, 1.0),
        wet_basin: wet_basin.clamp(0.0, 1.0),
    }
}

fn lowland_biome(c: Climate) -> Biome {
    let base_t = c.temp01();
    let base_h = c.humid01();
    let detail = c.weirdness.clamp(-1.0, 1.0);
    let local_t = (base_t + detail * 0.13 + c.depth * 0.040).clamp(0.0, 1.0);
    let local_h = (base_h - detail * 0.22 + c.erosion * 0.070).clamp(0.0, 1.0);

    // Preserve the large, readable hot/dry cores as ONE connected desert province.
    // Badlands only emerge on a rare extreme-weirdness streak (≈3% of the hot/dry
    // corner), so the desert stays coherent instead of being shot through with
    // badlands patches.
    if base_t > data::HOT_TEMP_MIN + 0.08 && base_h < 0.30 {
        return if c.weirdness > 0.32 {
            Biome::Badlands
        } else {
            Biome::Desert
        };
    }

    if local_t < data::COLD_TEMP_MAX {
        // Very cold + very dry + a rare extreme-weirdness streak: ice spikes (kept
        // rare so the snowy tundra stays a connected province).
        if local_h < 0.26 && c.weirdness > 0.32 {
            return Biome::IceSpikes;
        }
        if local_h < 0.42 {
            return if detail < -0.12 || (detail < 0.06 && local_h > 0.12) {
                Biome::SnowyTaiga
            } else {
                Biome::SnowyTundra
            };
        }
        if detail > -0.12 {
            return Biome::SnowyTundra;
        }
        return Biome::SnowyTaiga;
    }
    if local_t > data::HOT_TEMP_MIN {
        return data::select_humidity_band(data::HOT_LOWLAND_BANDS, local_h);
    }
    if local_h > data::HUMID_HUMIDITY_MIN {
        if local_t < data::TAIGA_TEMP_MAX {
            // Cool + very humid + a calm-weirdness streak: old-growth taiga.
            return if local_h > 0.74 && detail < -0.14 {
                Biome::OldGrowthTaiga
            } else {
                Biome::Taiga
            };
        }
        // Temperate + very humid + a calm-weirdness streak: dark forest.
        if local_h > 0.76 && detail < -0.14 {
            return Biome::DarkForest;
        }
        if detail > 0.02 && local_h < 0.92 {
            return Biome::Plains;
        }
        return if detail < -0.02 && local_h < 0.95 {
            Biome::BirchForest
        } else {
            Biome::Forest
        };
    }
    if local_h > data::MESIC_HUMIDITY_MIN {
        if local_t < data::TAIGA_TEMP_MAX {
            return Biome::Taiga;
        }
        return if local_t > data::BIRCH_TEMP_MIN || detail < -0.10 {
            Biome::BirchForest
        } else {
            Biome::Forest
        };
    }
    data::TEMPERATE_DRY_DEFAULT
}

/// Pick biome from climate + surface height. Oceans and beaches still respect
/// the shoreline, but land biomes are selected from climate and broad landform
/// weights instead of final height bands. That lets plains, deserts, forests,
/// and savannas form their own hills/plateaus without abrupt mountain takeovers.
pub fn biome_at(c: Climate, surf_y: i32) -> Biome {
    use Biome::*;
    let sea = crate::chunk::SEA_LEVEL; // 64
    let t = c.temp01();
    let h = c.humid01();
    let cont = c.cont01();
    let land = landform_weights(c);
    // Edge dither: weirdness is an independent fbm, so the band edges wander.
    let ey = (c.weirdness * 14.0) as i32; // ~±3 block altitude jitter

    // ---- Oceans: continentalness-led so inland basins can be swamps/lakes,
    // not mislabeled oceans just because their floor is near sea level. ----
    let ocean_jitter = c.weirdness * 0.035;
    if cont <= data::DEEP_OCEAN_CONT_MAX + ocean_jitter && surf_y <= data::DEEP_OCEAN_MAX_Y + ey / 2
    {
        return DeepOcean;
    }
    if cont <= data::OCEAN_CONT_MAX + ocean_jitter && surf_y <= data::OCEAN_MAX_Y + ey / 2 {
        return Ocean;
    }

    // ---- Wetland / Swamp: humid low land near water OR an inland wet basin. ----
    let coastal_wet = surf_y <= sea + data::WETLAND_MAX_ABOVE_SEA + ey;
    let inland_wet = surf_y <= sea + data::INLAND_WETLAND_MAX_ABOVE_SEA + ey
        && land.wet_basin > data::WETLAND_BASIN_MIN;
    if h > data::WETLAND_HUMIDITY_MIN && (coastal_wet || inland_wet) {
        if h > data::SWAMP_HUMIDITY_MIN && (coastal_wet || land.wet_basin > data::SWAMP_BASIN_MIN) {
            return Swamp;
        }
        return Wetland;
    }

    // ---- Beach: a thin shore slab, but only on scattered stretches — gated on an
    // independent noise so it does NOT form a closed ring around every coast.
    // Where it doesn't form, the coast falls through to grass / wetland down to the
    // waterline (varied shores). Cold shores stay non-sandy.
    if surf_y <= data::BEACH_MAX_Y + ey
        && (data::BEACH_CONT_MIN..=data::BEACH_CONT_MAX).contains(&cont)
        && c.weirdness > data::BEACH_WEIRDNESS_MIN
        && t > data::BEACH_TEMP_MIN
    {
        return Beach;
    }

    // ---- Mushroom island: GENUINELY RARE. A small, warm island sitting right at
    // the ocean boundary (so it is isolated, not a patch carved out of mainland)
    // with an extreme-weirdness signature. The narrow continentalness band keeps it
    // to tiny near-shore islands; the high weirdness gate keeps it well under 0.1%. ----
    if (data::OCEAN_CONT_MAX..=data::OCEAN_CONT_MAX + 0.03).contains(&cont)
        && surf_y > sea
        && surf_y <= sea + 5
        && t > 0.5
        && c.weirdness > 0.37
    {
        return MushroomFields;
    }

    // Terrain is now tall wherever erosion is low (mountains are emergent from the
    // height field, not a climate landform), so mountain/foothill PROVINCES are
    // labelled by actual height + ruggedness (low erosion), NOT the old
    // climate-mountain weight. Smooth high ground (a high desert/forest plateau)
    // has low ruggedness, so it keeps its lowland biome — the no-relabel-by-height
    // invariant.
    let rugged = 1.0 - c.erode01();

    // ---- Mountain provinces: genuinely high AND rugged. ----
    if surf_y > data::MOUNTAIN_MIN_Y + ey && rugged > 0.50 {
        if t < data::SNOWY_PEAK_TEMP_MAX {
            return SnowyPeaks;
        }
        // Temperate summits above the snowline read as bare stony peaks.
        return if surf_y > 150 { StonyPeaks } else { Mountains };
    }

    // ---- Foothill province: cold slopes are snowy slopes / groves; rugged
    // temperate slopes are windswept; calm/odd-weirdness slopes pick up
    // cherry-grove and meadow flavour; the rest stay plain foothills. ----
    if surf_y > data::FOOTHILLS_MIN_Y + ey && rugged > 0.40 {
        if t < data::COLD_TEMP_MAX {
            return if h > data::MESIC_HUMIDITY_MIN {
                Grove
            } else {
                SnowySlopes
            };
        }
        if c.erode01() < 0.32 {
            return WindsweptHills;
        }
        if c.weirdness < -0.16 {
            return CherryGrove;
        }
        if c.weirdness > 0.14 {
            return Meadow;
        }
        return Foothills;
    }

    // ---- Lowland temperature × humidity grid ----
    lowland_biome(c)
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
    use super::{
        biome_at, blended_fog_color, data, landform_weights, Biome, Climate,
        SKY_FOG_BLEND_SPAN_BLOCKS,
    };

    const EXPECTED_BIOMES: [Biome; 29] = [
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
        Biome::Jungle,
        Biome::Badlands,
        Biome::DarkForest,
        Biome::OldGrowthTaiga,
        Biome::CherryGrove,
        Biome::Meadow,
        Biome::Grove,
        Biome::SnowySlopes,
        Biome::IceSpikes,
        Biome::MushroomFields,
        Biome::WindsweptHills,
        Biome::StonyPeaks,
    ];

    fn climate(temp01: f32, humid01: f32, weirdness: f32) -> Climate {
        climate_params(temp01, humid01, 0.70, 0.70, 0.45, weirdness)
    }

    fn climate_params(
        temp01: f32,
        humid01: f32,
        cont01: f32,
        erode01: f32,
        depth01: f32,
        weirdness: f32,
    ) -> Climate {
        Climate {
            temperature: temp01 * 2.0 - 1.0,
            humidity: humid01 * 2.0 - 1.0,
            continentalness: cont01 * 2.0 - 1.0,
            erosion: erode01 * 2.0 - 1.0,
            weirdness,
            depth: depth01 * 2.0 - 1.0,
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
        assert_eq!(
            biome_at(climate_params(0.50, 0.50, 0.20, 0.70, 0.45, 0.0), 46),
            Biome::DeepOcean
        );
        assert_eq!(
            biome_at(climate_params(0.50, 0.50, 0.40, 0.70, 0.45, 0.0), 61),
            Biome::Ocean
        );
        assert_eq!(
            biome_at(climate_params(0.50, 0.50, 0.50, 0.70, 0.45, 0.0), 64),
            Biome::Beach
        );
        assert_eq!(
            biome_at(climate_params(0.20, 0.50, 0.82, 0.10, 0.92, 0.2), 101),
            Biome::SnowyPeaks
        );
        assert_eq!(
            biome_at(climate_params(0.50, 0.50, 0.82, 0.10, 0.92, 0.2), 101),
            Biome::Mountains
        );
        assert_eq!(
            biome_at(climate_params(0.50, 0.50, 0.62, 0.55, 0.56, 0.0), 89),
            Biome::Foothills
        );
        assert_eq!(biome_at(climate(0.50, 0.75, 0.0), 70), Biome::Swamp);
        assert_eq!(biome_at(climate(0.50, 0.61, 0.0), 70), Biome::Wetland);

        assert_eq!(biome_at(climate(0.20, 0.30, 0.3), 71), Biome::SnowyTundra);
        assert_eq!(biome_at(climate(0.20, 0.50, -0.2), 71), Biome::SnowyTaiga);
        assert_eq!(biome_at(climate(0.80, 0.20, 0.0), 71), Biome::Desert);
        assert_eq!(biome_at(climate(0.80, 0.45, 0.0), 71), Biome::Savanna);
        assert_eq!(biome_at(climate(0.80, 0.60, 0.0), 71), Biome::Forest);
        assert_eq!(biome_at(climate(0.35, 0.60, 0.0), 71), Biome::Taiga);
        assert_eq!(biome_at(climate(0.50, 0.60, 0.0), 71), Biome::Forest);
        assert_eq!(biome_at(climate(0.35, 0.50, 0.0), 71), Biome::Taiga);
        assert_eq!(biome_at(climate(0.65, 0.50, 0.0), 71), Biome::BirchForest);
        assert_eq!(biome_at(climate(0.50, 0.30, 0.0), 71), Biome::Plains);
    }

    #[test]
    fn high_lowland_columns_keep_their_climate_biome_without_mountain_landform() {
        assert_eq!(
            biome_at(climate_params(0.82, 0.18, 0.78, 0.82, 0.45, 0.0), 112),
            Biome::Desert
        );
        assert_eq!(
            biome_at(climate_params(0.54, 0.66, 0.78, 0.82, 0.45, 0.0), 108),
            Biome::Forest
        );
        assert_eq!(
            biome_at(climate_params(0.82, 0.48, 0.78, 0.82, 0.45, 0.0), 104),
            Biome::Savanna
        );
    }

    #[test]
    fn wet_basin_landform_allows_inland_and_coastal_swamps() {
        let inland = climate_params(0.55, 0.88, 0.75, 0.78, 0.10, 0.0);
        assert!(
            landform_weights(inland).wet_basin > data::SWAMP_BASIN_MIN,
            "test climate should be a strong inland wet basin"
        );
        assert_eq!(biome_at(inland, 78), Biome::Swamp);

        let coast = climate_params(0.55, 0.86, 0.50, 0.78, 0.12, 0.0);
        assert_eq!(biome_at(coast, 66), Biome::Swamp);
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
