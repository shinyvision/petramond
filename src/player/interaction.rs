use petramond_math::math::{IVec3, Vec3};
use petramond_math::world_pos::WorldPos;
pub use petramond_world::world::raycast::{ray_vs_aabb, RayFilter, RaycastHit, REACH};

pub fn block_within_reach(eye: WorldPos, block: IVec3) -> bool {
    let lo = WorldPos::block_min(block) - eye;
    let closest = Vec3::ZERO.clamp(lo, lo + Vec3::ONE);
    closest.length() <= REACH + 1.0
}
