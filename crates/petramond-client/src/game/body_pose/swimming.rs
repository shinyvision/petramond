//! The swimming half of the body pose: the water blend and the stroke clock.

use super::{BodyPose, MotionFrame, MovementMedium, SPEED_EPSILON};

/// Exponential rate every swim weight settles at — slower than the land
/// crossfades, because water entry and exit are gradual events.
const SWIM_BLEND_RATE: f32 = 8.0;
/// Below this swim weight an exiting swimmer's blend is dropped to rest.
const SWIM_REST_WEIGHT: f32 = 0.001;
/// Stroke weight ramps from 0 at `SWIM_MOVE_START` blocks/s to 1 over
/// `SWIM_MOVE_RAMP` more: a drifting swimmer treads, a swimming one strokes.
const SWIM_MOVE_START: f32 = 0.15;
const SWIM_MOVE_RAMP: f32 = 2.2;
/// Backstroke weight ramps over the velocity's backward component (fraction
/// of speed) from `SWIM_BACKWARD_DEAD_ZONE` over `SWIM_BACKWARD_RAMP`.
const SWIM_BACKWARD_DEAD_ZONE: f32 = 0.1;
const SWIM_BACKWARD_RAMP: f32 = 0.7;
/// Ascent weight ramps from `SWIM_RISE_START` blocks/s upward over
/// `SWIM_RISE_RAMP` more.
const SWIM_RISE_START: f32 = 0.2;
const SWIM_RISE_RAMP: f32 = 1.8;
/// Stroke clock rate (cycles/s): treading water, plus what a full stroke and a
/// full ascent each add — arms work harder when going somewhere.
const STROKE_RATE_TREAD: f32 = 0.48;
const STROKE_RATE_MOVING: f32 = 0.42;
const STROKE_RATE_RISING: f32 = 0.16;

impl BodyPose {
    pub(super) fn advance_swimming(&mut self, dt: f32, frame: &MotionFrame) {
        let swimming = frame.medium == MovementMedium::Swimming;
        let blend = &mut self.locomotion.swim;
        let ease = 1.0 - (-SWIM_BLEND_RATE * dt).exp();
        let target = if swimming { 1.0 } else { 0.0 };
        blend.weight += (target - blend.weight) * ease;
        if !swimming && blend.weight < SWIM_REST_WEIGHT {
            *blend = Default::default();
            return;
        }
        let v = frame.velocity;
        let speed = v.x.hypot(v.z);
        if swimming {
            let grounded = if frame.grounded { 1.0 } else { 0.0 };
            blend.grounded += (grounded - blend.grounded) * ease;
            let forward =
                (v.x * frame.yaw.sin() + v.z * frame.yaw.cos()) / speed.max(SPEED_EPSILON);
            let moving = ((speed - SWIM_MOVE_START) / SWIM_MOVE_RAMP).clamp(0.0, 1.0);
            let backward =
                ((-forward - SWIM_BACKWARD_DEAD_ZONE) / SWIM_BACKWARD_RAMP).clamp(0.0, 1.0);
            let rising = ((v.y - SWIM_RISE_START) / SWIM_RISE_RAMP).clamp(0.0, 1.0);
            blend.moving += (moving - blend.moving) * ease;
            blend.backward += (backward - blend.backward) * ease;
            blend.rising += (rising - blend.rising) * ease;
        }
        // Treading needs a live clock even over stationary feet. Continue the
        // outgoing stroke through the exit blend instead of freezing its arms.
        let rate = STROKE_RATE_TREAD
            + blend.moving * STROKE_RATE_MOVING
            + blend.rising * STROKE_RATE_RISING;
        blend.phase = (blend.phase + dt * rate).rem_euclid(1.0);
    }
}
