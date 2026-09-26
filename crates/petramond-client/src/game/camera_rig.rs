//! The first-person camera's presentation easing: where the eye sits relative
//! to the predicted body this frame, and how the lens widens with speed.
//!
//! Everything here is visual only. The body's feet, collision box and sim eye
//! update immediately in the local physics; the rig only eases what the
//! player SEES — the step-up glide, the sneak dip, the walking sway, the
//! pillow-height eye in bed and the speed-coupled FOV.

use petramond_math::math::Vec3;
use petramond_math::world_pos::WorldPos;

use super::first_person::LookRate;
use super::speed_fov::SpeedFov;
use super::view_bob::ViewBob;

const STEP_CAMERA_SETTLE_SPEED: f32 = 12.0;
/// Below this, a vertical speed or height change is no motion at all.
pub(super) const STEP_CAMERA_EPS: f32 = 0.001;
/// Camera height above the feet while asleep: head-on-the-pillow, a touch
/// above the mattress the body is standing on (vs the standing `player::EYE`).
const SLEEP_EYE_HEIGHT: f32 = 0.25;
/// Peak lateral camera sway while walking, in blocks. Subtle on purpose: this
/// is the channel a player FEELS, and a big one is what makes view bob a motion
/// sickness complaint rather than a sense of weight.
const BOB_EYE_SWAY: f32 = 0.045;
/// Peak vertical camera rise/dip while walking, in blocks — deliberately far
/// smaller than the sway. Vertical camera motion is the more nauseating axis
/// and it also fights the step-up glide, which owns real height changes.
const BOB_EYE_RISE: f32 = 0.018;
/// How far the first-person eye drops while sneaking (blocks) — the in-view
/// feedback that sneak is active. CAMERA ONLY: the sim eye (`player::EYE`),
/// reach, and the collision box stay full height, exactly like the sleep
/// pillow-height camera. Well inside the server's `REACH + 1` target-latch
/// slack, so a raycast from the lowered eye never trips reach validation.
const SNEAK_EYE_DROP: f32 = 0.3;
/// Exponential settle rate of the sneak eye drop — matches the body pose's
/// sneak blend rate so the first-person dip and the third-person crouch land
/// together.
const SNEAK_EYE_SETTLE_SPEED: f32 = 10.0;

/// The body facts one frame's eye placement reads.
#[derive(Copy, Clone, Debug)]
pub(super) struct EyeInputs {
    /// The body's sim eye this frame.
    pub eye: WorldPos,
    /// The body's feet height (the sleeping eye sits on the pillow above it).
    pub feet_y: f64,
    /// Grounded and not moving vertically — the only state a height change
    /// can be a step in.
    pub grounded_still: bool,
    /// Something else carries the body (a mount) or it flies through blocks
    /// (a spectator): height changes are never steps.
    pub carried: bool,
    /// The sneak intent the physics consumed (spectators never crouch).
    pub sneaking: bool,
    /// The body is walking in a way that sways the view.
    pub striding: bool,
    /// The body's horizontal speed (blocks/second).
    pub hspeed: f32,
    /// The body lies in bed.
    pub sleeping: bool,
    /// The camera's yaw this frame — the sway is lateral in its frame.
    pub yaw: f32,
}

pub(super) struct CameraRig {
    /// Visual-only vertical lag after grounded auto-step movement: upward
    /// after a step-up, downward after a sneak snap-down.
    step_y_offset: f32,
    /// Visual-only eased eye drop while sneaking (`0` upright …
    /// `-SNEAK_EYE_DROP` crouched).
    sneak_y_offset: f32,
    /// The body's eye height last frame, for step detection.
    last_eye_y: f64,
    /// First-person walking sway — a presentation offset on the camera, and
    /// the stride phase the first-person animator's walk plays on.
    view_bob: ViewBob,
    /// The look's turn rates, advanced once per frame with the look.
    look_rate: LookRate,
    /// Speed-coupled FOV — the camera widens with the body's WISHED land
    /// speed, a presentation retarget of the authored FOV.
    speed_fov: SpeedFov,
}

impl CameraRig {
    /// A rig at rest for a body whose eye is at `eye_y`, widening from the
    /// authored `base_fov_y`.
    pub(super) fn new(base_fov_y: f32, eye_y: f64) -> Self {
        Self {
            step_y_offset: 0.0,
            sneak_y_offset: 0.0,
            last_eye_y: eye_y,
            view_bob: ViewBob::default(),
            look_rate: LookRate::default(),
            speed_fov: SpeedFov::new(base_fov_y),
        }
    }

    /// Advance the lens by one frame: ease the FOV toward the body's wished
    /// `speed_ratio` (of walking speed) and track the look's turn rate.
    /// Returns this frame's FOV.
    pub(super) fn advance_lens(&mut self, dt: f32, speed_ratio: f32, yaw: f32, pitch: f32) -> f32 {
        self.speed_fov.advance(dt, speed_ratio);
        self.look_rate.advance(dt, yaw, pitch);
        self.speed_fov.fov_y()
    }

