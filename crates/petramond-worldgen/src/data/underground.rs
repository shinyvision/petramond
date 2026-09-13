//! Underground habitat identity, lining and local water configuration.
//!
//! Spatial regions gate habitat ranges before nearest-climate fallback.
//! Equal-fitness rows resolve by name, independently of registry order.
//! Excavation requests live in their own catalog and do not follow from biome identity.
//!
//! This table resolves blocks, so do not initialize it from a block registry loader.

use std::sync::LazyLock;

use serde::Deserialize;

use crate::noise::settings::CAVE_MIN_Y;
use petramond_world::block::Block;
use petramond_world::chunk::{WORLD_MAX_Y, WORLD_MIN_Y};
use petramond_world::registry::Catalog;

/// Engine underground-biome names in frozen id order. Id 0 is the structural
/// fallback, competing with custom climate ranges at surface and deep levels.
const ENGINE_UNDERGROUND_BIOME_NAMES: &[&str] = &["petramond:stone"];

#[derive(Copy, Clone, Debug)]
pub struct FaceLining {
    /// Block written here; `0` (air) leaves the rock bare.
    pub block: u16,
    /// Share of eligible cells painted. `1.0` takes NO roll at all, which is
    /// what makes a floor rule a guarantee rather than a high probability.
    pub weight: f32,
    pub(crate) pattern: Option<&'static pattern::MaterialPattern>,
}

/// Per-orientation lining for one row. Absent (`UndergroundBiomes::faces`
/// answers `None`) means the row lines its walls the one way it always did, so
/// a table with no `faces` anywhere compiles to exactly the old code path.
#[derive(Copy, Clone, Debug)]
pub struct LiningFaces {
    pub biome: u8,
    /// A cell directly under carved air. Painted whether or not it falls in the
    /// carve's lining SHELL — that is the whole point of the clause, since the
    /// shell is a level set in noise units and comes out thinnest exactly where
    /// the field's Y gradient is steepest, i.e. on horizontal surfaces.
    pub floor: FaceLining,
    /// A cell that hugs a wall (the shell band) and is neither floor nor
    /// ceiling.
    pub wall: FaceLining,
    /// A cell in the shell band directly above carved air.
    pub ceiling: FaceLining,
    /// Bounded thickness sampled at the exposed floor.
    pub floor_depth: FloorDepth,
    /// Subsurface material. Omit to use the top material throughout the course.
    pub floor_under: Option<FaceLining>,
    pub floor_submerged: Option<FaceLining>,
    /// The fluids a floor counts as submerged under.
    pub submerged_in: &'static [u16],
    /// Dither stream salt, from the row's namespaced NAME with its own prefix
    /// so independent surface-treatment consumers do not share a random stream.
    pub salt: u64,
}

impl LiningFaces {
    /// The surface lining of a floor course whose open cell holds `over`.
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
    /// Highest filled voxel in the territory.
    pub level: i32,
    pub barrier: u16,
    pub fluid: u16,
}

