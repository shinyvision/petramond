//! Composable feature system — replaces the bespoke `trees::oak_*` functions.
//!
//! A feature is split into reusable, data-driven pieces:
//!   - `Feature`        — the imperative voxel-writing shape (e.g. `TreeFeature`)
//!   - `TrunkPlacer` / `FoliagePlacer` — reusable sub-shapes a tree composes
//!   - `ConfiguredFeature` — a feature + baked params (the oaks are rows)
//!
//! Strata P3: the abstraction is established and the oaks become data, but the
//! per-column placement loop reproduces the god file's exact two-roll
//! (`tree_probability` chance → `pick_oak_variant` `next_i32(0,99)`) and every
//! placer mirrors its original RNG draw order and block-write order, so output
//! is byte-parity under the unchanged per-chunk xorshift64 stream.

pub mod placers;
pub mod scatter;
pub mod tree;
pub mod vegetation;

mod field;
pub mod placement;
mod plan;
mod sink;
mod tree_select;
pub(crate) use plan::FeaturePlan;
/// The tree origin loop over one column's footprint, for planning.
pub(crate) use tree_select::place_feature_origins as place_trees;

#[cfg(test)]
mod tests;

pub use self::field::{
    cached_feature_region, cached_tile_biomes, ColumnFeatureField, FeatureField, SurfaceHeights,
};
pub(crate) use self::field::{cached_tile_raw, RegionTile, TileKey};
pub use self::sink::*;

use petramond_world::block::Block;
use petramond_world::chunk::CHUNK_SX;
use petramond_world::mathh::IVec3;

use self::tree::REDWOOD_BASE_SUPPORT_REACH;
use super::biome;
use super::rng::FeatureRng;

/// Border (in blocks) considered around a column for cross-column feature
/// placement: the tree pass derives feature origins in `[-MARGIN, 16+MARGIN)`
/// so a tree rooted in a neighbour can write its overlapping voxels here.
///
/// Feature writes are clipped to the target's own footprint (see
/// `FeatureCtx`), so no wider buffer is needed: an in-footprint write only
/// ever reads in-footprint cells, and a feature whose footprint <= MARGIN is
/// materialised identically by every section that owns part of it
/// (seam-consistent, no double-placement). MARGIN is sized to the widest
/// feature: the grand oak's fenced branch tips (`tree::TIP_FENCE` = 11) plus
/// its widest leaf-clump overhang (satellite box offset 2 + half-extent 3).
/// Redwoods (branch reach + leaf blob ≤ 10) fit inside that. Raising this
/// widens every column's candidate scan and padded surface region — re-time
/// generation when it changes.
pub const MARGIN: i32 = 16;

/// Highest surface a tree will root on — above this (bare snow/stone peaks) the
/// canopy is left off regardless of biome.
pub const TREELINE: i32 = 118;

/// Worst-case vertical reach of a tree ABOVE its root anchor, used to bound which
/// cubic sections a column's features can touch. The tallest tree (redwood) has a
/// height-clearance of 56; the crown / leaf blobs add a few more, so 64 is a safe
/// over-estimate. Trees never write BELOW their anchor (every trunk placer starts at
/// the anchor and builds up), so there is no matching downward reach.
pub const MAX_TREE_REACH_ABOVE: i32 = 64;

pub fn feature_region_bounds(ox: i32, oz: i32) -> (i32, i32, usize, usize) {
    let pad = MARGIN + biome::trees::MAX_TREE_SPACING_RADIUS + REDWOOD_BASE_SUPPORT_REACH;
    feature_bounds_with_pad(ox, oz, pad)
}

pub fn feature_candidate_bounds(ox: i32, oz: i32) -> (i32, i32, usize, usize) {
    let pad = MARGIN + biome::trees::MAX_TREE_SPACING_RADIUS;
    feature_bounds_with_pad(ox, oz, pad)
}

fn feature_bounds_with_pad(ox: i32, oz: i32, pad: i32) -> (i32, i32, usize, usize) {
    let w = (CHUNK_SX as i32 + 2 * pad) as usize;
    (ox - pad, oz - pad, w, w)
}

