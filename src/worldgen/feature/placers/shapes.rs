//! Raw voxel-shape primitives shared by placers and features.

use crate::block::Block;
use crate::mathh::IVec3;
use crate::worldgen::feature::FeatureCtx;

/// Spherical-ish leaf blob centered at `center` (== `trees::leaf_blob`).
/// Loop order (ly, lx, lz) and the radius test are preserved for parity.
pub fn leaf_blob(
    ctx: &mut FeatureCtx,
    center: IVec3,
    radius: i32,
    leaf: Block,
    allow_overwrite: bool,
) {
    let r = radius;
    for ly in -r..=r {
        for lx in -r..=r {
            for lz in -r..=r {
                let d2 = lx * lx + ly * ly + lz * lz;
                if d2 > r * r + 1 {
                    continue;
                }
                if d2 > r * r - 1 && (lx.abs() == r || lz.abs() == r || ly.abs() == r) {
                    continue;
                }
                let p = IVec3::new(center.x + lx, center.y + ly, center.z + lz);
                if allow_overwrite {
                    ctx.set_leaf_force(p, leaf);
                } else {
                    ctx.set_leaf(p, leaf);
                }
            }
        }
    }
}
