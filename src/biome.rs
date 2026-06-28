//! Biome definitions and per-biome metadata (names, ids, fog/grass/foliage/water colours).

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

/// Radius, in blocks, used when blending the above-water sky/fog colour across
/// neighbouring biome columns.
pub const SKY_FOG_BLEND_SPAN_BLOCKS: i32 = 10;

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

    /// Grass-block top tint colour (linear sRGB 0..1) for biome. Every biome
    /// except Desert and Savanna uses a green-dominant, saturated tint, with
    /// brightness varied by biome.
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

/// Every biome in id order — the canonical key list, pinned by the id-stability
/// and registry-ordering tests. (Biome exposes no public `ALL` const; this is
/// test-only.)
#[cfg(test)]
const ALL_BIOMES: [Biome; 29] = [
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

/// One-line delegating call for the shared id-ordering test in [`crate::registry`]:
/// the `BIOME_DEFS` table is id-ordered and one-to-one with [`ALL_BIOMES`].
#[cfg(test)]
pub(crate) fn assert_registry_ordered() {
    crate::registry::assert_id_ordered(data::BIOME_DEFS, &ALL_BIOMES);
}

#[cfg(test)]
mod tests {
    use super::{blended_fog_color, data, Biome, ALL_BIOMES, SKY_FOG_BLEND_SPAN_BLOCKS};

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
        for (id, biome) in ALL_BIOMES.into_iter().enumerate() {
            assert_eq!(biome.id(), id as u8);
            assert_eq!(Biome::from_id(id as u8), biome);
        }
        assert_eq!(Biome::from_id(u8::MAX), Biome::Ocean);
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

        // A concrete, non-vacuous anchor that the loop above actually ran. Only
        // the name is pinned literally: it is a stable identifier, not a tuned
        // value. Colours are deliberately NOT spot-checked here — they are
        // hand-tuned table data, so duplicating a row's colour in an assertion
        // just goes stale the next time it is retuned (which is exactly what had
        // happened to SnowyPeaks' foliage). The loop already proves every accessor
        // returns its own row, whatever the values are.
        assert_eq!(Biome::Beach.name(), "beach");
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
