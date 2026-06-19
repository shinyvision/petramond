//! `SurfaceSystem` — composes a column's surface material per voxel.
//!
//! The driver's skin pass walks each contiguous solid run top-down and calls
//! `skin_block` with the voxel's `depth_from_top`. A cross-cutting river/beach
//! sand pre-pass (applies in every biome, only at the exposed top) wraps the
//! biome's layered `SurfaceRule` stack, which resolves the grass/dirt/stone/sand/
//! snow bands by depth and altitude.

pub mod rule;

use crate::block::Block;
use crate::chunk::SEA_LEVEL;
use rule::{SurfaceCtx, SurfaceRule};

pub struct SurfaceSystem;

impl SurfaceSystem {
    /// Material for one solid voxel given its surface context and the column's
    /// (already looked-up) biome surface rule. The river/beach sand pre-pass runs
    /// only at the exposed top (depth 0) near sea level; otherwise the layered rule
    /// resolves the band by depth/altitude. The rule is passed in so the caller
    /// looks the biome up once per column, not once per voxel.
    #[inline]
    pub fn skin_block(&self, c: &SurfaceCtx, rule: &SurfaceRule) -> Block {
        // River bed + waterline banks: sand a couple of blocks up from the water,
        // so river edges read as sandy point-bars rather than grass to the water.
        if c.depth_from_top == 0 && c.river > 0.05 && c.y <= SEA_LEVEL + 2 {
            return Block::Sand;
        }
        rule.resolve(c).unwrap_or(Block::Stone)
    }
}
