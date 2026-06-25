//! Grass spread: dirt greens over into grass when grass grows nearby.

use crate::block::Block;
use crate::mathh::IVec3;
use crate::world::World;

use super::{grass, BlockBehavior};

/// How far, in blocks on every axis, a grass block may sit for it to spread onto
/// this dirt — a `(2·R+1)³` neighbourhood. Spread is pure proximity: whatever sits
/// *between* the two cells is irrelevant (unlike Minecraft, which also wants the
/// dirt uncovered and lit). One knob; the world reads it through the behaviour.
const SPREAD_RADIUS: i32 = 2;

/// Dirt. On a random tick it greens into [`Block::Grass`] when its top is open and
/// dry — neither smothered by a solid cover nor under water — and any grass block
/// lies within [`SPREAD_RADIUS`] blocks, so grass creeps outward over exposed dirt
/// across many ticks. That is the exact condition under which grass *survives* (see
/// [`grass::smothered`] / [`grass::submerged`]): dirt will not green a cell where the
/// grass would only die back on its next tick. Like grass, dirt tolerates a leaf
/// canopy and other `NoGrassDecay` cover but not a flood. The dirt is the active
/// party in the spread — it looks for grass and converts itself.
pub struct Dirt;

impl BlockBehavior for Dirt {
    fn has_random_tick(&self) -> bool {
        true
    }

    fn random_tick(&self, world: &mut World, pos: IVec3) {
        // Only green a cell where grass could actually live — an open, dry top
        // (not smothered, not flooded) — and only with grass within reach to spread.
        if !grass::smothered(world, pos)
            && !grass::submerged(world, pos)
            && grass_within(world, pos, SPREAD_RADIUS)
        {
            // Runs the usual block + light + mesh updates; the cell stays
            // random-tickable (grass ticks too), so the counter is unchanged.
            world.set_block_world(pos.x, pos.y, pos.z, Block::Grass);
        }
    }
}

/// The dirt singleton a row points at (`behavior: &behavior::DIRT`).
pub static DIRT: Dirt = Dirt;

/// Whether any [`Block::Grass`] sits within `radius` blocks of `center` on every
/// axis — a `(2·radius+1)³` box scan with the centre (the dirt itself) skipped.
/// A cell in an unloaded chunk simply reads as "not grass": missing information can
/// only delay a spread, never trigger one wrongly. (The opposite bias to leaf
/// decay, which *keeps* a leaf on an unknown neighbour — there the safe default is
/// "supported"; here it is "no grass yet".)
fn grass_within(world: &World, center: IVec3, radius: i32) -> bool {
    for dy in -radius..=radius {
        for dz in -radius..=radius {
            for dx in -radius..=radius {
                if dx == 0 && dy == 0 && dz == 0 {
                    continue;
                }
                let p = center + IVec3::new(dx, dy, dz);
                if world.block_if_loaded(p.x, p.y, p.z) == Some(Block::Grass) {
                    return true;
                }
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::{Chunk, ChunkPos};

    /// A world with one loaded chunk at (0,0). Coords kept a few blocks inside the
    /// 16-wide chunk so a `SPREAD_RADIUS` scan stays within the loaded cell.
    fn world_with_chunk() -> World {
        let mut w = World::new(1, 1);
        w.insert_chunk_for_test(ChunkPos::new(0, 0), Chunk::new(0, 0));
        w
    }

    #[test]
    fn grass_at_radius_is_found() {
        let mut w = world_with_chunk();
        let p = IVec3::new(8, 70, 8);
        w.set_block_world(p.x + SPREAD_RADIUS, p.y, p.z, Block::Grass);
        assert!(grass_within(&w, p, SPREAD_RADIUS));
    }

    #[test]
    fn grass_one_past_radius_is_not_found() {
        let mut w = world_with_chunk();
        let p = IVec3::new(8, 70, 8);
        w.set_block_world(p.x + SPREAD_RADIUS + 1, p.y, p.z, Block::Grass);
        assert!(!grass_within(&w, p, SPREAD_RADIUS));
    }

    #[test]
    fn diagonal_grass_within_radius_is_found() {
        // A corner of the box still counts: proximity is per-axis, not Euclidean.
        let mut w = world_with_chunk();
        let p = IVec3::new(8, 70, 8);
        let c = p + IVec3::new(SPREAD_RADIUS, SPREAD_RADIUS, SPREAD_RADIUS);
        w.set_block_world(c.x, c.y, c.z, Block::Grass);
        assert!(grass_within(&w, p, SPREAD_RADIUS));
    }

    #[test]
    fn dirt_with_grass_in_range_greens_over() {
        let mut w = world_with_chunk();
        let p = IVec3::new(8, 70, 8);
        w.set_block_world(p.x, p.y, p.z, Block::Dirt);
        w.set_block_world(p.x + 1, p.y, p.z, Block::Grass);
        DIRT.random_tick(&mut w, p);
        assert_eq!(w.block_if_loaded(p.x, p.y, p.z), Some(Block::Grass));
    }

    #[test]
    fn dirt_with_no_grass_in_range_stays_dirt() {
        let mut w = world_with_chunk();
        let p = IVec3::new(8, 70, 8);
        w.set_block_world(p.x, p.y, p.z, Block::Dirt);
        DIRT.random_tick(&mut w, p);
        assert_eq!(w.block_if_loaded(p.x, p.y, p.z), Some(Block::Dirt));
    }

    #[test]
    fn covered_dirt_does_not_green_even_with_grass_in_range() {
        // A solid block on top means grass could not survive here, so dirt must not
        // spread onto it — otherwise it would flip to grass and straight back.
        let mut w = world_with_chunk();
        let p = IVec3::new(8, 70, 8);
        w.set_block_world(p.x, p.y, p.z, Block::Dirt);
        w.set_block_world(p.x + 1, p.y, p.z, Block::Grass); // grass in range
        w.set_block_world(p.x, p.y + 1, p.z, Block::Stone); // but covered on top
        DIRT.random_tick(&mut w, p);
        assert_eq!(w.block_if_loaded(p.x, p.y, p.z), Some(Block::Dirt));
    }

    #[test]
    fn dirt_under_no_grass_decay_cover_greens() {
        // Grass spreads under a NoGrassDecay cover (leaves): the dirt greens even
        // though its top is solid, because that cover does not smother grass.
        let mut w = world_with_chunk();
        let p = IVec3::new(8, 70, 8);
        w.set_block_world(p.x, p.y, p.z, Block::Dirt);
        w.set_block_world(p.x + 1, p.y, p.z, Block::Grass); // grass in range
        w.set_block_world(p.x, p.y + 1, p.z, Block::OakLeaves); // leaf canopy on top
        DIRT.random_tick(&mut w, p);
        assert_eq!(w.block_if_loaded(p.x, p.y, p.z), Some(Block::Grass));
    }

    #[test]
    fn submerged_dirt_does_not_green_even_with_grass_in_range() {
        // Dirt under water must stay dirt even with grass alongside — otherwise the
        // spread would creep grass down a flooded slope (terrain under water is dirt).
        let mut w = world_with_chunk();
        let p = IVec3::new(8, 70, 8);
        w.set_block_world(p.x, p.y, p.z, Block::Dirt);
        w.set_block_world(p.x + 1, p.y, p.z, Block::Grass); // grass in range
        w.set_block_world(p.x, p.y + 1, p.z, Block::Water); // but flooded on top
        DIRT.random_tick(&mut w, p);
        assert_eq!(w.block_if_loaded(p.x, p.y, p.z), Some(Block::Dirt));
    }
}
