//! Declarative surface rules — `condition -> block`, replacing the hardcoded
//! `surface_block`/`subsurface_block` match arms.
//!
//! A rule resolves top-down; the first branch that yields `Some` wins. The live
//! biome surface stacks compose `Block` + `Sequence` + `Condition` over the
//! surface / depth / Y conditions below (e.g. the mountain colour bands key
//! off `SurfaceAboveY`).

use crate::rng::patch_field;
use petramond_world::block::Block;
use petramond_world::chunk::SEA_LEVEL;

pub enum SurfaceRule {
    /// Unconditionally place this block.
    Block(Block),
    /// First child that resolves to `Some` wins.
    Sequence(&'static [SurfaceRule]),
    /// Evaluate `then` only when `when` holds; otherwise yield `None`.
    Condition {
        when: SurfaceCond,
        then: &'static SurfaceRule,
    },
}

pub enum SurfaceCond {
    /// The COLUMN's heightfield surface is strictly above this world Y. Use this
    /// for altitude bands (snow caps / bare rock) so the whole column is treated
    /// uniformly by its height — not per-voxel, which would paint overhang
    /// undersides by absolute Y.
    SurfaceAboveY(i32),
    /// y is within N blocks below the column's surface top (depth <= N).
    DepthFromTop(u32),
    /// The column's surface is at or below sea level. This is a COLUMN predicate,
    /// true for every voxel down to bedrock — pair the branch with a `DepthFromTop`
    /// gate, or it skins the whole column and cave carving exposes it.
    Underwater,
    /// A SMOOTH low-frequency value-noise draw in `[0,1)` is below the threshold.
    /// Because nearby columns share corner samples, the result is contiguous
    /// CLUSTERS (`period`-sized patches) rather than per-column speckle — e.g.
    /// occasional grass clumps on a podzol floor. `period` is the patch wavelength
    /// in blocks.
    ClusterNoiseBelow {
        salt: u64,
        threshold: f32,
        period: f32,
    },
}

pub struct SurfaceCtx {
    pub seed: u32,
    pub wx: i32,
    pub wz: i32,
    pub y: i32,
    pub surf_y: i32,
    pub depth_from_top: u32,
}

impl SurfaceCond {
    #[inline]
    fn test(&self, c: &SurfaceCtx) -> bool {
        match self {
            SurfaceCond::SurfaceAboveY(n) => c.surf_y > *n,
            SurfaceCond::DepthFromTop(n) => c.depth_from_top <= *n,
            SurfaceCond::Underwater => c.surf_y < SEA_LEVEL,
            SurfaceCond::ClusterNoiseBelow {
                salt,
                threshold,
                period,
            } => patch_field(c.seed, *salt, c.wx, c.wz, *period) < *threshold,
        }
    }
}

impl SurfaceRule {
    /// The deepest `DepthFromTop` band anywhere in this rule, or `None` when
    /// no branch depends on depth.
    pub fn deepest_band(&self) -> Option<u32> {
        match self {
            SurfaceRule::Block(_) => None,
            SurfaceRule::Sequence(rules) => {
                rules.iter().filter_map(SurfaceRule::deepest_band).max()
            }
            SurfaceRule::Condition { when, then } => {
                let own = match when {
                    SurfaceCond::DepthFromTop(n) => Some(*n),
                    _ => None,
                };
                own.max(then.deepest_band())
            }
        }
    }

    /// Resolve to a block for this context, or `None` if no branch matches.
    pub fn resolve(&self, c: &SurfaceCtx) -> Option<Block> {
        match self {
            SurfaceRule::Block(b) => Some(*b),
            SurfaceRule::Sequence(rules) => rules.iter().find_map(|r| r.resolve(c)),
            SurfaceRule::Condition { when, then } => {
                if when.test(c) {
                    then.resolve(c)
                } else {
                    None
                }
            }
        }
    }
}
