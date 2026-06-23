//! Grass cover: a grass block smothered by a solid block above dies back to dirt.

use crate::block::Block;
use crate::mathh::IVec3;
use crate::world::World;

use super::BlockBehavior;

/// Grass. On a random tick it dies back to [`Block::Dirt`] when the cell directly
/// above is solid — grass smothered under a placed block (or any solid cover)
/// cannot survive. The exact inverse of [`Dirt`](super::dirt::Dirt)'s spread
/// (which refuses to green a covered cell for the same reason), so a surface
/// settles into a stable state: grass only where its top is open, dirt under cover.
pub struct Grass;

impl BlockBehavior for Grass {
    fn has_random_tick(&self) -> bool {
        true
    }

    fn random_tick(&self, world: &mut World, pos: IVec3) {
        if covered_by_solid(world, pos) {
            // Runs the usual block + light + mesh updates; the cell stays
            // random-tickable (dirt ticks too), so the counter is unchanged.
            world.set_block_world(pos.x, pos.y, pos.z, Block::Dirt);
        }
    }
}

/// The grass singleton a row points at (`behavior: &behavior::GRASS`).
pub static GRASS: Grass = Grass;

/// Whether the cell directly above `pos` is solid — the "smothered" condition that
/// kills grass, and (read the other way) the cover that stops dirt greening over.
/// Shared by both behaviours so the spread and the death agree on one rule. An
/// unloaded or out-of-column cell above counts as not solid: open sky over a
/// top-of-world block, and never a state change on missing information.
pub(super) fn covered_by_solid(world: &World, pos: IVec3) -> bool {
    world
        .block_if_loaded(pos.x, pos.y + 1, pos.z)
        .is_some_and(|b| b.is_solid())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::{Chunk, ChunkPos};

    fn world_with_chunk() -> World {
        let mut w = World::new(1, 1);
        w.insert_chunk_for_test(ChunkPos::new(0, 0), Chunk::new(0, 0));
        w
    }

    #[test]
    fn grass_under_solid_dies_to_dirt() {
        let mut w = world_with_chunk();
        let p = IVec3::new(8, 70, 8);
        w.set_block_world(p.x, p.y, p.z, Block::Grass);
        w.set_block_world(p.x, p.y + 1, p.z, Block::Stone);
        GRASS.random_tick(&mut w, p);
        assert_eq!(w.block_if_loaded(p.x, p.y, p.z), Some(Block::Dirt));
    }

    #[test]
    fn uncovered_grass_survives() {
        let mut w = world_with_chunk();
        let p = IVec3::new(8, 70, 8);
        w.set_block_world(p.x, p.y, p.z, Block::Grass);
        GRASS.random_tick(&mut w, p);
        assert_eq!(w.block_if_loaded(p.x, p.y, p.z), Some(Block::Grass));
    }
}
