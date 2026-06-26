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
pub mod scatter;
pub mod tree;
pub mod vegetation;

use crate::biome::Biome;
use crate::block::Block;
use crate::chunk::{Chunk, CHUNK_SX, CHUNK_SY, CHUNK_SZ, SEA_LEVEL};
use crate::mathh::IVec3;

use super::classic::world::{map_biome, CascadeWorld, RegionCells};
use super::data;
use super::rng::FeatureRng;

/// Highest surface a tree will root on — above this (bare snow/stone peaks) the
/// canopy is left off regardless of biome.
const TREELINE: i32 = 118;

/// Biome-driven surface + biome over the chunk plus the feature margin (and the
/// spacing-rule neighbourhood), computed in ONE cascade region pass. The driver
/// reuses the result for terrain fill too.
pub fn feature_region(world: &CascadeWorld, ox: i32, oz: i32) -> RegionCells {
    let pad = super::proto::MARGIN + TREE_SPACING_RADIUS;
    let w = (CHUNK_SX as i32 + 2 * pad) as usize;
    world.region(ox - pad, oz - pad, w, w)
}

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

/// A destination a feature paints voxels into. Abstracting WHERE the writes land
/// lets the SAME `Feature` / placer code drive two callers: worldgen, which writes
/// into one [`Chunk`] clipped to its footprint ([`ChunkSink`]), and runtime sapling
/// growth, which writes into the live `World` through a validating overlay (see
/// `world::sapling`). `get` returns the sink's CURRENT occupant so the overwrite
/// predicates on [`FeatureCtx`] see a feature's own earlier writes; it reads `Air`
/// for any cell the sink can't address.
pub trait VoxelSink {
    fn get(&self, p: IVec3) -> Block;
    fn set(&mut self, p: IVec3, b: Block);
}

/// Worldgen voxel sink: writes into one [`Chunk`], in WORLD coords clipped to the
/// chunk's own `[0,16)×[0,16)×[0,256)` footprint. Out-of-footprint writes are
/// dropped and out-of-footprint reads return `Air`. That clipping IS the seam
/// mechanism: because every retained write only reads in-chunk cells, a feature
/// rooted in a neighbour materialises its overlapping voxels here identically to
/// how the owner chunk does — seam-consistent cross-chunk features with no shared
/// buffer.
pub struct ChunkSink<'a> {
    chunk: &'a mut Chunk,
    ox: i32,
    oz: i32,
}

impl<'a> ChunkSink<'a> {
    pub fn new(chunk: &'a mut Chunk) -> Self {
        let (ox, oz) = chunk.chunk_origin_world();
        Self { chunk, ox, oz }
    }

    /// Map a world position to in-chunk local indices, or `None` if outside.
    #[inline]
    fn local(&self, p: IVec3) -> Option<(usize, usize, usize)> {
        let lx = p.x - self.ox;
        let lz = p.z - self.oz;
        if !(0..16).contains(&lx) || !(0..16).contains(&lz) || p.y < 0 || p.y >= CHUNK_SY as i32 {
            return None;
        }
        Some((lx as usize, p.y as usize, lz as usize))
    }
}

impl VoxelSink for ChunkSink<'_> {
    #[inline]
    fn get(&self, p: IVec3) -> Block {
        match self.local(p) {
            Some((x, y, z)) => self.chunk.block(x, y, z),
            None => Block::Air,
        }
    }
    #[inline]
    fn set(&mut self, p: IVec3, b: Block) {
        if let Some((x, y, z)) = self.local(p) {
            self.chunk.set_block_raw(x, y, z, b.id());
        }
    }
}

/// Bounded voxel writer — the ONLY place imperative feature writes happen. Holds a
/// `&mut dyn VoxelSink` so one set of placer code targets either a chunk (worldgen)
/// or the world (growth). The overwrite predicates (`set_leaf` over air/water,
/// `set_branch` over air/leaves/water, `replace_block` over an expected block) read
/// the sink's CURRENT occupant, so a feature's own earlier writes are honoured.
/// Reproduces the god file's three overwrite predicates
/// (`log_at`/`leaf_at`/`oak_big`-branch).
pub struct FeatureCtx<'a> {
    sink: &'a mut dyn VoxelSink,
}

impl<'a> FeatureCtx<'a> {
    pub fn new(sink: &'a mut dyn VoxelSink) -> Self {
        Self { sink }
    }

    /// Unconditional write (== `trees::log_at`).
    pub fn set_log(&mut self, p: IVec3, b: Block) {
        self.sink.set(p, b);
    }

    /// Write over Air/Water only (== `trees::leaf_at`).
    pub fn set_leaf(&mut self, p: IVec3, b: Block) {
        let c = self.sink.get(p);
        if c == Block::Air || c == Block::Water {
            self.sink.set(p, b);
        }
    }

