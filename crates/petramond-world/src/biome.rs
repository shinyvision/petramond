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
    SnowyPlains,
}

pub const BIOME_COUNT: usize = data::ENGINE_BIOME_COUNT;

/// Radius, in blocks, used when blending the above-water sky/fog colour across
/// neighbouring biome columns.
pub const SKY_FOG_BLEND_SPAN_BLOCKS: i32 = 10;

/// Blend above-water sky/fog colour from nearby biome columns.
///
/// The sample kernel fades smoothly to zero at `SKY_FOG_BLEND_SPAN_BLOCKS`, so a
/// single border crossfades gradually and multi-biome intersections naturally
/// become a weighted mix of every biome near the camera.
pub fn blended_fog_color(
    x: f64,
    z: f64,
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
            let dx = (f64::from(wx) + 0.5 - x) as f32;
            let dz = (f64::from(wz) + 0.5 - z) as f32;
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
    /// The ambient particle bundles this biome drives, as `(bundle key,
    /// density 0..=1)` — the row's `ambient` map. The per-id table over it is
    /// [`crate::particle_emitters::biome_intensity`].
    #[inline]
    pub fn ambient(self) -> &'static [(&'static str, f32)] {
        self.def().ambient
    }

    /// The row's `trees` placement profile as JSON text, `None` when the row
    /// states none. Worldgen owns and parses the vocabulary.
    #[inline]
    pub fn trees(self) -> Option<&'static str> {
        self.def().trees
    }

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
mod tests;
