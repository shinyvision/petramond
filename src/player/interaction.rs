//! Player-side interaction rules over the shared block raycast
//! (`petramond_world::world::raycast`).

use petramond_math::math::{IVec3, Vec3};
use petramond_math::world_pos::WorldPos;
pub use petramond_world::world::raycast::{ray_vs_aabb, RayFilter, RaycastHit, REACH};

/// Server-side reach validation for a claimed block interaction: the CLOSEST
/// point of cell `block` within `REACH` (+1 slack for latency between the
/// claimed eye and the resolving tick) of `eye`. The one rule every
/// server-side block-reach check shares (look latch, `BreakFinished`).
pub fn block_within_reach(eye: WorldPos, block: IVec3) -> bool {
    let lo = WorldPos::block_min(block) - eye;
    let closest = Vec3::ZERO.clamp(lo, lo + Vec3::ONE);
    closest.length() <= REACH + 1.0
}
