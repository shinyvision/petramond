pub mod rule;

use petramond_world::block::Block;
use petramond_world::chunk::SEA_LEVEL;
use rule::{SurfaceCtx, SurfaceRule};

pub(crate) const MAX_SKIN_BAND_DEPTH: i32 = 8;

#[derive(Copy, Clone, Debug, Default)]
pub struct SurfaceSystem;

impl SurfaceSystem {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::biome::spec;
    use petramond_world::biome::Biome;

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

        let plains = ctx(SEA_LEVEL - 1, 0, Biome::PLAINS);
        assert_eq!(
            surface.skin_block(&plains, spec(Biome::PLAINS).surface),
            Block::Dirt
        );

        let snowy_top = ctx(SEA_LEVEL - 1, 0, Biome::SNOWY_TUNDRA);
        assert_eq!(
            surface.skin_block(&snowy_top, spec(Biome::SNOWY_TUNDRA).surface),
            Block::Dirt
        );

        let snowy_subsurface = ctx(SEA_LEVEL - 2, 1, Biome::SNOWY_TUNDRA);
        assert_eq!(
            surface.skin_block(&snowy_subsurface, spec(Biome::SNOWY_TUNDRA).surface),
            Block::Dirt
        );
    }

    #[test]
    fn above_sea_grass_caps_are_unchanged() {
        let surface = SurfaceSystem;

        let plains = ctx(SEA_LEVEL + 1, 0, Biome::PLAINS);
        assert_eq!(
            surface.skin_block(&plains, spec(Biome::PLAINS).surface),
            Block::Grass
        );

        let snowy = ctx(SEA_LEVEL + 1, 0, Biome::SNOWY_TUNDRA);
        assert_eq!(
            surface.skin_block(&snowy, spec(Biome::SNOWY_TUNDRA).surface),
            Block::Grass
        );
    }
}
