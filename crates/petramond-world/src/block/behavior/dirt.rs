//! Grass spread: dirt greens over into grass when grass grows nearby.

use crate::block::Block;
use crate::mathh::IVec3;
use super::BehaviorWorld;
use crate::world::data::WorldData;

use super::{grass, BlockBehavior};

/// How far, in blocks on every axis, a grass block may sit for it to spread onto
/// this dirt — a `(2·R+1)³` neighbourhood. Spread is pure proximity: whatever sits
/// *between* the two cells is irrelevant.
/// One knob; the world reads it through the behaviour.
pub const SPREAD_RADIUS: i32 = 2;

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
    fn key(&self) -> &'static str {
        "dirt"
    }

    fn has_random_tick(&self) -> bool {
        true
    }

    fn random_tick(&self, world: &mut dyn BehaviorWorld, pos: IVec3) {
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

/// Everything this module's relocated tests (in the engine crate) exercise.
/// Test-support builds only; never a public api surface.
#[cfg(any(test, feature = "test-support"))]
pub mod test_exports {
    pub use crate::block::Block;
    pub use super::DIRT;
    pub use crate::mathh::IVec3;
    pub use super::SPREAD_RADIUS;
    pub use super::grass_within;
    #[allow(unused_imports)]
    pub use super::*;
}
