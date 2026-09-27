use crate::feature::FeatureCtx;
use crate::rng::FeatureRng;
use petramond_world::block::Block;
use petramond_world::mathh::IVec3;

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

/// Rounded leaf blob. Plain radius test at small r is basically a solid cube (r=2 fills the whole
/// 3x3x3, corners are at d2=3<=r2), looks like a block, not leaves.
/// So we trim corner cells (outer shell, no axis zero, the bits sticking toward a box corner) with
/// probability `round`, so it reads as a rounded clump.
/// `round` near 1 gives an octahedral clump, 0 gives the cube. Only writes over Air/Water.
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

pub(crate) fn connected_line(a: IVec3, b: IVec3, mut emit: impl FnMut(IVec3)) {
    let delta = IVec3::new(b.x - a.x, b.y - a.y, b.z - a.z);
    let steps = delta.x.abs().max(delta.y.abs()).max(delta.z.abs()).max(1);
    let mut previous = a;
    emit(a);
    for i in 1..=steps {
        let t = i as f32 / steps as f32;
        let next = IVec3::new(
            a.x + (delta.x as f32 * t).round() as i32,
            a.y + (delta.y as f32 * t).round() as i32,
            a.z + (delta.z as f32 * t).round() as i32,
        );
        for axis in 0..3 {
            let target = match axis {
                0 => next.x,
                1 => next.z,
                _ => next.y,
            };
            let coordinate = match axis {
                0 => &mut previous.x,
                1 => &mut previous.z,
                _ => &mut previous.y,
            };
            *coordinate = target;
            emit(previous);
        }
    }
}
