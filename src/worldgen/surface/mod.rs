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
        let block = rule.resolve(c).unwrap_or(Block::Stone);
        if c.y < SEA_LEVEL && matches!(block, Block::Grass | Block::Snow) {
            Block::Dirt
        } else {
            block
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::biome::Biome;
    use crate::worldgen::data::biomes::def;

    fn ctx(y: i32, depth_from_top: u32, biome: Biome) -> SurfaceCtx {
        SurfaceCtx {
            y,
            surf_y: y,
            depth_from_top,
            biome,
            river: 0.0,
        }
    }

    #[test]
    fn below_sea_grass_caps_resolve_to_dirt() {
        let surface = SurfaceSystem;

        let plains = ctx(SEA_LEVEL - 1, 0, Biome::Plains);
        assert_eq!(
            surface.skin_block(&plains, def(Biome::Plains).surface),
            Block::Dirt
        );

        let snowy_top = ctx(SEA_LEVEL - 1, 0, Biome::SnowyTundra);
        assert_eq!(
            surface.skin_block(&snowy_top, def(Biome::SnowyTundra).surface),
            Block::Dirt
        );

        let snowy_subsurface = ctx(SEA_LEVEL - 2, 1, Biome::SnowyTundra);
        assert_eq!(
            surface.skin_block(&snowy_subsurface, def(Biome::SnowyTundra).surface),
            Block::Dirt
        );
    }

    #[test]
    fn above_sea_grass_caps_are_unchanged() {
        let surface = SurfaceSystem;

        let plains = ctx(SEA_LEVEL + 1, 0, Biome::Plains);
        assert_eq!(
            surface.skin_block(&plains, def(Biome::Plains).surface),
            Block::Grass
        );

        let snowy = ctx(SEA_LEVEL + 1, 0, Biome::SnowyTundra);
        assert_eq!(
            surface.skin_block(&snowy, def(Biome::SnowyTundra).surface),
            Block::Snow
        );
    }
}