/// One row of the loaded underground-biome table.
pub struct UndergroundBiomeDef {
    /// The row's registry name (`"petramond:marble"`, `"mymod:mushroom_cavern"`).
    pub name: &'static str,
    climate: Option<ClimateRange>,
    region: Option<regions::RawRegion>,
    y: (i32, i32),
    whole_column: bool,
    /// Block id this biome lines cave walls with; `0` (air) = bare stone.
    lining: u16,
    lining_name: &'static str,
    aquifer: Option<Aquifer>,
    barrier_name: &'static str,
    aquifer_fluid_name: &'static str,
    /// Per-orientation override of `lining`; `None` = the one-block-everywhere
    /// shell every row had before.
    faces: Option<LiningFaces>,
    face_names: [&'static str; 3],
    shell: f64,
    blend: (f64, f64),
    pattern: Option<&'static pattern::MaterialPattern>,
    geology: Option<&'static pattern::MaterialPattern>,
}

/// A set over the closed row-id space (ids are `u8`), for the box queries that
/// answer "which biomes CAN be here" without visiting a cell.
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

/// The compiled underground-biome partition: everything the carver and the mod
/// ABI read, in the shapes their hot paths need.
pub struct UndergroundBiomes {
    catalog: Catalog<UndergroundBiomeDef>,
    /// Name-sorted row ids make equal-fitness selection independent of load order.
    selectors: Box<[u8]>,
    pub(crate) regions: Box<[regions::RegionGroup]>,
    pub base: f64,
    /// Dense lining block id per biome id (`0` = none): the carve inner loop's
    /// only lining lookup.
    lining: [u16; 256],
    patterns: Box<[Option<&'static pattern::MaterialPattern>; 256]>,
    geology: Box<[Option<&'static pattern::MaterialPattern>; 256]>,
    pub(crate) geology_ids: IdSet,
    /// Hoisted: some row the nearest-climate fallback can answer with has a
    /// geology pattern, so a column with no regional candidate still needs
    /// the per-cell pass.
    pub(crate) geology_via_climate: bool,
    aquifers: [Option<Aquifer>; 256],
    pub aquifer_y_span: Option<(i32, i32)>,
    /// Dense per-orientation lining, parallel to [`Self::lining`].
    faces: Box<[Option<LiningFaces>; 256]>,
    /// Hoisted from `faces`: whether ANY row declares one. The single gate on
    /// the orientation machinery — the extra lattice row it needs in Y, the
    /// skip mask's dilation, and the column bookkeeping all hang off it, so a
    /// table with no `faces` (every shipped one) carves exactly as before.
    pub lining_faces_vary: bool,
    /// Hoisted: some row paints FLOORS and claims depth `CAVE_MIN_Y - 1`. The
    /// deepest cave floor in the world rests on the plane the carvers refuse to
    /// cut, and that plane lives in a section the carve otherwise skips
    /// entirely — so a floor guarantee has to reach one block below the carve.
    pub lining_floor_under_world_floor: bool,
    /// Hoisted: the deepest floor course any row paints (`0` when none does).
    /// A course can start above a batch's top voxel and reach down into it, so
    /// the carve lattice pads Y by this much — the batch has to be able to ASK
    /// how far the cave floor above it is, and a remembered answer would make
    /// the course depend on which batch generated the cell.
    pub lining_floor_depth_max: i32,
    /// Maximum lining-shell multiplier over every loaded habitat.
    pub bounds: f64,
    /// Generated pools, in priority order.
    pub pools: Box<[FluidPool]>,
    /// Generated falls, in priority order.
    pub falls: Box<[FluidFall]>,
    /// Hash of the compiled table — stamped into the column-gen cache so a
    /// pack that changes cave shape cannot be served stale cached columns.
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

    /// The lining block id for a biome id (`0` = bare stone).
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

    /// [`Self::surface_material`] through a column's pattern cache.
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

    /// [`Self::face_material`] through a column's pattern cache.
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

    /// The per-orientation lining for a biome id, or `None` when the row lines
    /// every cave surface with the same block.
    #[inline]
    pub fn faces(&self, id: u8) -> Option<&LiningFaces> {
        self.faces[id as usize].as_ref()
    }

    /// Habitat treatment does not contribute to the density decision.
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

    /// Conservative nearest-climate candidates throughout a spatial query.
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

    /// A row's inclusive world-height band.
    #[inline]
    pub fn row_y(&self, id: u8) -> (i32, i32) {
        self.catalog.rows()[id as usize].y
    }

    /// The id registered under `name`, or `None` when no such row is loaded.
    pub fn id(&self, name: &str) -> Option<u8> {
        self.catalog.id(name).map(|id| id as u8)
    }

    /// The registry name of `id`, or `None` when out of range.
    pub fn name(&self, id: u8) -> Option<&'static str> {
        self.catalog.rows().get(id as usize).map(|r| r.name)
    }
}

/// One underground-biome row as written in `underground_biomes.json`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawUndergroundBiome {
    underground_biome: String,
    #[serde(default)]
    climate: Option<ClimateRange>,
    #[serde(default)]
    region: Option<regions::RawRegion>,
    /// Inclusive `[min, max]` world-height band; absent = the whole column.
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
    /// Blocks of rock below a cave floor the floor rule paints.
    #[serde(default)]
    floor_depth: depth::RawDepth,
    /// The course BELOW its top cell — surface over subsurface, as grass sits
    /// over dirt. Omit to paint the whole course with the floor block.
    #[serde(default)]
    floor_under: Option<RawFace>,
    #[serde(default)]
    floor_submerged: Option<RawFace>,
    /// The fluids `floor_submerged` applies under; omit for the row's aquifer fluid.
    #[serde(default)]
    submerged_in: Option<Vec<Block>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFace {
    /// Overrides `lining.block` for this orientation; omit to use it.
    #[serde(default)]
    block: Option<Block>,
    /// Share of eligible cells painted; `1` (the default) is every one of them.
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

/// Lining feather widths: climate fitness distance, then blocks.
fn default_blend() -> [f64; 2] {
    [0.05, 8.0]
}

// Load bounds. Generous by design — they bound how far the carver's skip mask
// has to widen (and hence generation cost), not the author's taste. Violations
// are load errors: loud beats plausible.

/// The process-wide table, built once from the real catalog layers.
///
/// See the module docs: safe from worldgen and the host-call handlers, never
/// from a block/item/shape loader.
mod load;

use load::parse_layers;

pub fn table() -> &'static UndergroundBiomes {
    static TABLE: LazyLock<UndergroundBiomes> = LazyLock::new(|| {
        petramond_world::registry::read_catalog(
            "underground_biomes.json",
            "underground biome",
            parse_layers,
        )
    });
    &TABLE
}

/// The underground-biome id registered under `name` — the mod ABI's resolver.
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

/// Test seam: compile a table from the shipped engine layer plus synthetic pack
/// layers, leaked so it can drive a [`CaveField`](crate::noise::cave_field::CaveField)
/// without touching the process-wide catalog.
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

/// A table from synthetic layers over a bare ordinary-stone base: no shipped
/// habitat, pool or fall row takes part.
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

/// A test fixture is a whole PACK — habitats beside excavations in one text —
/// while the loader reads only its own file, which rejects foreign keys.
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

/// The BASE layer only — a synthetic table must mean the same thing whether or
/// not a pack shipping its own cave biomes happens to be installed.
#[cfg(test)]
pub fn shipped_layer() -> String {
    petramond_world::assets::read_base_text("underground_biomes.json")
        .expect("shipped underground_biomes.json")
        .0
}

#[cfg(test)]
mod tests;
