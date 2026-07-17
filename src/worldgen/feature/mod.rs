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
mod sink;
mod tree_select;

#[cfg(all(test, feature = "worldgen-tests"))]
mod tests;

pub(crate) use self::field::{
    cached_feature_region, ColumnFeatureField, FeatureField, RuntimeFeatureField, SurfaceHeights,
};
pub use self::sink::*;
pub(crate) use self::tree_select::{place_features_section, place_features_with_field};

use crate::block::Block;
use crate::chunk::CHUNK_SX;
use crate::mathh::IVec3;

use self::tree::REDWOOD_BASE_SUPPORT_REACH;
use super::biome;
use super::proto;
use super::rng::FeatureRng;

/// Highest surface a tree will root on — above this (bare snow/stone peaks) the
/// canopy is left off regardless of biome.
pub(crate) const TREELINE: i32 = 118;

/// Worst-case vertical reach of a tree ABOVE its root anchor, used to bound which
/// cubic sections a column's features can touch. The tallest tree (redwood) has a
/// height-clearance of 56; the crown / leaf blobs add a few more, so 64 is a safe
/// over-estimate. Trees never write BELOW their anchor (every trunk placer starts at
/// the anchor and builds up), so there is no matching downward reach.
pub(crate) const MAX_TREE_REACH_ABOVE: i32 = 64;

pub(crate) fn feature_region_bounds(ox: i32, oz: i32) -> (i32, i32, usize, usize) {
    let pad = super::proto::MARGIN + biome::MAX_TREE_SPACING_RADIUS + REDWOOD_BASE_SUPPORT_REACH;
    feature_bounds_with_pad(ox, oz, pad)
}

pub(crate) fn feature_candidate_bounds(ox: i32, oz: i32) -> (i32, i32, usize, usize) {
    let pad = super::proto::MARGIN + biome::MAX_TREE_SPACING_RADIUS;
    feature_bounds_with_pad(ox, oz, pad)
}

fn feature_bounds_with_pad(ox: i32, oz: i32, pad: i32) -> (i32, i32, usize, usize) {
    let w = (CHUNK_SX as i32 + 2 * pad) as usize;
    (ox - pad, oz - pad, w, w)
}

/// A worldgen feature: imperatively writes voxels around a world origin.
pub trait Feature: Send + Sync {
    fn generate(&self, ctx: &mut FeatureCtx, origin: IVec3, rng: &mut FeatureRng);

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

    /// Write over Air/leaves/Water (== branch predicate). A branch may pass
    /// through leaves placed earlier by its own crown or a neighbouring canopy.
    pub fn set_branch(&mut self, p: IVec3, b: Block) {
        let c = self.sink.get(p);
        if c == Block::Air || c.is_leaves() || c == Block::Water {
            self.sink.set(p, b);
        }
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
