//! `SurfaceSystem` — composes a column's surface material per voxel.
//!
//! The driver's skin pass walks each contiguous solid run top-down and calls
//! `skin_block` with the voxel's `depth_from_top`. A cross-cutting river-bed
//! pre-pass (applies in every biome, only at exposed river banks/beds) wraps the
//! biome's layered `SurfaceRule` stack, which resolves the grass/dirt/stone/sand/
//! snow bands by depth and altitude.

pub mod rule;

use crate::block::Block;
use crate::chunk::SEA_LEVEL;
use rule::{SurfaceCtx, SurfaceRule};

const RIVER_WATERLINE_INFLUENCE: f32 = 0.05;
// River influence now plateaus near 1 across the channel + floodplain (the valley
// carve), so the inner-bank knee is pushed out to keep banks from over-sanding.
const RIVER_INNER_BANK_INFLUENCE: f32 = 0.55;
const RIVER_WATERLINE_MARGIN: i32 = 1;
const RIVER_INNER_BANK_MARGIN: i32 = 3;

pub struct SurfaceSystem;

impl SurfaceSystem {
    /// Material for one solid voxel given its surface context and the column's
    /// (already looked-up) biome surface rule. The river-bed pre-pass runs only
    /// at the exposed top near the waterline; otherwise the layered rule resolves
    /// the band by depth/altitude. The rule is passed in so the caller looks the
    /// biome up once per column, not once per voxel.
    #[inline]
    pub fn skin_block(&self, c: &SurfaceCtx, rule: &SurfaceRule) -> Block {
        // Submerged river bed uses the river's bedding material. Exposed banks
        // keep the biome surface unless a separate bank deposit was selected.
        let low_waterline =
            c.river > RIVER_WATERLINE_INFLUENCE && c.y <= c.water_y + RIVER_WATERLINE_MARGIN;
        let inner_bank =
            c.river > RIVER_INNER_BANK_INFLUENCE && c.y <= c.water_y + RIVER_INNER_BANK_MARGIN;
        if c.depth_from_top == 0 && c.surf_y < c.water_y && low_waterline && !c.preserve_river_bed {
            return c.river_bed;
        }
        if c.depth_from_top == 0 && c.surf_y >= c.water_y && (low_waterline || inner_bank) {
            if let Some(bank) = c.river_bank {
                return bank;
            }
        }
        let block = rule.resolve(c).unwrap_or(Block::Stone);
        if c.y < SEA_LEVEL && matches!(block, Block::Grass | Block::Snow) {
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
    use crate::worldgen::data::biomes::def;

    fn ctx(y: i32, depth_from_top: u32, _biome: Biome) -> SurfaceCtx {
        SurfaceCtx {
            y,
            surf_y: y,
            depth_from_top,
            river: 0.0,
            water_y: SEA_LEVEL,
            river_bed: Block::Dirt,
            river_bank: None,
            preserve_river_bed: false,
        }
    }

    fn river_ctx(
        y: i32,
        influence: f32,
        bed: Block,
        bank: Option<Block>,
        preserve_bed: bool,
    ) -> SurfaceCtx {
        SurfaceCtx {
            y,
            surf_y: y,
            depth_from_top: 0,
            river: influence,
            water_y: SEA_LEVEL,
            river_bed: bed,
            river_bank: bank,
            preserve_river_bed: preserve_bed,
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

    #[test]
    fn river_bed_material_applies_only_to_submerged_bed() {
        let surface = SurfaceSystem;
        let plains = def(Biome::Plains).surface;

        let underwater_bed = river_ctx(SEA_LEVEL - 1, 0.72, Block::Sand, None, false);
        assert_eq!(surface.skin_block(&underwater_bed, plains), Block::Sand);

        let grass_shelf = river_ctx(SEA_LEVEL, 0.72, Block::Dirt, None, false);
        assert_eq!(surface.skin_block(&grass_shelf, plains), Block::Grass);

        let outer_slope = river_ctx(SEA_LEVEL + 3, 0.18, Block::Sand, None, false);
        assert_eq!(surface.skin_block(&outer_slope, plains), Block::Grass);
    }

    #[test]
    fn optional_bank_deposit_can_override_exposed_bank() {
        let surface = SurfaceSystem;
        let plains = def(Biome::Plains).surface;

        let waterline = river_ctx(SEA_LEVEL + 1, 0.12, Block::Dirt, Some(Block::Sand), false);
        assert_eq!(surface.skin_block(&waterline, plains), Block::Sand);

        let inner_bank = river_ctx(SEA_LEVEL + 3, 0.72, Block::Dirt, Some(Block::Gravel), false);
        assert_eq!(surface.skin_block(&inner_bank, plains), Block::Gravel);
    }

    #[test]
    fn existing_water_body_floors_preserve_biome_surface() {
        let surface = SurfaceSystem;

        let lake_floor = river_ctx(SEA_LEVEL - 1, 0.9, Block::Sand, Some(Block::Gravel), true);
        assert_eq!(
            surface.skin_block(&lake_floor, def(Biome::Plains).surface),
            Block::Dirt
        );
    }
}
