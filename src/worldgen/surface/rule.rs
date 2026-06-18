//! Declarative surface rules — `condition -> block`, replacing the hardcoded
//! `surface_block`/`subsurface_block` match arms.
//!
//! A rule resolves top-down; the first branch that yields `Some` wins. Strata P2
//! uses only `AboveY` + `Block` + `Sequence` to mirror the original surface
//! material exactly (the mountain `>95/>78` colour bands). The remaining
//! conditions exist for the richer layered stacks introduced in P4.

use crate::biome::Biome;
use crate::block::Block;
use crate::chunk::SEA_LEVEL;

pub enum SurfaceRule {
    /// Unconditionally place this block.
    Block(Block),
    /// First child that resolves to `Some` wins.
    Sequence(&'static [SurfaceRule]),
    /// Evaluate `then` only when `when` holds; otherwise yield `None`.
    Condition { when: SurfaceCond, then: &'static SurfaceRule },
}

pub enum SurfaceCond {
    /// Column surface (or evaluated y) is strictly above this world Y.
    AboveY(i32),
    /// Strictly below this world Y.
    BelowY(i32),
    /// y is within N blocks below the column's surface top (depth <= N).
    DepthFromTop(u32),
    /// The column's surface is at or below sea level.
    Underwater,
}

pub struct SurfaceCtx {
    pub y: i32,
    pub surf_y: i32,
    pub depth_from_top: u32,
    pub biome: Biome,
    pub river: f32,
}

impl SurfaceCond {
    #[inline]
    fn test(&self, c: &SurfaceCtx) -> bool {
        match self {
            SurfaceCond::AboveY(n) => c.y > *n,
            SurfaceCond::BelowY(n) => c.y < *n,
            SurfaceCond::DepthFromTop(n) => c.depth_from_top <= *n,
            SurfaceCond::Underwater => c.surf_y <= SEA_LEVEL,
        }
    }
}

impl SurfaceRule {
    /// Resolve to a block for this context, or `None` if no branch matches.
    pub fn resolve(&self, c: &SurfaceCtx) -> Option<Block> {
        match self {
            SurfaceRule::Block(b) => Some(*b),
            SurfaceRule::Sequence(rules) => rules.iter().find_map(|r| r.resolve(c)),
            SurfaceRule::Condition { when, then } => {
                if when.test(c) { then.resolve(c) } else { None }
            }
        }
    }
}
