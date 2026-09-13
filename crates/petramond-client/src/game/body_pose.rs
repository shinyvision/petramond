//! The player-body presentation pose shared by the LOCAL third-person view and
//! every REMOTE player: body-yaw follow (head turns freely inside a limit, the
//! body is dragged past it and re-aligns while walking) and the walk-cycle
//! phase/blend. ONE implementation — `game/third_person.rs` drives it from the
//! predicted player, `game/remote_players.rs` from interpolated replicated
//! rows. Everything here is per-frame presentation; nothing feeds the sim.

/// How far the head may yaw away from the body before the body is dragged
/// along (radians, ~45°).
const HEAD_YAW_LIMIT: f32 = std::f32::consts::FRAC_PI_4;
/// Exponential rate at which the body re-aligns to the look direction while
/// walking (a walking body faces where it goes).
const BODY_ALIGN_RATE: f32 = 8.0;
/// Walk-cycle phase advance per block walked (cycles/block): ties the authored
/// 1 s `walk` loop to actual ground speed so sprinting swings faster.
const WALK_CYCLES_PER_BLOCK: f32 = 0.35;
/// Horizontal speed (blocks/s) below which the player counts as standing.
const MOVING_SPEED_SQ: f32 = 0.05 * 0.05;
/// Exponential rate the walk↔stand pose blend settles at, so stopping eases the
/// limbs back to rest instead of snapping to the rest pose in one frame.
const WALK_BLEND_RATE: f32 = 10.0;
/// Exponential rate the stand↔sneak stance blend settles at — the same feel as
/// the walk blend, so crouching down and rising ease identically.
const SNEAK_BLEND_RATE: f32 = 10.0;
/// Ground speed (blocks/s) above which the stride phase stops accelerating:
/// past this the legs would blur rather than read as faster steps.
const STRIDE_SPEED_CAP: f32 = 12.0;
/// A frame longer than this (seconds) is clamped: an alt-tab hitch must not
/// integrate a whole second of pose motion in one step.
const MAX_FRAME_SECONDS: f32 = 0.1;
/// A position jump longer than this (blocks) between two frames is a
/// teleport — the pose snaps to rest instead of spinning across it.
const TELEPORT_DISTANCE: f32 = 3.0;
/// Exponential rate every locomotion weight (run, backward, lateral, air,
/// falling) settles at — one shared feel for all the crossfades.
const LOCOMOTION_BLEND_RATE: f32 = 12.0;
/// Wading (swimming with the feet on the floor) slows the stride phase by this
/// fraction at full swim weight — water drag on the legs.
const WADE_STRIDE_DRAG: f32 = 0.3;
/// Backpedal weight ramp: 0 when the velocity's backward component is
/// `BACKWARD_DEAD_ZONE` (fraction of speed), 1 at `BACKWARD_DEAD_ZONE +
/// BACKWARD_RAMP` — a slight rearward drift is still a forward walk.
const BACKWARD_DEAD_ZONE: f32 = 0.15;
const BACKWARD_RAMP: f32 = 0.65;
/// Run weight ramps from 0 at walking speed to 1 this many blocks/s above it.
const RUN_RAMP: f32 = 1.8;
/// Falling weight ramps from 0 at `-FALL_START` blocks/s of descent to 1 over
/// `FALL_RAMP` more: a small hop shows the rise pose, a drop shows the fall.
const FALL_START: f32 = 0.5;
const FALL_RAMP: f32 = 5.0;
/// Landing strength: 0 for a touchdown at `LANDING_MIN_SPEED` blocks/s or
/// slower (a step off a block), 1 at `LANDING_MIN_SPEED + LANDING_RAMP`
/// (terminal-ish velocity).
const LANDING_MIN_SPEED: f32 = 1.0;
const LANDING_RAMP: f32 = 11.0;
/// The landing envelope's total duration (seconds) and the fraction of it
/// spent compressing — fast compression, longer recovery.
const LANDING_SECONDS: f32 = 0.32;
const LANDING_COMPRESS_FRACTION: f32 = 0.22;
/// Horizontal speed (blocks/s) above which a swimmer's body follows the look.
const SWIM_TURN_SPEED: f32 = 0.1;
/// Speed floor (blocks/s) for the direction normalizations — avoids a divide
/// by zero over planted feet without moving the weights.
const SPEED_EPSILON: f32 = 0.001;

