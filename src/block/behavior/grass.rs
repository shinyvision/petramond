//! Grass cover: a grass block dies back to dirt when a solid block smothers it from
//! above or water floods it.

use crate::block::{Block, BlockTag};
use crate::mathh::IVec3;
use crate::world::World;

use super::BlockBehavior;

/// Grass. On a random tick it dies back to [`Block::Dirt`] when the cell directly
/// above [`smothered`]s it OR [`submerged`]s it — grass survives under neither a
/// placed solid block (though it tolerates leaves and other
/// [`BlockTag::NoGrassDecay`] cover) nor water. The exact inverse of
/// [`Dirt`](super::dirt::Dirt)'s spread (which refuses to green such a cell), so a
/// surface settles into a stable state: grass only where its top is open or
/// canopied and dry, dirt under solid cover or water.
pub struct Grass;

impl BlockBehavior for Grass {
    fn has_random_tick(&self) -> bool {
        true
    }

    fn random_tick(&self, world: &mut World, pos: IVec3) {
        // Grass dies back to dirt when a solid cover smothers it or water drowns it —
        // neither leaves it a top it can live under.
        if smothered(world, pos) || submerged(world, pos) {
            // Runs the usual block + light + mesh updates; the cell stays
            // random-tickable (dirt ticks too), so the counter is unchanged.
            world.set_block_world(pos.x, pos.y, pos.z, Block::Dirt);
        }
    }
}

/// The grass singleton a row points at (`behavior: &behavior::GRASS`).
pub static GRASS: Grass = Grass;

/// Whether the cell directly above `pos` smothers grass: a solid cover that does
/// NOT carry [`BlockTag::NoGrassDecay`]. This is the condition that kills grass, and
/// (read the other way) the cover that stops dirt greening over — both behaviours
/// share it so the spread and the death agree on one rule. Leaves and other
/// `NoGrassDecay` blocks are solid yet let grass live, so they never smother. An
/// unloaded or out-of-column cell above counts as not smothering: open sky over a
/// top-of-world block, and never a state change on missing information.
pub(super) fn smothered(world: &World, pos: IVec3) -> bool {
    world
        .block_if_loaded(pos.x, pos.y + 1, pos.z)
        .is_some_and(|b| b.is_solid() && !b.has_tag(BlockTag::NoGrassDecay))
}

/// Whether the cell directly above `pos` is water — grass drowns and dies back to
/// dirt when flooded, and (read the other way) dirt will not green under water.
/// Shared with [`Dirt`](super::dirt::Dirt) so a submerged column never re-greens:
/// worldgen already lays dirt below the waterline, and this keeps the spread from
/// creeping grass back down a flooded slope. An unloaded or out-of-column cell above
/// counts as not water — open sky, never a state change on missing information.
pub(super) fn submerged(world: &World, pos: IVec3) -> bool {
    world.block_if_loaded(pos.x, pos.y + 1, pos.z) == Some(Block::Water)
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

    #[test]
    fn grass_under_no_grass_decay_cover_survives() {
        // A solid cover tagged NoGrassDecay (leaves being the canonical carrier) does
        // not smother the grass below: it stays grass instead of dying back to dirt.
        let mut w = world_with_chunk();
        let p = IVec3::new(8, 70, 8);
        w.set_block_world(p.x, p.y, p.z, Block::Grass);
        w.set_block_world(p.x, p.y + 1, p.z, Block::OakLeaves);
        GRASS.random_tick(&mut w, p);
        assert_eq!(w.block_if_loaded(p.x, p.y, p.z), Some(Block::Grass));
    }

    #[test]
    fn flooded_grass_dies_to_dirt() {
        // Water directly overhead drowns grass — it reverts to dirt, so the spread
        // can never leave grass sitting under water.
        let mut w = world_with_chunk();
        let p = IVec3::new(8, 70, 8);
        w.set_block_world(p.x, p.y, p.z, Block::Grass);
        w.set_block_world(p.x, p.y + 1, p.z, Block::Water);
        GRASS.random_tick(&mut w, p);
        assert_eq!(w.block_if_loaded(p.x, p.y, p.z), Some(Block::Dirt));
    }
}
