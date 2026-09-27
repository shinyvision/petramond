use super::BehaviorWorld;
use crate::block::{Block, BlockTag};
use crate::mathh::IVec3;
use crate::world::data::WorldData;

use super::BlockBehavior;

pub struct Grass;

impl BlockBehavior for Grass {
    fn key(&self) -> &'static str {
        "grass"
    }

    fn has_random_tick(&self) -> bool {
        true
    }

    fn random_tick(&self, world: &mut dyn BehaviorWorld, pos: IVec3) {
        if smothered(world.data(), pos) || submerged(world.data(), pos) {
            world.set_block_world(pos.x, pos.y, pos.z, Block::Dirt);
        }
    }
}

pub static GRASS: Grass = Grass;

pub(super) fn smothered(world: &WorldData, pos: IVec3) -> bool {
    world
        .block_if_loaded(pos.x, pos.y + 1, pos.z)
        .is_some_and(|b| b.is_solid() && !b.has_tag(BlockTag::NO_GRASS_DECAY))
}

pub(super) fn submerged(world: &WorldData, pos: IVec3) -> bool {
    world
        .block_if_loaded(pos.x, pos.y + 1, pos.z)
        .is_some_and(|b| b.fluid().is_some())
}

#[cfg(test)]
mod tests;
