const HEAD_YAW_LIMIT: f32 = std::f32::consts::FRAC_PI_4;
const BODY_ALIGN_RATE: f32 = 8.0;
const WALK_CYCLES_PER_BLOCK: f32 = 0.35;
const MOVING_SPEED_SQ: f32 = 0.05 * 0.05;
const WALK_BLEND_RATE: f32 = 10.0;
const SNEAK_BLEND_RATE: f32 = 10.0;
const STRIDE_SPEED_CAP: f32 = 12.0;
const MAX_FRAME_SECONDS: f32 = 0.1;
const TELEPORT_DISTANCE: f32 = 3.0;
const LOCOMOTION_BLEND_RATE: f32 = 12.0;
const WADE_STRIDE_DRAG: f32 = 0.3;
const BACKWARD_DEAD_ZONE: f32 = 0.15;
const BACKWARD_RAMP: f32 = 0.65;
const RUN_RAMP: f32 = 1.8;
const FALL_START: f32 = 0.5;
const FALL_RAMP: f32 = 5.0;
const LANDING_MIN_SPEED: f32 = 1.0;
const LANDING_RAMP: f32 = 11.0;
const LANDING_SECONDS: f32 = 0.32;
const LANDING_COMPRESS_FRACTION: f32 = 0.22;
const SWIM_TURN_SPEED: f32 = 0.1;
const SPEED_EPSILON: f32 = 0.001;

#[derive(Copy, Clone, Debug, Default)]
pub struct BodyPose {
    pub body_yaw: f32,
    pub anim_time: f32,
    pub moving: bool,
    pub walk_weight: f32,
    pub sneak_weight: f32,
    pub locomotion: crate::animation::LocomotionBlend,
    was_grounded: Option<bool>,
    previous_position: Option<petramond_math::world_pos::WorldPos>,
    fall_speed: f32,
    landing_time: f32,
    landing_strength: f32,
}

pub struct MotionFrame {
    pub position: petramond_math::world_pos::WorldPos,
    pub velocity: glam::Vec3,
    pub yaw: f32,
    pub grounded: bool,
    pub medium: MovementMedium,
    pub enabled: bool,
    pub sneaking: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MovementMedium {
    Land,
    Swimming,
    Climbing,
}

pub(super) fn movement_medium(
    world: &petramond_world::world::WorldData,
    pos: petramond_math::world_pos::WorldPos,
) -> MovementMedium {
    let x = pos.x.floor() as i32;
    let z = pos.z.floor() as i32;
    if world
        .body_fluid(
            pos,
            petramond::player::HEIGHT,
            petramond_world::fluid::Buoyancy::Swim,
        )
        .is_some()
    {
        MovementMedium::Swimming
    } else if petramond_world::block::Block::from_id(world.chunk_block(x, pos.y.floor() as i32, z))
        .is_climbable()
    {
        MovementMedium::Climbing
    } else {
        MovementMedium::Land
    }
}
mod swimming;

impl BodyPose {
    pub fn advance(&mut self, dt: f32, frame: MotionFrame) {
        if !dt.is_finite() || dt <= 0.0 {
            return;
        }
        if self.previous_position.is_some_and(|pos| {
            pos.distance_squared(frame.position) > f64::from(TELEPORT_DISTANCE * TELEPORT_DISTANCE)
        }) {
            self.reset_facing(frame.yaw);
        }
        self.previous_position = Some(frame.position);
        let dt = dt.min(MAX_FRAME_SECONDS);
        let v = frame.velocity;
        let speed = v.x.hypot(v.z);
        let wading = self.locomotion.swim.grounded * self.locomotion.swim.weight;
        self.advance_gait(
            dt,
            speed * (1.0 - WADE_STRIDE_DRAG * wading),
            frame.yaw,
            frame.enabled && frame.grounded && frame.medium != MovementMedium::Climbing,
            frame.sneaking,
        );
        if !frame.enabled || frame.medium == MovementMedium::Climbing {
            self.locomotion = Default::default();
            self.was_grounded = None;
            self.fall_speed = 0.0;
            self.landing_strength = 0.0;
            return;
        }
        self.advance_swimming(dt, &frame);
        if frame.medium == MovementMedium::Swimming {
            self.was_grounded = None;
            self.fall_speed = 0.0;
            self.landing_strength = 0.0;
            self.locomotion.landing = 0.0;
            self.locomotion.airborne *= (-LOCOMOTION_BLEND_RATE * dt).exp();
            self.body_yaw = follow_body_yaw(self.body_yaw, frame.yaw, speed > SWIM_TURN_SPEED, dt);
            return;
        }
        let speed_floor = speed.max(SPEED_EPSILON);
        let forward = (v.x * self.body_yaw.sin() + v.z * self.body_yaw.cos()) / speed_floor;
        let side = (-v.x * self.body_yaw.cos() + v.z * self.body_yaw.sin()) / speed_floor;
        let ease = 1.0 - (-LOCOMOTION_BLEND_RATE * dt).exp();
        let backward = ((-forward - BACKWARD_DEAD_ZONE) / BACKWARD_RAMP).clamp(0.0, 1.0);
        let run = ((speed - petramond::player::WALK) / RUN_RAMP).clamp(0.0, 1.0) * (1.0 - backward);
        self.locomotion.run += (run - self.locomotion.run) * ease;
        self.locomotion.backward += (backward - self.locomotion.backward) * ease;
        self.locomotion.strafe += (side * self.walk_weight - self.locomotion.strafe) * ease;
        let air = if frame.grounded { 0.0 } else { 1.0 };
        self.locomotion.airborne += (air - self.locomotion.airborne) * ease;
        let fall = ((-v.y + FALL_START) / FALL_RAMP).clamp(0.0, 1.0);
        self.locomotion.falling += (fall - self.locomotion.falling) * ease;
        if !frame.grounded {
            self.fall_speed = self.fall_speed.max(-v.y);
        } else if self.was_grounded == Some(false) {
            self.landing_strength =
                ((self.fall_speed - LANDING_MIN_SPEED) / LANDING_RAMP).clamp(0.0, 1.0);
            self.landing_time = 0.0;
            self.fall_speed = 0.0;
        }
        self.was_grounded = Some(frame.grounded);
        self.landing_time += dt;
        let t = (self.landing_time / LANDING_SECONDS).clamp(0.0, 1.0);
        let envelope = if t < LANDING_COMPRESS_FRACTION {
            smooth(t / LANDING_COMPRESS_FRACTION)
        } else {
            1.0 - smooth((t - LANDING_COMPRESS_FRACTION) / (1.0 - LANDING_COMPRESS_FRACTION))
        };
        self.locomotion.landing = envelope * self.landing_strength * (1.0 - air);
    }

