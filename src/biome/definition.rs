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
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub(super) struct HumidityBand {
    pub max_humidity: f32,
    pub biome: Biome,
}