    /// Where the camera sits this frame for a body described by `body`.
    pub(super) fn place_eye(&mut self, dt: f32, body: EyeInputs) -> WorldPos {
        let target = body.eye;
        let eye_dy = (target.y - self.last_eye_y) as f32;
        let step = petramond_world::collision::STEP_HEIGHT;
        if body.carried {
            // A mount carries the body: a seat rising up a slope is not a
            // step, and gliding it would draw the rider under the seat.
            self.step_y_offset = 0.0;
        } else if body.grounded_still
            && eye_dy > STEP_CAMERA_EPS
            && eye_dy <= step + STEP_CAMERA_EPS
        {
            let max_lag = step * 1.5;
            self.step_y_offset = (self.step_y_offset - eye_dy).max(-max_lag);
        } else if body.grounded_still
            && body.sneaking
            && (-(step + STEP_CAMERA_EPS)..-STEP_CAMERA_EPS).contains(&eye_dy)
        {
            // The sneak snap-down: physics dropped the feet onto the lower step
            // instantly (grounded throughout); the camera starts the step ABOVE
            // and settles down — the mirror of the step-up glide. Sneak-gated
            // so ordinary landing dips keep their un-eased feel.
            let max_lag = step * 1.5;
            self.step_y_offset = (self.step_y_offset - eye_dy).min(max_lag);
        }

        let settle = 1.0 - (-STEP_CAMERA_SETTLE_SPEED * dt.max(0.0)).exp();
        self.step_y_offset += (0.0 - self.step_y_offset) * settle;
        if self.step_y_offset.abs() <= STEP_CAMERA_EPS {
            self.step_y_offset = 0.0;
        }

        // The sneak eye drop eases toward its target so crouching dips instead
        // of teleporting; the same intent drives the third-person stance blend.
        let sneak_target = if body.sneaking { -SNEAK_EYE_DROP } else { 0.0 };
        let sneak_settle = 1.0 - (-SNEAK_EYE_SETTLE_SPEED * dt.max(0.0)).exp();
        self.sneak_y_offset += (sneak_target - self.sneak_y_offset) * sneak_settle;

        // Walking sway: a body that is not striding simply eases back to rest
        // rather than being special-cased at the apply.
        self.view_bob.advance(dt, body.hspeed, body.striding);
        let [bob_side, bob_up] = self.view_bob.offset();

        // Lying in bed: the body stays a standing collision box on the
        // mattress, but the camera drops to pillow height.
        let eye_y = if body.sleeping {
            body.feet_y + f64::from(SLEEP_EYE_HEIGHT)
        } else {
            target.y + f64::from(self.step_y_offset + self.sneak_y_offset + bob_up * BOB_EYE_RISE)
        };
        // The sway is LATERAL in the camera's own frame, so it reads as the
        // body swinging under the head whichever way the player is looking;
        // the horizontal right vector ignores pitch, so looking up or down
        // cannot tilt the sway out of the horizon.
        let (sin_yaw, cos_yaw) = body.yaw.sin_cos();
        let right = Vec3::new(cos_yaw, 0.0, -sin_yaw);
        self.last_eye_y = target.y;
        WorldPos::new(target.x, eye_y, target.z) + right * (bob_side * BOB_EYE_SWAY)
    }

    /// The current step-glide lag (blocks) — the third-person body and its
    /// emitters sit on the same eased height as the eye.
    #[inline]
    pub(super) fn step_y_offset(&self) -> f32 {
        self.step_y_offset
    }

    /// The look's turn rates (degrees per second, rightward and upward).
    #[inline]
    pub(super) fn look_rates(&self) -> (f32, f32) {
        self.look_rate.rates()
    }

    /// The walking stride phase and its weight, for the first-person walk.
    #[inline]
    pub(super) fn stride(&self) -> (f32, f32) {
        self.view_bob.stride()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn still(eye_y: f64) -> EyeInputs {
        EyeInputs {
            eye: WorldPos::new(0.0, eye_y, 0.0),
            feet_y: eye_y - 1.62,
            grounded_still: true,
            carried: false,
            sneaking: false,
            striding: false,
            hspeed: 0.0,
            sleeping: false,
            yaw: 0.0,
        }
    }

    #[test]
    fn a_step_up_glides_and_settles() {
        let mut rig = CameraRig::new(1.2, 65.62);
        let placed = rig.place_eye(1.0 / 60.0, still(66.12));
        assert!(placed.y < 66.12, "the eye trails the step: {}", placed.y);
        for _ in 0..120 {
            rig.place_eye(1.0 / 60.0, still(66.12));
        }
        assert_eq!(rig.step_y_offset(), 0.0, "the glide settles");
    }

    #[test]
    fn a_carried_body_never_glides() {
        let mut rig = CameraRig::new(1.2, 65.62);
        let placed = rig.place_eye(
            1.0 / 60.0,
            EyeInputs {
                carried: true,
                ..still(66.12)
            },
        );
        assert_eq!(placed.y, 66.12);
        assert_eq!(rig.step_y_offset(), 0.0);
    }

    #[test]
    fn a_sleeper_sees_from_the_pillow() {
        let mut rig = CameraRig::new(1.2, 65.62);
        let placed = rig.place_eye(
            1.0 / 60.0,
            EyeInputs {
                sleeping: true,
                ..still(65.62)
            },
        );
        assert!((placed.y - (64.0 + f64::from(SLEEP_EYE_HEIGHT))).abs() < 1e-6);
    }
}