    /// Write over Air/OakLeaves/Water (== `oak_big` branch predicate). Only the
    /// giant oak draws branches, and it is an oak, so the tolerated leaf is the
    /// literal `OakLeaves` (a branch may pass through the crown's own leaves).
    pub fn set_branch(&mut self, p: IVec3, b: Block) {
        let c = self.sink.get(p);
        if c == Block::Air || c == Block::OakLeaves || c == Block::Water {
            self.sink.set(p, b);
        }
    }

    /// Unconditional leaf write (== `leaf_blob` with `allow_overwrite = true`).
    pub fn set_leaf_force(&mut self, p: IVec3, b: Block) {
        self.sink.set(p, b);
    }

    /// Replace a voxel only when it currently equals `expect`. Used by the
    /// underground ore / stone-blob veins, which overwrite Stone (and never air,
    /// dirt, or an already-placed ore). World coords; clipped to this chunk.
    pub fn replace_block(&mut self, p: IVec3, expect: Block, b: Block) {
        if self.sink.get(p) == expect {
            self.sink.set(p, b);
        }
    }
}

/// Salt distinguishing the tree-feature positional RNG stream from other users.
const FEATURE_SALT: u64 = 0x0000_7A3E_0AC0_FFEE;
/// Minimum horizontal block radius between generated tree origins.
pub(crate) const TREE_SPACING_RADIUS: i32 = 3;
/// Separate stream used only to break ties between nearby tree candidates.
const TREE_PRIORITY_SALT: u64 = 0x0000_7A3E_51AC_1EAF;

#[derive(Copy, Clone)]
struct TreeCandidate {
    anchor: i32,
    biome: Biome,
    density: f32,
    priority: u64,
}

#[inline]
fn tree_priority(seed: u32, wx: i32, wz: i32) -> u64 {
    FeatureRng::positional(seed, TREE_PRIORITY_SALT, wx, 0, wz).next_u64()
}

#[inline]
fn tree_candidate_beats(
    lhs_priority: u64,
    lhs_wx: i32,
    lhs_wz: i32,
    rhs_priority: u64,
    rhs_wx: i32,
    rhs_wz: i32,
) -> bool {
    lhs_priority > rhs_priority
        || (lhs_priority == rhs_priority && (lhs_wz, lhs_wx) < (rhs_wz, rhs_wx))
}

fn tree_candidate_at(field: &RegionCells, seed: u32, wx: i32, wz: i32) -> Option<TreeCandidate> {
    // Anchor on the final region surface. Ocean and wet river-channel columns sit
    // at/below their waterline, so the water guard keeps trees off them.
    let (surf, biome_id) = field.at(wx, wz);
    let anchor = surf;
    if anchor <= SEA_LEVEL || surf > TREELINE {
        return None;
    }
    // place_oak height guard (origin too low / too near the world top).
    if anchor < 1 || anchor + 14 >= CHUNK_SY as i32 {
        return None;
    }

    let biome = map_biome(biome_id);
    let density = data::features::tree_density(biome);
    if density <= 0.0 {
        return None;
    }

    let mut rng = FeatureRng::positional(seed, FEATURE_SALT, wx, 0, wz);
    if !rng.chance(density) {
        return None;
    }

    Some(TreeCandidate {
        anchor,
        biome,
        density,
        priority: tree_priority(seed, wx, wz),
    })
}

fn tree_spacing_allows(
    candidate: TreeCandidate,
    field: &RegionCells,
    seed: u32,
    wx: i32,
    wz: i32,
) -> bool {
    for dz in -TREE_SPACING_RADIUS..=TREE_SPACING_RADIUS {
        for dx in -TREE_SPACING_RADIUS..=TREE_SPACING_RADIUS {
            if dx == 0 && dz == 0 {
                continue;
            }
            let nx = wx + dx;
            let nz = wz + dz;
            if let Some(other) = tree_candidate_at(field, seed, nx, nz) {
                if tree_candidate_beats(other.priority, nx, nz, candidate.priority, wx, wz) {
                    return false;
                }
            }
        }
    }
    true
}

