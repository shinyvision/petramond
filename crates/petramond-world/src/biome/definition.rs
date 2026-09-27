use super::Biome;

pub(crate) type Color = [f32; 3];

#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct BiomeDef {
    pub biome: Biome,
    pub key: &'static str,
    pub name: &'static str,
    pub fog_color: Color,
    pub grass_color: Color,
    pub foliage_color: Color,
    pub water_color: Color,
    pub ambient: &'static [(&'static str, f32)],
    pub trees: Option<&'static str>,
    pub generation: Option<&'static str>,
}
