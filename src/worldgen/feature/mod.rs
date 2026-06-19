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

use super::carve::CarverSet;
use super::climate::source::BiomeSource;
use super::data;
use super::field_cache::FieldCache;
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
/// Coordinates are WORLD coords. Writes are clipped to this chunk's own
/// `[0,16)×[0,16)` (and `[0,256)` vertically); out-of-footprint writes are
/// dropped and out-of-footprint reads return Air. Because every retained write
/// only ever reads in-chunk cells, a feature rooted in a neighbour materialises
/// its overlapping voxels here identically to how the owner chunk does — giving
/// seam-consistent cross-chunk features with no shared buffer. Reproduces the
/// god file's three overwrite predicates (`log_at`/`leaf_at`/`oak_big`-branch).
pub struct FeatureCtx<'a> {
    chunk: &'a mut Chunk,
    ox: i32,
    oz: i32,
}

impl<'a> FeatureCtx<'a> {
    pub fn new(chunk: &'a mut Chunk) -> Self {
        let (ox, oz) = chunk.chunk_origin_world();
        Self { chunk, ox, oz }
    }

    /// Map a world position to in-chunk local indices, or `None` if outside.
    #[inline]
    fn local(&self, p: IVec3) -> Option<(usize, usize, usize)> {
        let lx = p.x - self.ox;
        let lz = p.z - self.oz;
        if lx < 0 || lx >= 16 || lz < 0 || lz >= 16 || p.y < 0 || p.y >= CHUNK_SY as i32 {
            return None;
        }
        Some((lx as usize, p.y as usize, lz as usize))
    }

    /// Unconditional write (== `trees::log_at`).
    pub fn set_log(&mut self, p: IVec3, b: Block) {
        if let Some((x, y, z)) = self.local(p) {
            self.chunk.set_block_raw(x, y, z, b.id());
        }
    }

    /// Write over Air/Water only (== `trees::leaf_at`).
    pub fn set_leaf(&mut self, p: IVec3, b: Block) {
        if let Some((x, y, z)) = self.local(p) {
            let c = self.chunk.block_raw(x, y, z);
            if c == Block::Air.id() || c == Block::Water.id() {
                self.chunk.set_block_raw(x, y, z, b.id());
            }
        }
    }

    /// Write over Air/OakLeaves/Water (== `oak_big` branch predicate).
    pub fn set_branch(&mut self, p: IVec3, b: Block) {
        if let Some((x, y, z)) = self.local(p) {
            let c = self.chunk.block_raw(x, y, z);
            if c == Block::Air.id() || c == Block::OakLeaves.id() || c == Block::Water.id() {
                self.chunk.set_block_raw(x, y, z, b.id());
            }
        }
    }

    /// Unconditional leaf write (== `leaf_blob` with `allow_overwrite = true`).
    pub fn set_leaf_force(&mut self, p: IVec3, b: Block) {
        if let Some((x, y, z)) = self.local(p) {
            self.chunk.set_block_raw(x, y, z, b.id());
        }
    }
}

/// Salt distinguishing the tree-feature positional RNG stream from other users.
const FEATURE_SALT: u64 = 0x0000_7A3E_0AC0_FFEE;

