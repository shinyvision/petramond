//! Foliage placers — build a tree's leaves around the trunk's attach points.
//!
//! Canopies follow the canonical Minecraft oak silhouette: two wide layers with
//! trimmed corners, a narrower layer, and a small cap — not a sphere. Iteration
//! order and the per-cell RNG draws are fixed so cross-chunk seam replay stays
//! deterministic (a tree rooted in a neighbour materialises identically here).

use crate::block::Block;
use crate::mathh::IVec3;
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

/// Place a square leaf layer of the given radius at world Y `y`, centred on
/// `(cx, cz)`. Hard corners (|lx|==|lz|==radius) are always cut; outer-ring cells
/// are trimmed with probability `ragged` for a natural edge. Over Air/Water only.
fn leaf_layer(
    ctx: &mut FeatureCtx,
    cx: i32,
    y: i32,
    cz: i32,
    radius: i32,
    leaf: Block,
    ragged: f32,
    rng: &mut FeatureRng,
) {
    for lx in -radius..=radius {
        for lz in -radius..=radius {
            if lx.abs() == radius && lz.abs() == radius {
                continue; // cut hard corners
            }
            let outer = lx.abs() == radius || lz.abs() == radius;
            if outer && ragged > 0.0 && rng.chance(ragged) {
                continue; // ragged edge
            }
            ctx.set_leaf(IVec3::new(cx + lx, y, cz + lz), leaf);
        }
    }
}

/// Canonical Minecraft oak canopy (vanilla `BlobFoliagePlacer`, radius 2).
///
/// Four leaf layers attach one block ABOVE the top log (`topY = topLog + 1`) and
/// run `topY-3 ..= topY`. Per-layer radius, bottom→top, is `[r, r, r-1, r-1]`:
/// the two BOTTOM layers are the wide 5×5 squares, the two top layers are 3×3 —
/// a fuller, rounder blob than a single wide layer. The ONLY cells ever removed
/// are the four extreme corners (`|dx|==r && |dz|==r`): always on the very top
/// layer, 50% on the others. (We deliberately do NOT trim the whole outer ring —
/// that over-thins the canopy into the scraggly look this replaces.)
pub struct CanopyOakFoliage;

impl FoliagePlacer for CanopyOakFoliage {
    fn place(
        &self,
        ctx: &mut FeatureCtx,
        attach: &[IVec3],
        radius: i32,
        leaf: Block,
        rng: &mut FeatureRng,
    ) {
        let a = attach[0];
        let r = radius.max(1);
        let rn = (r - 1).max(1); // narrowed (upper) radius
        let top_y = a.y + 1; // leaves attach one block above the highest log
        // (dy from top, layer radius) bottom→top: wide, wide, narrow, narrow.
        let layers = [(-3, r), (-2, r), (-1, rn), (0, rn)];
        for (dy, lr) in layers {
            let y = top_y + dy;
            let is_top = dy == 0;
            for lx in -lr..=lr {
                for lz in -lr..=lr {
                    if lx.abs() == lr && lz.abs() == lr {
                        // Only the 4 extreme corners are removable: always on the
                        // top layer, 50% elsewhere (one draw per corner cell).
                        if is_top || rng.chance(0.5) {
                            continue;
                        }
                    }
                    ctx.set_leaf(IVec3::new(a.x + lx, y, a.z + lz), leaf);
                }
            }
        }
    }
}

/// Droopy swamp canopy: a wide flat layer at the trunk top, a small cap above,
/// and leaves that hang one block down from the outer ring (the swamp "drip").
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
        let (cx, cz, ct) = (a.x, a.z, a.y);
        // Wide main layer + a small cap.
        leaf_layer(ctx, cx, ct, cz, 2, leaf, 0.15, rng);
        leaf_layer(ctx, cx, ct + 1, cz, 1, leaf, 0.0, rng);
        // Hanging drips: from each outer-ring cell of the main layer, sometimes
        // extend one leaf straight down.
        for lx in -2i32..=2 {
            for lz in -2i32..=2 {
                if lx.abs() == 2 && lz.abs() == 2 {
                    continue;
                }
                if !(lx.abs() == 2 || lz.abs() == 2) {
                    continue; // outer ring only
                }
                if rng.chance(0.45) {
                    continue;
                }
                ctx.set_leaf(IVec3::new(cx + lx, ct - 1, cz + lz), leaf);
            }
        }
    }
}

/// Flat sparse savanna canopy (acacia-like silhouette using oak blocks): a thin
/// diamond umbrella spread above a tall trunk, with gaps so it reads as airy.
pub struct FlatSparseFoliage;

impl FoliagePlacer for FlatSparseFoliage {
    fn place(
        &self,
        ctx: &mut FeatureCtx,
        attach: &[IVec3],
        _radius: i32,
        leaf: Block,
        rng: &mut FeatureRng,
    ) {
        let a = attach[0];
        let (cx, cz, ct) = (a.x, a.z, a.y);
        // Upper disc: diamond radius 3, sparse.
        for lx in -3i32..=3 {
            for lz in -3i32..=3 {
                if lx.abs() + lz.abs() > 3 {
                    continue;
                }
                if rng.chance(0.30) {
                    continue;
                }
                ctx.set_leaf(IVec3::new(cx + lx, ct + 1, cz + lz), leaf);
            }
        }
        // Lower ring: diamond radius 2, sparser.
        for lx in -2i32..=2 {
            for lz in -2i32..=2 {
                if lx.abs() + lz.abs() > 2 {
                    continue;
                }
                if rng.chance(0.40) {
                    continue;
                }
                ctx.set_leaf(IVec3::new(cx + lx, ct, cz + lz), leaf);
            }
        }
    }
}
