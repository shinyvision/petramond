use serde::Deserialize;

use crate::noise::settings::CAVE_MIN_Y;
use petramond_world::block::Block;
use petramond_world::chunk::{WORLD_MAX_Y, WORLD_MIN_Y};
use petramond_world::registry::Catalog;

const ENGINE_UNDERGROUND_BIOME_NAMES: &[&str] = &["petramond:stone"];

#[derive(Copy, Clone, Debug)]
pub struct FaceLining {
    pub block: u16,
    pub weight: f32,
    pub(crate) pattern: Option<&'static pattern::MaterialPattern>,
}

#[derive(Copy, Clone, Debug)]
pub struct LiningFaces {
    pub biome: u8,
    pub floor: FaceLining,
    pub wall: FaceLining,
    pub ceiling: FaceLining,
    pub floor_depth: FloorDepth,
    pub floor_under: Option<FaceLining>,
    pub floor_submerged: Option<FaceLining>,
    pub submerged_in: &'static [u16],
    pub salt: u64,
}

impl LiningFaces {
    #[inline]
    pub fn floor_surface(&self, over: u16) -> FaceLining {
        match self.floor_submerged {
            Some(face) if self.submerged_in.contains(&over) => face,
            _ => self.floor,
        }
    }
}

mod climate;
mod depth;
mod fluids;
pub use fluids::{FluidFall, FluidPool, POOL_CELL};
pub(crate) mod pattern;
mod regions;
use climate::{ordinary_fitness, ordinary_upper, ClimateRange};
pub use climate::{ClimateBox, ClimatePoint};
pub use depth::FloorDepth;
pub(crate) use depth::MAX_FLOOR_DEPTH;

#[derive(Copy, Clone, Debug)]
pub struct Aquifer {
    pub level: i32,
    pub barrier: u16,
    pub fluid: u16,
}

