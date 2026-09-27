use super::BehaviorWorld;
use crate::block::Block;
use crate::mathh::IVec3;
use crate::world::data::WorldData;

use super::{grass, BlockBehavior};

pub const SPREAD_RADIUS: i32 = 2;

pub struct Dirt;

impl BlockBehavior for Dirt {
    fn key(&self) -> &'static str {
        "dirt"
    }

    fn has_random_tick(&self) -> bool {
        true
    }

    fn random_tick(&self, world: &mut dyn BehaviorWorld, pos: IVec3) {
        if !grass::smothered(world.data(), pos)
            && !grass::submerged(world.data(), pos)
            && grass_within(world.data(), pos, SPREAD_RADIUS)
        {
            world.set_block_world(pos.x, pos.y, pos.z, Block::Grass);
        }
    }
}

pub static DIRT: Dirt = Dirt;

pub fn grass_within(world: &WorldData, center: IVec3, radius: i32) -> bool {
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
mod tests;
