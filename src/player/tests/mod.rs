use super::{
    collision::Axis,
    movement::{
        friction_retain, AIR_ACCEL, AIR_FRICTION, CLIMB_LATERAL_SPEED, CLIMB_SPEED,
        FRICTION_REF_DT, GRAVITY, GROUND_ACCEL, GROUND_FRICTION, SPECTATOR_SPEED, SPRINT,
        SWIM_CLIMB, WALK, WATER_PROBE_Y,
    },
    *,
};
use crate::block::Block;
use crate::mathh::{IVec3, SelectionShape, Vec3};

mod fall;
mod health;
mod ice;
mod ladders;
mod locomotion;
mod modes;
mod sneaking;
mod sweep;
mod targeting;
mod water;

/// No water anywhere -- the dry-land predicate every physics test uses.
fn dry(_x: i32, _y: i32, _z: i32) -> bool {
    false
}

/// No ladders anywhere -- the climb predicate for tests off the ladder.
fn no_ladder(_x: i32, _y: i32, _z: i32) -> Option<crate::facing::Facing> {
    None
}

fn no_slip(_x: i32, _y: i32, _z: i32) -> bool {
    false
}

fn p(feet: Vec3) -> Player {
    Player::new(feet)
}
