//! Presentation-only inertia of the hands. Impulses come from changes in body
//! velocity and look; the simulation eye and the crosshair never wear them.
use glam::Vec3;
use petramond_math::math::wrap_angle;

/// A frame longer than this (seconds) resets the spring: a hitch must not
/// read as one enormous velocity change.
const MAX_FRAME_SECONDS: f32 = 0.15;
/// A position jump longer than this (blocks) between frames is a teleport —
/// the offset resets rather than carrying a phantom impulse.
const TELEPORT_DISTANCE: f32 = 3.0;
/// Per-axis clamp (blocks/s) on the velocity change one frame may inject.
const MAX_VELOCITY_DELTA: f32 = 16.0;
/// View-space impulse (blocks of hand travel per block/s of body velocity
/// change) per axis: sideways, vertical (landings), and forward.
const IMPULSE_SIDE: f32 = 0.12;
const IMPULSE_UP: f32 = 0.075;
const IMPULSE_FORWARD: f32 = 0.09;
/// Largest look change (radians) one frame may feed the offset.
const MAX_LOOK_DELTA: f32 = 0.35;
/// View-space offset (blocks) per radian of yaw and of pitch change — the
/// hand lags a turn of the head.
const LOOK_YAW_LAG: f32 = 0.065;
const LOOK_PITCH_LAG: f32 = 0.055;
/// Critically damped spring rate (1/s) the offset recovers at.
const SPRING_RATE: f32 = 16.0;
/// Clamps on the offset (blocks) and its velocity (blocks/s): the hand never
/// leaves its screen corner however hard the body is thrown.
const MAX_OFFSET: f32 = 0.055;
const MAX_OFFSET_VELOCITY: f32 = 2.0;

#[derive(Default)]
pub(super) struct HandMotion {
    offset: Vec3,
    velocity: Vec3,
    previous: Option<MotionSample>,
}

#[derive(Clone, Copy)]
pub(super) struct MotionSample {
    pub position: Vec3,
    pub velocity: Vec3,
    pub yaw: f32,
    pub pitch: f32,
}

impl HandMotion {
    pub fn advance(&mut self, dt: f32, sample: MotionSample, enabled: bool) {
        if !enabled || !dt.is_finite() || dt > MAX_FRAME_SECONDS {
            *self = Self::default();
            return;
        }
        if dt <= 0.0 {
            return;
        }
        let previous = self.previous.replace(sample);
        let Some(previous) = previous else {
            return;
        };
        if sample.position.distance_squared(previous.position) > TELEPORT_DISTANCE.powi(2) {
            self.offset = Vec3::ZERO;
            self.velocity = Vec3::ZERO;
            return;
        }
        let delta = (sample.velocity - previous.velocity).clamp(
            Vec3::splat(-MAX_VELOCITY_DELTA),
            Vec3::splat(MAX_VELOCITY_DELTA),
        );
        let right = -delta.x * sample.yaw.cos() + delta.z * sample.yaw.sin();
        let forward = delta.x * sample.yaw.sin() + delta.z * sample.yaw.cos();
        self.velocity += Vec3::new(
            -right * IMPULSE_SIDE,
            -delta.y * IMPULSE_UP,
            forward * IMPULSE_FORWARD,
        );
        let yaw_delta =
            wrap_angle(sample.yaw - previous.yaw).clamp(-MAX_LOOK_DELTA, MAX_LOOK_DELTA);
        let pitch_delta = (sample.pitch - previous.pitch).clamp(-MAX_LOOK_DELTA, MAX_LOOK_DELTA);
        self.offset.x += yaw_delta * LOOK_YAW_LAG;
        self.offset.y -= pitch_delta * LOOK_PITCH_LAG;
        settle(&mut self.offset, &mut self.velocity, SPRING_RATE, dt);
        self.offset = self
            .offset
            .clamp(Vec3::splat(-MAX_OFFSET), Vec3::splat(MAX_OFFSET));
        self.velocity = self.velocity.clamp(
            Vec3::splat(-MAX_OFFSET_VELOCITY),
            Vec3::splat(MAX_OFFSET_VELOCITY),
        );
    }

    pub fn offset(&self) -> [f32; 3] {
        self.offset.to_array()
    }
}

// Exact critically damped spring: a frame hitch cannot explode the integrator,
// and subdividing a quiet recovery does not change its duration or trajectory.
fn settle(position: &mut Vec3, velocity: &mut Vec3, rate: f32, dt: f32) {
    let decay = (-rate * dt).exp();
    let c = *velocity + *position * rate;
    *position = (*position + c * dt) * decay;
    *velocity = (*velocity - c * (rate * dt)) * decay;
}

#[cfg(test)]
mod tests;
