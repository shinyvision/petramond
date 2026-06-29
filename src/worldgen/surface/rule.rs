//! Declarative surface rules — `condition -> block`, replacing the hardcoded
//! `surface_block`/`subsurface_block` match arms.
//!
//! A rule resolves top-down; the first branch that yields `Some` wins. The live
//! biome surface stacks compose `Block` + `Sequence` + `Condition` over the
//! surface / depth / Y conditions below (e.g. the mountain colour bands key
//! off `SurfaceAboveY`).

use crate::block::Block;
use crate::chunk::SEA_LEVEL;
use crate::mathh::smoothstep;
use crate::worldgen::rng::FeatureRng;

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
    /// The column's surface is at or below sea level.
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
            } => cluster_field(c.seed, *salt, c.wx, c.wz, *period) < *threshold,
        }
    }
}

/// Smooth low-frequency value field in `[0,1)` at world `(wx,wz)`: hashed lattice
/// corners with a smoothstep bilinear blend, so a threshold cuts out organic
/// blobs rather than a hard grid. Pure function of `(seed, salt, wx, wz)`, so it
/// is seamless across chunk borders. (Mirrors the patch field the vegetation pass
/// uses for flower/fern clusters; kept here so surface rules don't depend on the
/// feature layer.)
fn cluster_field(seed: u32, salt: u64, wx: i32, wz: i32, period: f32) -> f32 {
    let fx = wx as f32 / period;
    let fz = wz as f32 / period;
    let x0 = fx.floor() as i32;
    let z0 = fz.floor() as i32;
    let tx = smoothstep(0.0, 1.0, fx - x0 as f32);
    let tz = smoothstep(0.0, 1.0, fz - z0 as f32);
    let corner = |ix: i32, iz: i32| FeatureRng::positional(seed, salt, ix, 0, iz).next_f32();
    let a = corner(x0, z0) + (corner(x0 + 1, z0) - corner(x0, z0)) * tx;
    let b = corner(x0, z0 + 1) + (corner(x0 + 1, z0 + 1) - corner(x0, z0 + 1)) * tx;
    a + (b - a) * tz
}

impl SurfaceRule {
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