    pub fn reset_facing(&mut self, yaw: f32) {
        *self = Self::default();
        self.body_yaw = yaw;
    }

    pub fn lie(&mut self, body_yaw: f32) {
        *self = Self::default();
        self.body_yaw = body_yaw;
    }

    fn advance_gait(
        &mut self,
        dt: f32,
        hspeed: f32,
        head_yaw: f32,
        can_move: bool,
        sneaking: bool,
    ) {
        self.moving = hspeed * hspeed > MOVING_SPEED_SQ && can_move;
        let starting = self.moving && self.walk_weight == 0.0;
        let sneak_target = if sneaking && can_move { 1.0 } else { 0.0 };
        let sneak_settle = 1.0 - (-SNEAK_BLEND_RATE * dt.max(0.0)).exp();
        self.sneak_weight += (sneak_target - self.sneak_weight) * sneak_settle;
        if self.sneak_weight < 0.01 && sneak_target == 0.0 {
            self.sneak_weight = 0.0;
        }
        let target = if self.moving { 1.0 } else { 0.0 };
        let settle = 1.0 - (-WALK_BLEND_RATE * dt.max(0.0)).exp();
        self.walk_weight += (target - self.walk_weight) * settle;
        if self.moving {
            if starting {
                self.anim_time = 0.0;
            }
            self.anim_time = (self.anim_time
                + dt.max(0.0) * hspeed.min(STRIDE_SPEED_CAP) * WALK_CYCLES_PER_BLOCK)
                .rem_euclid(1.0);
        } else if self.walk_weight < 0.01 {
            self.walk_weight = 0.0;
        }
        self.body_yaw = follow_body_yaw(self.body_yaw, head_yaw, self.moving, dt);
    }
}

pub fn follow_body_yaw(body_yaw: f32, head_yaw: f32, moving: bool, dt: f32) -> f32 {
    let mut body = body_yaw;
    if moving {
        let settle = 1.0 - (-BODY_ALIGN_RATE * dt.max(0.0)).exp();
        body += wrap_angle(head_yaw - body) * settle;
    }
    let diff = wrap_angle(head_yaw - body);
    let clamped = diff.clamp(-HEAD_YAW_LIMIT, HEAD_YAW_LIMIT);
    head_yaw - clamped
}

fn smooth(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

pub use petramond_math::math::{lerp_angle, wrap_angle};

#[cfg(test)]
mod tests;