/// A worldgen feature: imperatively writes voxels around a world origin.
pub trait Feature: Send + Sync {
    /// `open` answers whether a world cell may hold a canopy leaf, or route
    /// leaf-support through one (the `TreeFeature` canopies gate and
    /// connectivity-flood their leaves through it; implementations that
    /// guarantee support another way — the oak's hidden clump wood — may
    /// ignore it). The caller must supply a DETERMINISTIC oracle whose answers
    /// are identical for every chunk that replays the feature: worldgen passes
    /// the world-anchored cave-adjusted surface model (`p.y > surf(p.x, p.z)`
    /// — reads fenced inside the candidate window by the load-time reach
    /// check in `data::features`), runtime growth passes a live-world
    /// occupancy read.
    fn generate(
        &self,
        ctx: &mut FeatureCtx,
        open: &mut dyn FnMut(IVec3) -> bool,
        origin: IVec3,
        rng: &mut FeatureRng,
    );

    /// Ground-anchoring gate, consulted for an ACCEPTED origin just before
    /// `generate`: return false to skip the feature at this site entirely
    /// (e.g. oak roots that would hang over a drop). `surf` is the
    /// cave-adjusted generation surface per column; `rng` is a COPY of the
    /// stream `generate` will receive (positioned right after the variant
    /// pick), so an implementation may dry-run its draw prefix. Must read
    /// only `surf` and the rng — never chunk content — and must stay within
    /// `MAX_TREE_SPACING_RADIUS` of the origin so the candidate window covers
    /// every read on both placement paths. Default: anchored everywhere.
    fn is_anchored(
        &self,
        surf: &mut dyn FnMut(i32, i32) -> i32,
        origin: IVec3,
        rng: FeatureRng,
    ) -> bool {
        let _ = (surf, origin, rng);
        true
    }
}

/// A feature plus its baked parameters.
pub struct ConfiguredFeature {
    pub feature: &'static dyn Feature,
}

/// A distinct, inert species for tests that only care WHICH feature a
/// selection returned, never what it writes.
#[cfg(test)]
pub(crate) fn stub_species() -> &'static ConfiguredFeature {
    struct Inert;
    impl Feature for Inert {
        fn generate(
            &self,
            _: &mut FeatureCtx,
            _: &mut dyn FnMut(IVec3) -> bool,
            _: IVec3,
            _: &mut FeatureRng,
        ) {
        }
    }
    Box::leak(Box::new(ConfiguredFeature {
        feature: Box::leak(Box::new(Inert)),
    }))
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

    /// Write over Air/Water, a fragile plant, or a snow blanket.
    ///
    /// Worldgen dresses the ground (vegetation, snow) BEFORE trees run, so a
    /// canopy cell can already hold a tuft, flower or snow layer; refusing
    /// those punched permanent holes in generated canopies and stranded the
    /// leaves behind the hole for the decay flood. Ground cover yields to a
    /// growing tree, exactly as it always yielded to trunk and root wood
    /// (`set_log` is unconditional). Still reads only the cell it writes, so
    /// it stays seam-safe.
    pub fn set_leaf(&mut self, p: IVec3, b: Block) {
        self.sink.place(p, b, PlacementRule::Leaf);
    }

    /// Write over Air/leaves/Water (== branch predicate). A branch may pass
    /// through leaves placed earlier by its own crown or a neighbouring canopy.
    pub fn set_branch(&mut self, p: IVec3, b: Block) {
        self.sink.place(p, b, PlacementRule::Branch);
    }

    /// Write over Air/Water or a SNOW BLANKET (== ground-litter predicate).
    ///
    /// Litter lands on the ground after the vegetation pass has already dressed
    /// the column, so it must decide what it is allowed to displace. A tuft or
    /// a flower it must not — burying those is what [`Self::set_leaf`] exists to
    /// prevent. A snow layer it must, because in a `SnowCover::Always` biome
    /// EVERY column carries one, so refusing it means no branch ever lands in a
    /// snowy forest at all — measured as literally zero over 400 chunks before
    /// this existed, in a biome a fresh player can spawn in.
    pub fn set_ground_litter(&mut self, p: IVec3, b: Block) {
        self.sink.place(p, b, PlacementRule::Litter);
    }

    /// Replace a voxel only when it currently holds one of `hosts`. Used by
    /// the underground ore / stone-blob veins, which overwrite their host rock
    /// (and never air, dirt, or an already-placed ore unless listed). World
    /// coords; clipped to this chunk.
    pub fn replace_block(&mut self, p: IVec3, hosts: &'static [Block], b: Block) {
        self.sink.place(p, b, PlacementRule::Replace(hosts));
    }
}
