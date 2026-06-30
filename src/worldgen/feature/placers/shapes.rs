//! Raw voxel-shape primitives shared by placers and features.

use crate::block::Block;
use crate::mathh::IVec3;
use crate::worldgen::feature::FeatureCtx;
use crate::worldgen::rng::FeatureRng;

/// Canonical fancy-oak leaf disc: a flat horizontal disc of leaves at world Y
/// `y`, radius `radius` (float). The `(|dx|+0.5)² + (|dz|+0.5)² <= radius²` test
/// rounds the corners; the loop bound `(radius + 0.618).floor()` matches the
/// reference disc's integer reach.
/// Over Air/Water only (== `set_leaf`).
pub fn leaf_disc(ctx: &mut FeatureCtx, center: IVec3, radius: f32, leaf: Block) {
    let ri = (radius + 0.618).floor() as i32;
    let r2 = radius * radius;
    for dx in -ri..=ri {
        for dz in -ri..=ri {
            let fx = dx.abs() as f32 + 0.5;
            let fz = dz.abs() as f32 + 0.5;
            if fx * fx + fz * fz <= r2 {
                ctx.set_leaf(IVec3::new(center.x + dx, center.y, center.z + dz), leaf);
            }
        }
    }
}

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

/// Like [`leaf_blob`] but rounds the box corners: at small radii `leaf_blob` is a
/// near-solid cube (r=2 fills the whole 3×3×3, since the 8 corners sit at
/// `d²=3 ≤ r²`), which reads as a literal block of leaves. Each corner-ish cell —
/// one on the outer shell with NO axis at zero, i.e. the bits that bulge the
/// sphere toward a box corner — is trimmed with probability `round`, so the mass
/// reads as a rounded clump. `round` near 1 gives an octahedral clump; 0 is the
/// plain cube. Over Air/Water only.
pub fn leaf_blob_rounded(
    ctx: &mut FeatureCtx,
    center: IVec3,
    radius: i32,
    leaf: Block,
    round: f32,
    rng: &mut FeatureRng,
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
                // Outer-shell corner (no axis on a face plane): the cube-y cells.
                // One draw per candidate keeps the stream deterministic.
                let corner = d2 >= r * r - 1 && lx != 0 && ly != 0 && lz != 0;
                if corner && rng.chance(round) {
                    continue;
                }
                ctx.set_leaf(
                    IVec3::new(center.x + lx, center.y + ly, center.z + lz),
                    leaf,
                );
            }
        }
    }
}