/// Per-chunk feature placement (P4). Iterates feature origins across the chunk
/// plus a `MARGIN` border, in canonical (wz, wx) order, so a tree rooted in a
/// neighbour that reaches into this chunk is generated here too. Each origin
/// seeds its OWN positional RNG (`FeatureRng::positional`), so the per-biome
/// density roll, variant pick, and geometry are pure functions of (seed, wx, wz)
/// — independent of chunk and order. Candidate origins are then thinned by a
/// deterministic three-block spacing rule. Features write in world coords and
/// are clipped to this chunk, so seams are continuous with no double-placement
/// and the old chunk-edge skip is gone.
pub fn place_features(chunk: &mut Chunk, field: &RegionCells, seed: u32) {
    let (ox, oz) = chunk.chunk_origin_world();
    let mut sink = ChunkSink::new(chunk);
    let mut ctx = FeatureCtx::new(&mut sink);
    let margin = super::proto::MARGIN;

    for wz in (oz - margin)..(oz + CHUNK_SZ as i32 + margin) {
        for wx in (ox - margin)..(ox + CHUNK_SX as i32 + margin) {
            let Some(candidate) = tree_candidate_at(field, seed, wx, wz) else {
                continue;
            };

            if !tree_spacing_allows(candidate, field, seed, wx, wz) {
                continue;
            }

            // Recreate the accepted origin's stream and consume the already-proven
            // density roll so variant and geometry draws stay on the tree stream.
            let mut rng = FeatureRng::positional(seed, FEATURE_SALT, wx, 0, wz);
            let _density_hit = rng.chance(candidate.density);
            debug_assert!(_density_hit);
            let cf = data::features::pick_oak(&mut rng, candidate.biome);
            cf.feature
                .generate(&mut ctx, IVec3::new(wx, candidate.anchor, wz), &mut rng);
        }
    }
}

#[cfg(all(test, feature = "worldgen-tests"))]
mod tests {
    use super::{feature_region, tree_candidate_at, tree_spacing_allows, TREE_SPACING_RADIUS};
    use crate::biome::Biome;
    use crate::block::Block;
    use crate::chunk::{CHUNK_SX, CHUNK_SY, CHUNK_SZ};
    use crate::worldgen::classic::world::CascadeWorld;
    use crate::worldgen::data;
    use crate::worldgen::generate_chunk;
    use crate::worldgen::river::RiverSystem;

    fn is_tree(id: u8) -> bool {
        id == Block::OakLog.id() || id == Block::OakLeaves.id()
    }

    fn accepted_tree_origins(seed: u32, chunk_radius: i32) -> Vec<(i32, i32)> {
        let world = CascadeWorld::new(seed);
        let rivers = RiverSystem::new(seed);
        let mut origins = Vec::new();

        for cz in -chunk_radius..=chunk_radius {
            for cx in -chunk_radius..=chunk_radius {
                let ox = cx * CHUNK_SX as i32;
                let oz = cz * CHUNK_SZ as i32;
                let mut field = feature_region(&world, ox, oz);
                rivers.apply(&mut field);
                for wz in oz..(oz + CHUNK_SZ as i32) {
                    for wx in ox..(ox + CHUNK_SX as i32) {
                        let Some(candidate) = tree_candidate_at(&field, seed, wx, wz) else {
                            continue;
                        };
                        if tree_spacing_allows(candidate, &field, seed, wx, wz) {
                            origins.push((wx, wz));
                        }
                    }
                }
            }
        }

        origins
    }

    #[test]
    fn forest_like_tree_density_values_are_thinned() {
        assert_eq!(data::features::tree_density(Biome::Forest), 0.055);
        assert_eq!(data::features::tree_density(Biome::BirchForest), 0.045);
        assert_eq!(data::features::tree_density(Biome::Taiga), 0.026);
        assert_eq!(data::features::tree_density(Biome::SnowyTaiga), 0.020);
        assert_eq!(data::features::tree_density(Biome::Jungle), 0.070);
        assert_eq!(data::features::tree_density(Biome::DarkForest), 0.075);
    }

    #[test]
    fn tree_origin_spacing_rule_enforces_three_block_radius() {
        // Sample a wide area so we cross enough forested biomes to collect a
        // meaningful tree population (near origin many regions are plains/ocean).
        for seed in [1u32, 7, 42, 0x1234_5678] {
            let origins = accepted_tree_origins(seed, 10);
            assert!(
                origins.len() > 10,
                "spacing test sampled too few tree origins for seed {seed:#x}"
            );

            for i in 0..origins.len() {
                for j in (i + 1)..origins.len() {
                    let (ax, az) = origins[i];
                    let (bx, bz) = origins[j];
                    let dx = (ax - bx).abs();
                    let dz = (az - bz).abs();
                    assert!(
                        dx > TREE_SPACING_RADIUS || dz > TREE_SPACING_RADIUS,
                        "tree origins ({ax},{az}) and ({bx},{bz}) are within {TREE_SPACING_RADIUS} blocks"
                    );
                }
            }
        }
    }

    #[test]
    fn generate_chunk_is_deterministic() {
        let seed = 0x1234_5678;
        for &(cx, cz) in &[(0, 0), (3, -2), (-5, 7), (12, 9)] {
            let a = generate_chunk(seed, cx, cz);
            let b = generate_chunk(seed, cx, cz);
            assert_eq!(
                a.blocks_slice(),
                b.blocks_slice(),
                "blocks differ at {cx},{cz}"
            );
            assert_eq!(
                a.biomes_slice(),
                b.biomes_slice(),
                "biomes differ at {cx},{cz}"
            );
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
        assert!(
            found,
            "no tree blocks on any chunk edge — edge-skip not removed?"
        );
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
