//! Composable feature system — replaces the bespoke `trees::oak_*` functions.
//!
//! A feature is split into reusable, data-driven pieces:
//!   - `Feature`        — the imperative voxel-writing shape (e.g. `TreeFeature`)
//!   - `TrunkPlacer` / `FoliagePlacer` — reusable sub-shapes a tree composes
//!   - `ConfiguredFeature` — a feature + baked params (the five oaks are rows)
//!   - `PlacedFeature`  — configured + where/how-often/when (`PlacementModifier`s)
//!
//! Strata P3: the abstraction is established and the oaks become data, but the
//! per-column placement loop reproduces the god file's exact two-roll
//! (`tree_probability` chance → `pick_oak_variant` `next_i32(0,99)`) and every
//! placer mirrors its original RNG draw order and block-write order, so output
//! is byte-parity under the unchanged per-chunk xorshift64 stream. P4 switches
//! to the generalized `PlacementModifier`/`DecoStep` walk + positional RNG.

pub mod placement;
pub mod placers;
pub mod tree;

use crate::block::Block;
use crate::chunk::{Chunk, CHUNK_SX, CHUNK_SY, CHUNK_SZ, SEA_LEVEL};
use crate::mathh::IVec3;

use super::climate::source::BiomeSource;
use super::data;
use super::noise::HeightField;
use super::rng::FeatureRng;

/// Decoration step ordering. Features declare which step they run in so the
/// driver can place all of step N before step N+1. P4 iterates these; P3
/// places trees in a single implicit `VegetationTall` pass.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum DecoStep {
    RawGeneration,
    Lakes,
    Ores,
    VegetationGround,
    VegetationTall,
    Scatter,
}

impl DecoStep {
    pub const ALL: [DecoStep; 6] = [
        DecoStep::RawGeneration,
        DecoStep::Lakes,
        DecoStep::Ores,
        DecoStep::VegetationGround,
        DecoStep::VegetationTall,
        DecoStep::Scatter,
    ];
}

/// A worldgen feature: imperatively writes voxels around a world origin.
pub trait Feature: Send + Sync {
    fn generate(&self, ctx: &mut FeatureCtx, origin: IVec3, rng: &mut FeatureRng);
    /// Worst-case horizontal reach from origin (validated against MARGIN at P4).
    fn max_footprint(&self) -> i32;
}

/// A feature plus its baked parameters.
pub struct ConfiguredFeature {
    pub feature: &'static dyn Feature,
}

/// A configured feature plus placement (where / how often / when).
pub struct PlacedFeature {
    pub configured: &'static ConfiguredFeature,
    pub step: DecoStep,
    pub placement: &'static [placement::PlacementModifier],
}

/// Bounded voxel writer — the ONLY place imperative feature writes happen.
///
/// P3: wraps a `&mut Chunk` in LOCAL chunk coords (0..16, 0..256) and reproduces
/// the god file's three overwrite predicates exactly. P4 reparents this to a
/// bordered `ProtoChunk` in world coords so features can cross chunk seams.
pub struct FeatureCtx<'a> {
    chunk: &'a mut Chunk,
}

impl<'a> FeatureCtx<'a> {
    pub fn new(chunk: &'a mut Chunk) -> Self {
        Self { chunk }
    }

    #[inline]
    fn in_bounds(p: IVec3) -> bool {
        p.x >= 0 && p.x < 16 && p.z >= 0 && p.z < 16 && p.y >= 0 && p.y < CHUNK_SY as i32
    }

    /// Unconditional write (== `trees::log_at`).
    pub fn set_log(&mut self, p: IVec3, b: Block) {
        if Self::in_bounds(p) {
            self.chunk.set_block_raw(p.x as usize, p.y as usize, p.z as usize, b.id());
        }
    }

    /// Write over Air/Water only (== `trees::leaf_at`).
    pub fn set_leaf(&mut self, p: IVec3, b: Block) {
        if Self::in_bounds(p) {
            let cur = self.chunk.block_raw(p.x as usize, p.y as usize, p.z as usize);
            if cur == Block::Air.id() || cur == Block::Water.id() {
                self.chunk.set_block_raw(p.x as usize, p.y as usize, p.z as usize, b.id());
            }
        }
    }

    /// Write over Air/OakLeaves/Water (== `oak_big` branch predicate).
    pub fn set_branch(&mut self, p: IVec3, b: Block) {
        if Self::in_bounds(p) {
            let cur = self.chunk.block_raw(p.x as usize, p.y as usize, p.z as usize);
            if cur == Block::Air.id() || cur == Block::OakLeaves.id() || cur == Block::Water.id() {
                self.chunk.set_block_raw(p.x as usize, p.y as usize, p.z as usize, b.id());
            }
        }
    }

    /// Unconditional leaf write (== `leaf_blob` with `allow_overwrite = true`).
    pub fn set_leaf_force(&mut self, p: IVec3, b: Block) {
        if Self::in_bounds(p) {
            self.chunk.set_block_raw(p.x as usize, p.y as usize, p.z as usize, b.id());
        }
    }
}

/// Per-chunk feature placement. Reproduces `features::place_features` exactly:
/// edge-skip, surf>sea gate, per-biome tree density (one `chance` roll), variant
/// pick (one `next_i32(0,99)` roll), then the `place_oak` height guard, then the
/// feature's own draws. Byte-parity under the per-chunk xorshift64 stream.
pub fn place_features(
    chunk: &mut Chunk,
    field: &HeightField,
    biome_source: &dyn BiomeSource,
    seed: u32,
    cx: i32,
    cz: i32,
) {
    let (ox, oz) = chunk.chunk_origin_world();
    let mut rng = FeatureRng::new(seed, cx, cz);
    let mut ctx = FeatureCtx::new(chunk);

    for z in 0..CHUNK_SZ {
        for x in 0..CHUNK_SX {
            // Edge-skip: avoid cross-chunk writes (removed at P4 via margin).
            if x == 0 || z == 0 || x == CHUNK_SX - 1 || z == CHUNK_SZ - 1 {
                continue;
            }
            let wx = ox + x as i32;
            let wz = oz + z as i32;
            let surf = field.surface_height(wx, wz);
            if surf <= SEA_LEVEL {
                continue;
            }
            let climate = field.climate(wx, wz);
            let biome = biome_source.pick(&climate, surf);

            // Roll 1: per-biome tree density.
            let p = data::features::tree_density(biome);
            if !rng.chance(p) {
                continue;
            }
            // Roll 2: variant pick (draws even if the height guard then fails).
            let cf = data::features::pick_oak(&mut rng, biome);

            // place_oak height guard.
            if surf < 1 || surf + 12 >= CHUNK_SY as i32 {
                continue;
            }
            cf.feature
                .generate(&mut ctx, IVec3::new(x as i32, surf, z as i32), &mut rng);
        }
    }
}
