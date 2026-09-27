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

pub const TREELINE: i32 = 118;

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

    /// Checked for an ACCEPTED origin right before `generate`, false skips the feature (oak roots
    /// hanging over a drop, say). `surf` is the cave-adjusted surface per column, `rng` is a copy
    /// of generate's rng already past the variant pick, so you can dry-run draws against it.
    /// Touch only `surf` and `rng`, no chunk reads. Stay inside `MAX_TREE_SPACING_RADIUS` or the
    /// candidate window misses reads from one of the placement paths.
    /// Default: anchored everywhere.
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

pub struct ConfiguredFeature {
    pub feature: &'static dyn Feature,
}

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

pub struct FeatureCtx<'a> {
    sink: &'a mut dyn VoxelSink,
}

impl<'a> FeatureCtx<'a> {
    pub fn new(sink: &'a mut dyn VoxelSink) -> Self {
        Self { sink }
    }

    pub fn set_log(&mut self, p: IVec3, b: Block) {
        self.sink.set(p, b);
    }

    pub fn set_leaf(&mut self, p: IVec3, b: Block) {
        self.sink.place(p, b, PlacementRule::Leaf);
    }

    pub fn set_branch(&mut self, p: IVec3, b: Block) {
        self.sink.place(p, b, PlacementRule::Branch);
    }

    /// Write over Air/Water or a SNOW BLANKET (== ground-litter predicate).
    ///
    /// Litter lands on the ground after the vegetation pass has already dressed
    /// the column, so it must decide what it is allowed to displace. A tuft or
    /// a flower it must not — burying those is what [`Self::set_leaf`] exists to
    /// prevent. A snow layer it must, because in a `SnowCover::Always` biome
    /// every column carries one, so refusing it would prevent branches from
    /// landing in a snowy forest where a fresh player can spawn.
    pub fn set_ground_litter(&mut self, p: IVec3, b: Block) {
        self.sink.place(p, b, PlacementRule::Litter);
    }

    pub fn replace_block(&mut self, p: IVec3, hosts: &'static [Block], b: Block) {
        self.sink.place(p, b, PlacementRule::Replace(hosts));
    }
}