/// Per-chunk feature placement (P4). Iterates feature origins across the chunk
/// plus a `MARGIN` border, in canonical (wz, wx) order, so a tree rooted in a
/// neighbour that reaches into this chunk is generated here too. Each origin
/// seeds its OWN positional RNG (`FeatureRng::positional`), so the per-biome
/// density roll, variant pick, and geometry are pure functions of (seed, wx, wz)
/// — independent of chunk and order. Features write in world coords and are
/// clipped to this chunk, so seams are continuous with no double-placement and
/// the old chunk-edge skip is gone.
pub fn place_features(
    chunk: &mut Chunk,
    cache: &mut FieldCache,
    carvers: &CarverSet,
    biome_source: &dyn BiomeSource,
    seed: u32,
) {
    let mut ctx = FeatureCtx::new(chunk);
    let (ox, oz) = (ctx.ox, ctx.oz);
    let margin = super::proto::MARGIN;

    for wz in (oz - margin)..(oz + CHUNK_SZ as i32 + margin) {
        for wx in (ox - margin)..(ox + CHUNK_SX as i32 + margin) {
            let surf = cache.surf(wx, wz);
            // Anchor to the ACTUAL top solid block, which on a river column is the
            // carved valley floor — not the natural heightfield surface. Otherwise
            // a tree on a river column would float over the carved channel. The
            // floor matches the driver's carve exactly (no bed noise), so trees sit
            // on the ground; wet channels (floor at/below sea) drop out of the
            // `<= SEA_LEVEL` guard below, so nothing grows in the water.
            let river = cache.river(wx, wz);
            let plan = carvers.smoothed_plan(cache, wx, wz, river, surf);
            let anchor = if plan.carve { plan.river_floor } else { surf };
            // No trees in water/on the riverbed, and a treeline at the overhang
            // onset (y96): above it columns can be 3-D carved, so a tree anchored
            // at the heightfield `surf` could float or bury — so we don't plant there.
            if anchor <= SEA_LEVEL || surf > 95 {
                continue;
            }
            // Biome from the natural surface (matches the column's stored biome id).
            let climate = cache.climate(wx, wz);
            let biome = biome_source.pick(&climate, surf);
            let p = data::features::tree_density(biome);
            if p <= 0.0 {
                continue;
            }

            // Each origin draws its own positional stream.
            let mut rng = FeatureRng::positional(seed, FEATURE_SALT, wx, 0, wz);
            if !rng.chance(p) {
                continue;
            }
            let cf = data::features::pick_oak(&mut rng, biome);

            // place_oak height guard (origin too low / too near the world top).
            if anchor < 1 || anchor + 14 >= CHUNK_SY as i32 {
                continue;
            }
            cf.feature
                .generate(&mut ctx, IVec3::new(wx, anchor, wz), &mut rng);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::block::Block;
    use crate::chunk::{CHUNK_SX, CHUNK_SY, CHUNK_SZ};
    use crate::worldgen::generate_chunk;

    fn is_tree(id: u8) -> bool {
        id == Block::OakLog.id() || id == Block::OakLeaves.id()
    }

    #[test]
    fn generate_chunk_is_deterministic() {
        let seed = 0x1234_5678;
        for &(cx, cz) in &[(0, 0), (3, -2), (-5, 7), (12, 9)] {
            let a = generate_chunk(seed, cx, cz);
            let b = generate_chunk(seed, cx, cz);
            assert_eq!(a.blocks_slice(), b.blocks_slice(), "blocks differ at {cx},{cz}");
            assert_eq!(a.biomes_slice(), b.biomes_slice(), "biomes differ at {cx},{cz}");
        }
    }

    #[test]
    fn features_occupy_chunk_edges() {
        // P4 removed the chunk-edge skip: trees may now sit on the border.
        let seed = 1u32;
        let mut found = false;
        'scan: for cz in 0..24 {
            for cx in 0..24 {
                let c = generate_chunk(seed, cx, cz);
                for z in 0..CHUNK_SZ {
                    for x in 0..CHUNK_SX {
                        let edge = x == 0 || x == CHUNK_SX - 1 || z == 0 || z == CHUNK_SZ - 1;
                        if !edge {
                            continue;
                        }
                        for y in 0..CHUNK_SY {
                            if is_tree(c.block_raw(x, y, z)) {
                                found = true;
                                break 'scan;
                            }
                        }
                    }
                }
            }
        }
        assert!(found, "no tree blocks on any chunk edge — edge-skip not removed?");
    }

    #[test]
    fn trees_span_chunk_seams() {
        // A trunk rooted on the west border of chunk (cx,cz) (world x = cx*16)
        // must have canopy reaching into the previous chunk's east column
        // (local x = 15). Any one confirmed seam-spanning tree proves the
        // cross-chunk feature mechanism (no bald seam, no gap).
        for seed in [1u32, 7, 13, 42, 0x1234_5678] {
            for cz in 0..6 {
                for cx in 1..6 {
                    let west = generate_chunk(seed, cx - 1, cz);
                    let east = generate_chunk(seed, cx, cz);
                    for z in 0..CHUNK_SZ {
                        for y in 2..CHUNK_SY - 2 {
                            if east.block_raw(0, y, z) != Block::OakLog.id() {
                                continue;
                            }
                            // Canopy of this trunk should reach the west chunk's
                            // x = 15 column near (y.., z..).
                            let z_lo = z.saturating_sub(2);
                            let z_hi = (z + 3).min(CHUNK_SZ);
                            for yy in y..(y + 8).min(CHUNK_SY) {
                                for zz in z_lo..z_hi {
                                    if is_tree(west.block_raw(15, yy, zz)) {
                                        return; // seam-spanning tree confirmed
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        panic!("no seam-spanning tree found in the sampled region");
    }
}
