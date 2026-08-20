//! Trunk placers — build a tree's trunk and return the foliage attach point(s)
//! plus every log cell written (the support set the canopy's connectivity
//! commit floods from — see `foliage::Canopy`).
//!
//! Draws happen here in the god file's order. `sample_height` consumes exactly
//! one `next_i32` iff the height range is non-degenerate (matching e.g.
//! `4 + next_i32(0,1)`), and consumes NOTHING for a fixed height (matching the
//! literal-height oaks) — this no-extra-draw rule is load-bearing for parity.

use petramond_world::block::Block;
use petramond_world::mathh::IVec3;
use crate::feature::FeatureCtx;
use crate::rng::FeatureRng;

/// A placed trunk: where the foliage attaches, and every log cell written —
/// the wood the canopy commit treats as leaf support.
pub struct TrunkPlan {
    pub attach: Vec<IVec3>,
    pub logs: Vec<IVec3>,
}

pub trait TrunkPlacer: Send + Sync {
    /// Place the trunk; return its plan. `height` is {min, max}.
    fn place(
        &self,
        ctx: &mut FeatureCtx,
        origin: IVec3,
        height: (i32, i32),
        log: Block,
        rng: &mut FeatureRng,
    ) -> TrunkPlan;

    /// Maximum horizontal wander of any log (and so of the attach column) from
    /// the origin column. Part of the canopy reach fence `data::features`
    /// validates at load, so foliage never reads outside the candidate window.
    fn max_lean(&self) -> i32 {
        0
    }
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
    ) -> TrunkPlan {
        let h = sample_height(height, rng);
        let mut logs = Vec::with_capacity(h as usize);
        for i in 0..h {
            let p = IVec3::new(origin.x, origin.y + i, origin.z);
            ctx.set_log(p, log);
            logs.push(p);
        }
        TrunkPlan {
            attach: vec![IVec3::new(origin.x, origin.y + h - 1, origin.z)],
            logs,
        }
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
    ) -> TrunkPlan {
        let h = sample_height(height, rng);
        let dx = rng.next_i32(-1, 1);
        let dz = rng.next_i32(-1, 1);
        let (mut cx, mut cz) = (origin.x, origin.z);
        let mut logs = Vec::with_capacity(h as usize);
        for i in 0..h {
            let p = IVec3::new(cx, origin.y + i, cz);
            ctx.set_log(p, log);
            logs.push(p);
            if i == h / 2 {
                cx += dx;
                cz += dz;
            }
        }
        TrunkPlan {
            attach: vec![IVec3::new(cx, origin.y + h - 1, cz)],
            logs,
        }
    }

    fn max_lean(&self) -> i32 {
        1
    }
}
