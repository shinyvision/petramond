use petramond_math::math::Vec3;
use petramond_math::world_pos::WorldPos;

use super::first_person::LookRate;
use super::speed_fov::SpeedFov;
use super::view_bob::ViewBob;

const STEP_CAMERA_SETTLE_SPEED: f32 = 12.0;
pub(super) const STEP_CAMERA_EPS: f32 = 0.001;
const SLEEP_EYE_HEIGHT: f32 = 0.25;
const BOB_EYE_SWAY: f32 = 0.045;
const BOB_EYE_RISE: f32 = 0.018;
const SNEAK_EYE_DROP: f32 = 0.3;
const SNEAK_EYE_SETTLE_SPEED: f32 = 10.0;

#[derive(Copy, Clone, Debug)]
pub(super) struct EyeInputs {
    pub eye: WorldPos,
    pub feet_y: f64,
    pub grounded_still: bool,
    pub carried: bool,
    pub sneaking: bool,
    pub striding: bool,
    pub hspeed: f32,
    pub sleeping: bool,
    pub yaw: f32,
}

pub(super) struct CameraRig {
    step_y_offset: f32,
    sneak_y_offset: f32,
    last_eye_y: f64,
    view_bob: ViewBob,
    look_rate: LookRate,
    speed_fov: SpeedFov,
}

impl CameraRig {
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

    pub(super) fn advance_lens(&mut self, dt: f32, speed_ratio: f32, yaw: f32, pitch: f32) -> f32 {
        self.speed_fov.advance(dt, speed_ratio);
        self.look_rate.advance(dt, yaw, pitch);
        self.speed_fov.fov_y()
    }

    pub(super) fn place_eye(&mut self, dt: f32, body: EyeInputs) -> WorldPos {
        let target = body.eye;
        let eye_dy = (target.y - self.last_eye_y) as f32;
        let step = petramond_world::collision::STEP_HEIGHT;
        if body.carried {
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
            let max_lag = step * 1.5;
            self.step_y_offset = (self.step_y_offset - eye_dy).min(max_lag);
        }

        let settle = 1.0 - (-STEP_CAMERA_SETTLE_SPEED * dt.max(0.0)).exp();
        self.step_y_offset += (0.0 - self.step_y_offset) * settle;
        if self.step_y_offset.abs() <= STEP_CAMERA_EPS {
            self.step_y_offset = 0.0;
        }

        let sneak_target = if body.sneaking { -SNEAK_EYE_DROP } else { 0.0 };
        let sneak_settle = 1.0 - (-SNEAK_EYE_SETTLE_SPEED * dt.max(0.0)).exp();
        self.sneak_y_offset += (sneak_target - self.sneak_y_offset) * sneak_settle;

        self.view_bob.advance(dt, body.hspeed, body.striding);
        let [bob_side, bob_up] = self.view_bob.offset();

        let eye_y = if body.sleeping {
            body.feet_y + f64::from(SLEEP_EYE_HEIGHT)
        } else {
            target.y + f64::from(self.step_y_offset + self.sneak_y_offset + bob_up * BOB_EYE_RISE)
        };
        let (sin_yaw, cos_yaw) = body.yaw.sin_cos();
        let right = Vec3::new(cos_yaw, 0.0, -sin_yaw);
        self.last_eye_y = target.y;
        WorldPos::new(target.x, eye_y, target.z) + right * (bob_side * BOB_EYE_SWAY)
    }

    #[inline]
    pub(super) fn step_y_offset(&self) -> f32 {
        self.step_y_offset
    }

    #[inline]
    pub(super) fn look_rates(&self) -> (f32, f32) {
        self.look_rate.rates()
    }

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
