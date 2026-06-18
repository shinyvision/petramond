//! Foliage placers — build a tree's leaves around the trunk's attach points.
//! Each mirrors the foliage half of one god-file oak, draw-for-draw.

use crate::block::Block;
use crate::mathh::IVec3;
use crate::worldgen::feature::placers::shapes;
use crate::worldgen::feature::FeatureCtx;
use crate::worldgen::rng::FeatureRng;

pub trait FoliagePlacer: Send + Sync {
    fn place(
        &self,
        ctx: &mut FeatureCtx,
        attach: &[IVec3],
        radius: i32,
        leaf: Block,
        rng: &mut FeatureRng,
    );
}

/// Simple spherical blob at each attach point (== `oak_simple` canopy). No draws.
pub struct BlobFoliage;

impl FoliagePlacer for BlobFoliage {
    fn place(
        &self,
        ctx: &mut FeatureCtx,
        attach: &[IVec3],
        radius: i32,
        leaf: Block,
        _rng: &mut FeatureRng,
    ) {
        for &a in attach {
            shapes::leaf_blob(ctx, a, radius, leaf, false);
        }
    }
}

/// Wider asymmetric canopy with a random horizontal offset (== `oak_canopy_offset`).
/// Draws dx, dz, then a `chance(0.5)` per canopy corner cell.
pub struct OffsetBlobFoliage;

impl FoliagePlacer for OffsetBlobFoliage {
    fn place(
        &self,
        ctx: &mut FeatureCtx,
        attach: &[IVec3],
        _radius: i32,
        leaf: Block,
        rng: &mut FeatureRng,
    ) {
        let a = attach[0];
        let dx = rng.next_i32(-1, 1);
        let dz = rng.next_i32(-1, 1);
        let top = a.y;
        for ly in -1i32..=2 {
            let r: i32 = if ly <= 0 { 2 } else { 1 };
            for lx in -r..=r {
                for lz in -r..=r {
                    if lx == 0 && lz == 0 && ly < 2 {
                        continue;
                    }
                    if (lx.abs() == r && lz.abs() == r) && rng.chance(0.5) {
                        continue;
                    }
                    ctx.set_leaf(
                        IVec3::new(a.x + lx + dx * (ly / 2), top + ly, a.z + lz + dz * (ly / 2)),
                        leaf,
                    );
                }
            }
        }
    }
}

/// Droopy swamp canopy: a sparse top cap plus a lower drooping layer
/// (== `oak_swamp`). Draws `chance(0.3)` per cap cell, `chance(0.6)` per droop cell.
pub struct DroopyFoliage;

impl FoliagePlacer for DroopyFoliage {
    fn place(
        &self,
        ctx: &mut FeatureCtx,
        attach: &[IVec3],
        _radius: i32,
        leaf: Block,
        rng: &mut FeatureRng,
    ) {
        let a = attach[0];
        let top = a.y;
        // Top small cap.
        for lx in -1i32..=1 {
            for lz in -1i32..=1 {
                if lx == 0 && lz == 0 {
                    continue;
                }
                if rng.chance(0.3) {
                    continue;
                }
                ctx.set_leaf(IVec3::new(a.x + lx, top + 1, a.z + lz), leaf);
            }
        }
        // Droopy lower layer.
        for lx in -2i32..=2 {
            for lz in -2i32..=2 {
                if lx.abs() == 2 && lz.abs() == 2 {
                    continue;
                }
                if rng.chance(0.6) {
                    continue;
                }
                ctx.set_leaf(IVec3::new(a.x + lx, top - 1, a.z + lz), leaf);
            }
        }
        ctx.set_leaf(IVec3::new(a.x, top + 1, a.z), leaf);
    }
}
