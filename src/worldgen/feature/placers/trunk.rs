//! Trunk placers — build a tree's trunk and return foliage attach point(s).
//!
//! Draws happen here in the god file's order. `sample_height` consumes exactly
//! one `next_i32` iff the height range is non-degenerate (matching e.g.
//! `4 + next_i32(0,1)`), and consumes NOTHING for a fixed height (matching the
//! literal-height oaks) — this no-extra-draw rule is load-bearing for parity.

use crate::block::Block;
use crate::mathh::IVec3;
use crate::worldgen::feature::FeatureCtx;
use crate::worldgen::rng::FeatureRng;

pub trait TrunkPlacer: Send + Sync {
    /// Place the trunk; return foliage attach points. `height` is {min, max}.
    fn place(
        &self,
        ctx: &mut FeatureCtx,
        origin: IVec3,
        height: (i32, i32),
        log: Block,
        rng: &mut FeatureRng,
    ) -> Vec<IVec3>;
}

/// Draw a height: one `next_i32(min,max)` iff `min < max`, else `min` (no draw).
#[inline]
pub fn sample_height(height: (i32, i32), rng: &mut FeatureRng) -> i32 {
    if height.0 < height.1 {
        rng.next_i32(height.0, height.1)
    } else {
        height.0
    }
}

/// Straight vertical trunk (== `oak_simple` with dx = dz = 0).
pub struct StraightTrunk;

impl TrunkPlacer for StraightTrunk {
    fn place(
        &self,
        ctx: &mut FeatureCtx,
        origin: IVec3,
        height: (i32, i32),
        log: Block,
        rng: &mut FeatureRng,
    ) -> Vec<IVec3> {
        let h = sample_height(height, rng);
        for i in 0..h {
            ctx.set_log(IVec3::new(origin.x, origin.y + i, origin.z), log);
        }
        vec![IVec3::new(origin.x, origin.y + h - 1, origin.z)]
    }
}

/// Trunk with a single mid-height lean (== `oak_simple` Oak2 path).
/// Draws height, then dx, then dz — matching the god file's argument order.
pub struct LeaningTrunk;

impl TrunkPlacer for LeaningTrunk {
    fn place(
        &self,
        ctx: &mut FeatureCtx,
        origin: IVec3,
        height: (i32, i32),
        log: Block,
        rng: &mut FeatureRng,
    ) -> Vec<IVec3> {
        let h = sample_height(height, rng);
        let dx = rng.next_i32(-1, 1);
        let dz = rng.next_i32(-1, 1);
        let (mut cx, mut cz) = (origin.x, origin.z);
        for i in 0..h {
            ctx.set_log(IVec3::new(cx, origin.y + i, cz), log);
            if i == h / 2 {
                cx += dx;
                cz += dz;
            }
        }
        vec![IVec3::new(cx, origin.y + h - 1, cz)]
    }
}
