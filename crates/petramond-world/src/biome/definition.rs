use super::Biome;

pub(super) type Color = [f32; 3];

#[derive(Copy, Clone, Debug, PartialEq)]
pub(super) struct BiomeDef {
    pub biome: Biome,
    pub name: &'static str,
    pub fog_color: Color,
    pub grass_color: Color,
    pub foliage_color: Color,
    pub water_color: Color,
    /// Ambient bundle key → density: the bundles this biome drives.
    pub ambient: &'static [(&'static str, f32)],
    /// The row's `trees` object as canonical JSON text, `None` when the row
    /// states none. Opaque here: worldgen owns the vocabulary and parses it.
    pub trees: Option<&'static str>,
}
