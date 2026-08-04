//! Dev-tool surface: feature previews, macro surface maps, positional
//! terrain/biome queries — what genmap/genfeature and pack acceptance checks
//! drive without generating full chunks.

use std::collections::HashMap;

use petramond_world::block::Block;
use petramond_world::chunk::Chunk;
use petramond_world::mathh::IVec3;
use crate::feature::{FeatureCtx, VoxelSink};
use crate::rng::FeatureRng;

const FEATURE_PREVIEW_SALT: u64 = 0x0000_FE47_0000_0001;

pub fn generate_chunk(seed: u32, cx: i32, cz: i32) -> Chunk {
    crate::generate_chunk(seed, cx, cz)
}

/// The underground biome owning each position — the SAME answer the
/// carver's lining and caliber read. A per-biome census needs it: judging
/// "did this row line every floor in its territory" by proximity to the
/// row's own lining block silently counts the neighbouring biome's rim as
/// a miss.
pub fn underground_biome_at(seed: u32, positions: &[[i32; 3]]) -> Vec<u8> {
    crate::underground_biomes_at(seed, positions)
}

/// The underground-biome id registered under `name`.
pub fn underground_biome_id(name: &str) -> Option<u8> {
    crate::data::underground::id_by_name(name)
}

/// Whether the generated terrain is solid at each position — the same
/// answer a mod's `terrain_solid_at` gets. A pack that mixes this with the
/// section snapshot (positional for a neighbour in the next section,
/// `GenCtx::block` for its own cells) is only sound where the two coincide,
/// and that is a property of the INSTALLED set, not of bare terrain, so it
/// wants an instrument outside the engine's own parity test.
pub fn terrain_solid_at(seed: u32, positions: &[[i32; 3]]) -> Vec<bool> {
    crate::terrain_solid_at(seed, positions)
}

// Cubic per-section generation, re-exported so dev tools (genmap's deep
// cross-section / cave statistics) can inspect terrain below y = 0 — the
// whole-column `Chunk` preview only covers `[0, CHUNK_SY)`.
pub use petramond_world::chunk::{SectionPos, SECTION_MAX_CY, SECTION_MIN_CY, SECTION_SIZE, WORLD_MIN_Y};
pub use petramond_world::section::Section;
pub use crate::driver::{ChunkGenerator, ColumnGen};

/// A kilometre-scale surface overview sampled straight from the climate
/// graph (no chunk generation): per grid point the classified biome id and
/// the base surface height. `side` points per edge, `stride` blocks apart,
/// centred on the origin — for verifying world-scale structure (mountain
/// belts, valley networks) that a chunk-sized genmap window cannot show.
pub struct MacroSurfaceMap {
    pub side: usize,
    pub biomes: Vec<u8>,
    pub heights: Vec<f64>,
}

pub fn macro_surface_map(seed: u32, side: usize, stride: i32) -> MacroSurfaceMap {
    use crate::biome::climate::{
        BiomeClimateIndex, ClimateSampleCell, ClimateSampler,
    };
    use crate::density::terrain::{channels, TerrainDensitySpec};
    use crate::graph::SamplePoint;

    let graph = TerrainDensitySpec::default_surface().build_graph(seed);
    let index = BiomeClimateIndex::default_surface().clone();
    let sampler = ClimateSampler::new(graph.graph());
    let half = (side as i32 / 2) * stride;
    let mut biomes = Vec::with_capacity(side * side);
    let mut heights = Vec::with_capacity(side * side);
    for gz in 0..side as i32 {
        for gx in 0..side as i32 {
            let wx = gx * stride - half;
            let wz = gz * stride - half;
            let biome = sampler
                .sample_surface_cell(ClimateSampleCell::surface(wx, wz))
                .and_then(|sample| index.classify_surface(sample.climate))
                .map(|b| b as u8)
                .unwrap_or(0);
            let height = graph
                .graph()
                .evaluate_channel(
                    channels::BASE_HEIGHT,
                    SamplePoint::new(f64::from(wx), 0.0, f64::from(wz)),
                )
                .unwrap_or(0.0);
            biomes.push(biome);
            heights.push(height);
        }
    }
    MacroSurfaceMap {
        side,
        biomes,
        heights,
    }
}

pub fn feature_preview_names() -> &'static [&'static str] {
    &[
        "redwood",
        "oak_young",
        "oak_small",
        "oak_swamp",
        "oak_big",
        "spruce",
        "birch",
        "jungle",
        "acacia",
    ]
}

pub fn preview_feature(name: &str, seed: u32) -> Option<FeaturePreview> {
    let cf = configured_feature(name)?;
    let mut sink = PreviewSink::default();
    let mut ctx = FeatureCtx::new(&mut sink);
    let mut rng = FeatureRng::positional(seed, FEATURE_PREVIEW_SALT, 0, 0, 0);
    cf.feature.generate(&mut ctx, IVec3::new(0, 0, 0), &mut rng);

    let mut bounds = FeatureBounds::empty();
    let mut voxels: Vec<FeatureVoxel> = sink
        .voxels
        .into_iter()
        .map(|(pos, block)| {
            bounds.include(pos);
            FeatureVoxel {
                pos: [pos.x, pos.y, pos.z],
                block,
            }
        })
        .collect();
    voxels.sort_by_key(|v| (v.pos[1], v.pos[2], v.pos[0], v.block.id()));
    Some(FeaturePreview { voxels, bounds })
}

fn configured_feature(
    name: &str,
) -> Option<&'static crate::feature::ConfiguredFeature> {
    let key = name.trim().to_ascii_lowercase().replace('-', "_");
    let key = match key.as_str() {
        "young_oak" => "oak_young",
        "oak" => "oak_small",
        "swamp_oak" => "oak_swamp",
        "giant_oak" | "fancy_oak" => "oak_big",
        other => other,
    };
    crate::data::features::by_name(&format!("petramond:{key}"))
}

#[derive(Clone, Debug)]
pub struct FeaturePreview {
    pub voxels: Vec<FeatureVoxel>,
    pub bounds: FeatureBounds,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct FeatureVoxel {
    pub pos: [i32; 3],
    pub block: Block,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct FeatureBounds {
    pub min: [i32; 3],
    pub max: [i32; 3],
    pub empty: bool,
}

impl FeatureBounds {
    fn empty() -> Self {
        Self {
            min: [0; 3],
            max: [0; 3],
            empty: true,
        }
    }

    fn include(&mut self, p: IVec3) {
        if self.empty {
            self.min = [p.x, p.y, p.z];
            self.max = [p.x, p.y, p.z];
            self.empty = false;
            return;
        }
        self.min[0] = self.min[0].min(p.x);
        self.min[1] = self.min[1].min(p.y);
        self.min[2] = self.min[2].min(p.z);
        self.max[0] = self.max[0].max(p.x);
        self.max[1] = self.max[1].max(p.y);
        self.max[2] = self.max[2].max(p.z);
    }
}

#[derive(Default)]
struct PreviewSink {
    voxels: HashMap<IVec3, Block>,
}

impl VoxelSink for PreviewSink {
    fn get(&self, p: IVec3) -> Block {
        self.voxels.get(&p).copied().unwrap_or(Block::Air)
    }

    fn set(&mut self, p: IVec3, b: Block) {
        self.voxels.insert(p, b);
    }
}

pub mod audit {
    pub use crate::audit::{
        audit, flood_audit, relief_audit, roughness, BiomeShare, DebrisAudit, FloodAudit,
        HeightStats, ReliefStats, RoughnessStats, RELIEF_HIST_LABELS,
    };
}
