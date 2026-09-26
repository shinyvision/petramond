//! Biome identity (a pack-extensible registry id) and per-biome metadata
//! (names, fog/grass/foliage/water colours, ambience, worldgen rows).

mod data;
mod definition;

/// A surface biome: an opaque one-byte registry id into the layered
/// `biomes.json` catalog, like blocks and underground biomes.
///
/// The engine biomes own ids `1..=ENGINE_BIOME_COUNT` in a frozen order (the
/// associated consts below; append-only, never reorder — ids are serialized
/// into chunk bytes, one per column). A mod pack ADDS a biome by stating a
/// row under its own namespaced key (`mod_id:name`); it registers the next id
/// after the engine range, in pack load order. Id 0 is unassigned.
#[repr(transparent)]
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Biome(u8);

impl Biome {
    pub const OCEAN: Biome = Biome(1);
    pub const BEACH: Biome = Biome(2);
    pub const RIVER: Biome = Biome(3);
    pub const DESERT: Biome = Biome(4);
    pub const PLAINS: Biome = Biome(5);
    pub const SAVANNA: Biome = Biome(6);
    pub const FOREST: Biome = Biome(7);
    pub const SWAMP: Biome = Biome(8);
    pub const TAIGA: Biome = Biome(9);
    pub const SNOWY_TUNDRA: Biome = Biome(10);
    pub const SNOWY_TAIGA: Biome = Biome(11);
    pub const MOUNTAINS: Biome = Biome(12);
    pub const SNOWY_PEAKS: Biome = Biome(13);
    pub const DEEP_OCEAN: Biome = Biome(14);
    pub const FOOTHILLS: Biome = Biome(15);
    pub const WETLAND: Biome = Biome(16);
    pub const REDWOOD_FOREST: Biome = Biome(17);
    pub const OLD_GROWTH_TAIGA: Biome = Biome(18);
    pub const MEADOW: Biome = Biome(19);
    pub const GROVE: Biome = Biome(20);
    pub const SNOWY_SLOPES: Biome = Biome(21);
    pub const WINDSWEPT_HILLS: Biome = Biome(22);
    pub const STONY_PEAKS: Biome = Biome(23);
    pub const WOODED_HILLS: Biome = Biome(24);
    pub const MOUNTAIN_EDGE: Biome = Biome(25);
    pub const DESERT_LAKES: Biome = Biome(26);
    pub const SNOWY_PLAINS: Biome = Biome(27);
}

/// How many biomes the engine itself defines (ids `1..=ENGINE_BIOME_COUNT`).
pub const ENGINE_BIOME_COUNT: usize = data::ENGINE_BIOME_COUNT;

/// How many biomes are registered: the engine range plus every pack biome.
/// Registered ids are exactly `1..=count()`.
#[inline]
pub fn count() -> usize {
    data::count()
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

    /// The row's `generation` rules as JSON text (surface stack, ground
    /// cover, snow, behaviour flags), `None` when the row states none.
    /// Worldgen owns and parses the vocabulary.
    #[inline]
    pub fn generation(self) -> Option<&'static str> {
        self.def().generation
    }

    #[inline]
    pub fn fog_color(self) -> [f32; 3] {
        self.def().fog_color
    }

    /// The stable name: the bare snake_case half for an engine biome
    /// (`"forest"`), the full namespaced key for a pack biome
    /// (`"mymod:crystal_fields"`).
    #[inline]
    pub fn name(self) -> &'static str {
        self.def().name
    }

    /// The registry key (`"petramond:forest"`, `"mymod:crystal_fields"`).
    #[inline]
    pub fn key(self) -> &'static str {
        self.def().key
    }

    /// The biome registered under `id`; an unregistered id (0, or past the
    /// loaded catalog) reads as [`Biome::OCEAN`].
    #[inline]
    pub fn from_id(id: u8) -> Biome {
        if (1..=count()).contains(&usize::from(id)) {
            Biome(id)
        } else {
            Biome::OCEAN
        }
    }

    /// Resolve a biome for data-driven catalogs (mob spawn rules, tree
    /// profiles, the climate table): a registry key (`"petramond:forest"`,
    /// `"mymod:crystal_fields"`) or an engine biome's bare name (`"forest"`).
    pub fn from_name(name: &str) -> Option<Biome> {
        data::id_of(name).map(Biome)
    }

    /// Every registered biome, in id order.
    pub fn all() -> impl Iterator<Item = Biome> {
        (1..=count()).map(|id| Biome(id as u8))
    }

    #[inline]
    pub fn id(self) -> u8 {
        self.0
    }

    /// Grass-block top tint colour (linear sRGB 0..1) for biome. Every engine
    /// biome except Desert and Savanna uses a green-dominant, saturated tint,
    /// with brightness varied by biome.
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