/// A player body's presentation pose, advanced once per frame.
#[derive(Copy, Clone, Debug, Default)]
pub struct BodyPose {
    /// The body's facing yaw (engine yaw space, like `Player::yaw`). Trails
    /// the head within `HEAD_YAW_LIMIT`; re-aligns while walking.
    pub body_yaw: f32,
    /// Normalized stride phase shared by every locomotion clip.
    pub anim_time: f32,
    pub moving: bool,
    /// Walk-pose blend weight (`0` standing … `1` full walk cycle), eased
    /// toward `moving` so starts and stops transition instead of snapping.
    pub walk_weight: f32,
    /// Sneak-stance blend weight (`0` upright … `1` fully crouched), eased
    /// toward the sneak intent. The renderer cross-fades the sneak animation
    /// in by this: its FRAME 0 is the standing-still stance, and while moving
    /// the same clip's cycle replaces the walk cycle.
    pub sneak_weight: f32,
    pub locomotion: petramond_render::views::LocomotionBlend,
    was_grounded: Option<bool>,
    previous_position: Option<petramond_math::world_pos::WorldPos>,
    fall_speed: f32,
    landing_time: f32,
    landing_strength: f32,
}

/// Movement facts sampled by either the predicted player or a remote replica.
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

pub(super) fn land_motion(
    world: &petramond_world::world::WorldData,
    pos: petramond_math::world_pos::WorldPos,
) -> bool {
    movement_medium(world, pos) == MovementMedium::Land
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
        // The envelope shapes only the TIMING; the compressed pose is the asset's.
        let t = (self.landing_time / LANDING_SECONDS).clamp(0.0, 1.0);
        let envelope = if t < LANDING_COMPRESS_FRACTION {
            smooth(t / LANDING_COMPRESS_FRACTION)
        } else {
            1.0 - smooth((t - LANDING_COMPRESS_FRACTION) / (1.0 - LANDING_COMPRESS_FRACTION))
        };
        self.locomotion.landing = envelope * self.landing_strength * (1.0 - air);
    }

    /// Snap the pose to face `yaw` at rest — entering third person, a remote
    /// player appearing, or a replicated teleport (`snap`), so the model never
    /// pops in mid-turn or spins across a jump.
    pub fn reset_facing(&mut self, yaw: f32) {
        *self = Self::default();
        self.body_yaw = yaw;
    }

    /// Freeze into the lying pose: the body faces `body_yaw` (the bed's
    /// base→pillow yaw) with the walk cycle fully rested.
    pub fn lie(&mut self, body_yaw: f32) {
        *self = Self::default();
        self.body_yaw = body_yaw;
    }

    /// One frame of the pose: ease the walk blend toward the moving state,
    /// advance the phase by ground speed, and follow the body yaw behind the
    /// head (`head_yaw` = the look yaw). `can_move` gates the walk animation
    /// (false for spectators — the local body never draws for one, but the
    /// gate keeps both drivers identical). `sneaking` eases the sneak-stance
    /// blend in/out.
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
        // Stand↔sneak blends like walk↔stand: eased, with a snap-to-rest floor
        // so the weight actually reaches 0/1.
        let sneak_target = if sneaking && can_move { 1.0 } else { 0.0 };
        let sneak_settle = 1.0 - (-SNEAK_BLEND_RATE * dt.max(0.0)).exp();
        self.sneak_weight += (sneak_target - self.sneak_weight) * sneak_settle;
        if self.sneak_weight < 0.01 && sneak_target == 0.0 {
            self.sneak_weight = 0.0;
        }
        // Walk↔stand blends instead of snapping: the weight eases toward the
        // moving state; the phase advances only while moving (a stopping body
        // fades its frozen mid-stride pose back to rest).
        let target = if self.moving { 1.0 } else { 0.0 };
        let settle = 1.0 - (-WALK_BLEND_RATE * dt.max(0.0)).exp();
        self.walk_weight += (target - self.walk_weight) * settle;
        if self.moving {
            // Start each fresh stride at phase 0 (but never mid-blend, so a
            // quick stop-start doesn't pop the legs).
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

/// One frame of the body-yaw follow rule: the head (look) turns freely within
/// `HEAD_YAW_LIMIT` of the body; past it the body is dragged along so the
/// neck never over-twists, and while walking the body eases toward the look
/// direction (a walking body faces where it goes).
pub fn follow_body_yaw(body_yaw: f32, head_yaw: f32, moving: bool, dt: f32) -> f32 {
    let mut body = body_yaw;
    if moving {
        let settle = 1.0 - (-BODY_ALIGN_RATE * dt.max(0.0)).exp();
        body += wrap_angle(head_yaw - body) * settle;
    }
    let diff = wrap_angle(head_yaw - body);
    let clamped = diff.clamp(-HEAD_YAW_LIMIT, HEAD_YAW_LIMIT);
    // Re-derive from the head so the stored yaw stays numerically near it
    // instead of accumulating whole turns.
    head_yaw - clamped
}

fn smooth(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

pub use petramond_math::math::{lerp_angle, wrap_angle};

#[cfg(test)]
mod tests;