pub struct UndergroundBiomeDef {
    pub name: &'static str,
    climate: Option<ClimateRange>,
    region: Option<regions::RawRegion>,
    y: (i32, i32),
    whole_column: bool,
    lining: u16,
    lining_name: &'static str,
    aquifer: Option<Aquifer>,
    barrier_name: &'static str,
    aquifer_fluid_name: &'static str,
    faces: Option<LiningFaces>,
    face_names: [&'static str; 3],
    shell: f64,
    blend: (f64, f64),
    pattern: Option<&'static pattern::MaterialPattern>,
    geology: Option<&'static pattern::MaterialPattern>,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct IdSet([u64; 4]);

impl IdSet {
    #[inline]
    pub fn insert(&mut self, id: u8) {
        self.0[id as usize >> 6] |= 1 << (id & 63);
    }
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.0 == [0; 4]
    }
    /// Holds the ordinary biome (id 0) and nothing else.
    #[inline]
    pub fn is_ordinary_only(&self) -> bool {
        self.0 == [1, 0, 0, 0]
    }
    #[inline]
    pub fn contains(&self, id: u8) -> bool {
        self.0[id as usize >> 6] & (1 << (id & 63)) != 0
    }
    #[inline]
    pub fn union(&mut self, other: &IdSet) {
        for (a, b) in self.0.iter_mut().zip(other.0) {
            *a |= b;
        }
    }
    pub fn ids(&self) -> Vec<u8> {
        self.iter().collect()
    }

    pub(crate) fn iter(self) -> impl Iterator<Item = u8> {
        self.0.into_iter().enumerate().flat_map(|(word, mut bits)| {
            std::iter::from_fn(move || {
                if bits == 0 {
                    return None;
                }
                let bit = bits.trailing_zeros();
                bits &= bits - 1;
                Some((word * 64 + bit as usize) as u8)
            })
        })
    }
}

pub struct UndergroundBiomes {
    catalog: Catalog<UndergroundBiomeDef>,
    selectors: Box<[u8]>,
    pub(crate) regions: Box<[regions::RegionGroup]>,
    pub base: f64,
    lining: [u16; 256],
    patterns: Box<[Option<&'static pattern::MaterialPattern>; 256]>,
    geology: Box<[Option<&'static pattern::MaterialPattern>; 256]>,
    pub(crate) geology_ids: IdSet,
    pub(crate) geology_via_climate: bool,
    aquifers: [Option<Aquifer>; 256],
    pub aquifer_y_span: Option<(i32, i32)>,
    faces: Box<[Option<LiningFaces>; 256]>,
    pub lining_faces_vary: bool,
    /// Some row paints floors at `CAVE_MIN_Y - 1`. Cave floors at the very bottom rest on a plane
    /// the carve never cuts or even visits, so floor painting has to go one block lower.
    pub lining_floor_under_world_floor: bool,
    pub lining_floor_depth_max: i32,
    pub bounds: f64,
    pub pools: Box<[FluidPool]>,
    pub falls: Box<[FluidFall]>,
    pub fingerprint: u64,
}

impl UndergroundBiomes {
    fn winner(&self, climate: ClimatePoint, y: i32) -> Option<(u8, &UndergroundBiomeDef, f64)> {
        let mut best = ordinary_fitness(climate[5]);
        let mut winner = None;
        for &id in &self.selectors {
            if best == 0.0 {
                break;
            }
            let row = &self.catalog.rows()[id as usize];
            if y < row.y.0 || y > row.y.1 {
                continue;
            }
            if let Some(distance) = row
                .climate
                .expect("compiled climate selector")
                .distance_below(climate, best)
            {
                best = distance;
                winner = Some((id, row, distance));
            }
        }
        winner
    }

    pub fn id_at(&self, climate: ClimatePoint, y: i32) -> u8 {
        self.winner(climate, y).map_or(0, |(id, _, _)| id)
    }

    #[inline]
    pub fn lining(&self, id: u8) -> u16 {
        self.lining[id as usize]
    }

    #[inline]
    pub fn surface_material(&self, seed: u32, biome: u8, block: u16, pos: [i32; 3]) -> u16 {
        if block == self.lining[biome as usize] {
            if let Some(pattern) = self.patterns[biome as usize] {
                return pattern.at(seed, pos);
            }
        }
        block
    }

    #[inline]
    pub(crate) fn surface_material_cached(
        &self,
        seed: u32,
        biome: u8,
        block: u16,
        pos: [i32; 3],
        cache: &mut pattern::ColumnCache,
    ) -> u16 {
        if block == self.lining[biome as usize] {
            if let Some(pattern) = self.patterns[biome as usize] {
                return pattern.at_cached(seed, pos, cache);
            }
        }
        block
    }

    pub(crate) fn face_material_cached(
        &self,
        seed: u32,
        biome: u8,
        face: FaceLining,
        pos: [i32; 3],
        cache: &mut pattern::ColumnCache,
    ) -> u16 {
        match face.pattern {
            Some(p) => p.at_cached(seed, pos, cache),
            None => self.surface_material_cached(seed, biome, face.block, pos, cache),
        }
    }

    pub(crate) fn geology(&self, biome: u8) -> Option<&'static pattern::MaterialPattern> {
        self.geology[biome as usize]
    }

    #[inline]
    pub fn faces(&self, id: u8) -> Option<&LiningFaces> {
        self.faces[id as usize].as_ref()
    }

    pub fn shell_at(&self, climate: ClimatePoint, y: i32) -> f64 {
        match self.winner(climate, y) {
            Some((_, row, distance)) => {
                let edge = (y - row.y.0).min(row.y.1 - y) as f64;
                let weight = if row.blend.1 > 0.0 {
                    smoothstep(edge / row.blend.1)
                } else {
                    1.0
                };
                let climate_weight = if row.blend.0 > 0.0 {
                    smoothstep(
                        (ordinary_fitness(climate[5]) - distance).max(0.0).sqrt() / row.blend.0,
                    )
                } else {
                    1.0
                };
                lerp(self.base, row.shell, weight.min(climate_weight))
            }
            None => self.base,
        }
    }

    pub fn ids_in(&self, y: (i32, i32), climate: ClimateBox, out: &mut IdSet) {
        out.insert(0);
        let mut upper = ordinary_upper(climate[5]);
        let mut lower = [f64::INFINITY; 256];
        let lower = &mut lower[..self.selectors.len()];
        for (&id, lower) in self.selectors.iter().zip(lower.iter_mut()) {
            let row = &self.catalog.rows()[id as usize];
            if row.y.0 > y.1 || row.y.1 < y.0 {
                continue;
            }
            let bounds = row
                .climate
                .expect("compiled selector")
                .distance_bounds(climate);
            *lower = bounds[0];
            if row.y.0 <= y.0 && row.y.1 >= y.1 {
                upper = upper.min(bounds[1]);
            }
        }
        for (&id, &lower) in self.selectors.iter().zip(lower.iter()) {
            if lower <= upper {
                out.insert(id);
            }
        }
    }

    #[inline]
    pub fn aquifer(&self, id: u8) -> Option<Aquifer> {
        self.aquifers[id as usize]
    }

    #[inline]
    pub fn row_y(&self, id: u8) -> (i32, i32) {
        self.catalog.rows()[id as usize].y
    }

    pub fn id(&self, name: &str) -> Option<u8> {
        self.catalog.id(name).map(|id| id as u8)
    }

    pub fn name(&self, id: u8) -> Option<&'static str> {
        self.catalog.rows().get(id as usize).map(|r| r.name)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawUndergroundBiome {
    underground_biome: String,
    #[serde(default)]
    climate: Option<ClimateRange>,
    #[serde(default)]
    region: Option<regions::RawRegion>,
    #[serde(default)]
    y: Option<[i32; 2]>,
    #[serde(default)]
    lining: Option<RawLining>,
    #[serde(default)]
    geology: Option<pattern::RawPattern>,
    #[serde(default)]
    aquifer: Option<RawAquifer>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAquifer {
    level: i32,
    barrier: Block,
    #[serde(default = "water")]
    fluid: Block,
}

fn water() -> Block {
    Block::Water
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLining {
    block: Block,
    #[serde(default = "one")]
    shell: f64,
    #[serde(default = "default_blend")]
    blend: [f64; 2],
    #[serde(default)]
    faces: Option<RawFaces>,
    #[serde(default)]
    pattern: Option<pattern::RawPattern>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFaces {
    #[serde(default)]
    floor: Option<RawFace>,
    #[serde(default)]
    wall: Option<RawFace>,
    #[serde(default)]
    ceiling: Option<RawFace>,
    #[serde(default)]
    floor_depth: depth::RawDepth,
    #[serde(default)]
    floor_under: Option<RawFace>,
    #[serde(default)]
    floor_submerged: Option<RawFace>,
    #[serde(default)]
    submerged_in: Option<Vec<Block>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFace {
    #[serde(default)]
    block: Option<Block>,
    #[serde(default = "one_f32")]
    weight: f32,
    #[serde(default)]
    pattern: Option<pattern::RawPattern>,
}

fn one() -> f64 {
    1.0
}

fn one_f32() -> f32 {
    1.0
}

fn default_blend() -> [f64; 2] {
    [0.05, 8.0]
}

mod load;

use load::parse_layers;

pub(crate) static TABLE: petramond_world::content::Slot<UndergroundBiomes> =
    petramond_world::content::Slot::new(
        "underground_biomes.json",
        &[
            petramond_world::content::stage::TILES,
            petramond_world::content::stage::BLOCKS,
        ],
        load_table,
    );

fn load_table(
    reg: &petramond_world::content::ContentRegistry,
) -> Result<UndergroundBiomes, String> {
    petramond_world::registry::read_catalog(
        reg.packs(),
        "underground_biomes.json",
        "underground biome",
        parse_layers,
    )
}

pub fn table() -> &'static UndergroundBiomes {
    TABLE.current()
}

pub fn id_by_name(name: &str) -> Option<u8> {
    table().id(name)
}

type ConvertedFaces = (Option<LiningFaces>, [&'static str; 3]);

#[inline]
fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

#[inline]
fn smoothstep(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
pub fn test_table(pack_layers: &[&str]) -> &'static UndergroundBiomes {
    let base = shipped_layer();
    let pack: Vec<String> = pack_layers.iter().map(|text| own_keys(text)).collect();
    let mut texts: Vec<&str> = vec![&base];
    texts.extend(pack.iter().map(String::as_str));
    Box::leak(Box::new(
        parse_layers(&texts).expect("synthetic underground table"),
    ))
}

#[cfg(test)]
pub fn synthetic_table(layers: &[&str]) -> &'static UndergroundBiomes {
    const BARE: &str = r#"{"underground_biomes":[{"underground_biome":"petramond:stone"}]}"#;
    let layers: Vec<String> = layers.iter().map(|text| own_keys(text)).collect();
    let mut texts: Vec<&str> = vec![BARE];
    texts.extend(layers.iter().map(String::as_str));
    Box::leak(Box::new(
        parse_layers(&texts).expect("synthetic underground table"),
    ))
}

#[cfg(test)]
fn own_keys(pack: &str) -> String {
    let mut value: serde_json::Value = serde_json::from_str(pack).expect("fixture JSON");
    if let Some(file) = value.as_object_mut() {
        file.retain(|key, _| {
            matches!(
                key.as_str(),
                "underground_biomes" | "fluid_pools" | "fluid_falls"
            )
        });
    }
    value.to_string()
}

#[cfg(test)]
pub fn shipped_layer() -> String {
    petramond_world::assets::read_base_text("underground_biomes.json")
        .expect("shipped underground_biomes.json")
        .0
}

#[cfg(test)]
mod tests;
