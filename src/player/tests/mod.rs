use super::{
    collision::Axis,
    movement::{
        friction_retain, Surroundings, AIR_ACCEL, AIR_FRICTION, CLIMB_LATERAL_SPEED, CLIMB_SPEED,
        FRICTION_REF_DT, GRAVITY, GROUND_ACCEL, GROUND_FRICTION, SPECTATOR_SPEED, SPRINT, WALK,
    },
    *,
};
use petramond_math::math::{IVec3, SelectionShape, Vec3};
use petramond_math::world_pos::WorldPos;
use petramond_world::block::Block;
use petramond_world::fluid::Buoyancy;

mod fall;
mod fluid_rays;
mod fluid_rows;
mod health;
mod ice;
mod ladders;
mod locomotion;
mod modes;
mod sneaking;
mod sweep;
mod targeting;

fn p(feet: WorldPos) -> Player {
    Player::new(feet)
}
