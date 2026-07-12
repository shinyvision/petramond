//! Biome definitions and per-biome metadata (names, ids, fog/grass/foliage/water colours).

mod data;
mod definition;

#[repr(u8)]
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Biome {
    Ocean = 1,
    Beach,
    River,
    Desert,
    Plains,
    Savanna,
    Forest,
    Swamp,
    Taiga,
    SnowyTundra,
    SnowyTaiga,
    Mountains,
    SnowyPeaks,
    DeepOcean,
    Foothills,
    Wetland,
    // --- appended (ids 16+): keep append-only; never reorder (biome ids are
    // serialized into chunk bytes). ---
    RedwoodForest,
    OldGrowthTaiga,
    Meadow,
    Grove,
    SnowySlopes,
    WindsweptHills,
    StonyPeaks,
    WoodedHills,
    MountainEdge,
    DesertLakes,
}

pub const BIOME_COUNT: usize = data::BIOME_DEFS.len();

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

    /// Resolve a biome by its stable snake_case name (`"forest"`), for data-driven
    /// catalogs (e.g. mob spawn rules in `mobs.json`) that reference biomes by name.
    pub fn from_name(name: &str) -> Option<Biome> {
        (1..=BIOME_COUNT as u8)
            .map(Biome::from_id)
            .find(|b| b.name() == name)
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

#[cfg(test)]
mod tests {
    use super::{blended_fog_color, Biome, BIOME_COUNT, SKY_FOG_BLEND_SPAN_BLOCKS};

    /// The mod-facing biome vocabulary (`mod_api::biome`) mirrors this
    /// compiled table. Worldgen hooks hand mods raw biome ids; the ABI names
    /// are their only sanctioned addressing, so a drifted or missing entry
    /// must fail HERE, not in a mod at runtime.
    #[test]
    fn mod_api_biome_vocabulary_matches_the_engine_table() {
        assert_eq!(
            mod_api::biome::BIOME_NAMES.len(),
            BIOME_COUNT,
            "append new biomes to mod_api::biome in the same change"
        );
        for id in 1..=BIOME_COUNT as u8 {
            let biome = Biome::from_id(id);
            assert_eq!(mod_api::biome::name(id), Some(biome.name()));
            assert_eq!(mod_api::biome::by_name(biome.name()), Some(id));
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
