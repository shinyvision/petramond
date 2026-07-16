//! `SurfaceSystem` — composes a column's surface material per voxel.
//!
//! The driver's skin pass walks each contiguous solid run top-down and calls
//! `skin_block` with the voxel's `depth_from_top`. The biome's layered
//! `SurfaceRule` stack resolves the grass/dirt/stone/sand bands by depth and
//! altitude.

pub mod rule;

use crate::block::Block;
use crate::chunk::SEA_LEVEL;
use rule::{SurfaceCtx, SurfaceRule};

#[derive(Copy, Clone, Debug, Default)]
pub struct SurfaceSystem;

impl SurfaceSystem {
    /// Material for one solid voxel given its surface context and the column's
    /// (already looked-up) biome surface rule. The rule is passed in so the caller
    /// looks the biome up once per column, not once per voxel.
    #[inline]
    pub fn skin_block(&self, c: &SurfaceCtx, rule: &SurfaceRule) -> Block {
        let block = rule.resolve(c).unwrap_or(Block::Stone);
        if c.y < SEA_LEVEL && block == Block::Grass {
            Block::Dirt
        } else {
            block
        }
    }
}

#[cfg(all(test, feature = "worldgen-tests"))]
mod tests {
    use super::*;
    use crate::biome::Biome;
    use crate::worldgen::biome::spec;

    fn ctx(y: i32, depth_from_top: u32, _biome: Biome) -> SurfaceCtx {
        SurfaceCtx {
            seed: 0,
            wx: 0,
            wz: 0,
            y,
            surf_y: y,
            depth_from_top,
        }
    }

    #[test]
    fn below_sea_grass_caps_resolve_to_dirt() {
        let surface = SurfaceSystem;

        let plains = ctx(SEA_LEVEL - 1, 0, Biome::Plains);
        assert_eq!(
            surface.skin_block(&plains, spec(Biome::Plains).surface),
            Block::Dirt
        );

        let snowy_top = ctx(SEA_LEVEL - 1, 0, Biome::SnowyTundra);
        assert_eq!(
            surface.skin_block(&snowy_top, spec(Biome::SnowyTundra).surface),
            Block::Dirt
        );

        let snowy_subsurface = ctx(SEA_LEVEL - 2, 1, Biome::SnowyTundra);
        assert_eq!(
            surface.skin_block(&snowy_subsurface, spec(Biome::SnowyTundra).surface),
            Block::Dirt
        );
    }

    #[test]
    fn above_sea_grass_caps_are_unchanged() {
        let surface = SurfaceSystem;

        let plains = ctx(SEA_LEVEL + 1, 0, Biome::Plains);
        assert_eq!(
            surface.skin_block(&plains, spec(Biome::Plains).surface),
            Block::Grass
        );

        // Snowy biomes cap with ordinary grass — the snow layer above it (and
        // the snowy side rendering) comes from the vegetation pass, not the
        // skin.
        let snowy = ctx(SEA_LEVEL + 1, 0, Biome::SnowyTundra);
        assert_eq!(
            surface.skin_block(&snowy, spec(Biome::SnowyTundra).surface),
            Block::Grass
        );
    }
}
