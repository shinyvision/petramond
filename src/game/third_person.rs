//! Third-person view state: the collision-clamped boom camera and the player
//! body's presentation pose (body yaw vs head yaw, walk-cycle phase).
//!
//! All of it is per-frame presentation layered over the unchanged sim: `Game.cam`
//! stays the authoritative first-person EYE (every raycast, streaming, audio and
//! reach consumer keeps reading it), and the boom camera exists only as the
//! render/frame camera returned by [`Game::render_camera`]. The body pose mirrors
//! how mobs animate (walk animation while moving, head-look override on the
//! `head` bone) but is driven from the per-frame player state, which is already
//! smooth — no tick interpolation needed.

use crate::camera::Camera;
use crate::mathh::Vec3;

use super::Game;

/// How far behind the eye the third-person camera wants to sit (blocks).
const BOOM_DIST: f32 = 4.0;
/// Clearance the boom keeps from any collision box, so the near plane (0.1)
/// never intersects a wall the camera is pushed against.
const CAM_PAD: f32 = 0.2;
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
/// Downward pitch of the sleep camera (radians, ~52°): it looks AT the lying
/// body from above the foot end, instead of booming behind the pillow-height
/// eye and ending up under the bed.
const SLEEP_CAM_PITCH: f32 = -0.9;

#[derive(Default)]
pub(super) struct ThirdPerson {
    pub(super) enabled: bool,
    /// The body's facing yaw (engine yaw space, like `Player::yaw`). Trails the
    /// head within [`HEAD_YAW_LIMIT`]; re-aligns while walking.
    pub(super) body_yaw: f32,
    /// Seconds into the walk animation while `moving`.
    pub(super) anim_time: f32,
    pub(super) moving: bool,
    /// Walk-pose blend weight (`0` standing … `1` full walk cycle), eased toward
    /// `moving` so starts and stops transition instead of snapping.
    pub(super) walk_weight: f32,
    /// The boom camera computed this frame, when enabled.
    pub(super) cam: Option<Camera>,
}

impl Game {
    pub fn toggle_third_person(&mut self) {
        self.third_person.enabled = !self.third_person.enabled;
        if self.third_person.enabled {
            // Entering third person: face the body where the player looks and
            // restart the walk cycle, so the model never pops in mid-turn.
            self.third_person.body_yaw = self.player.yaw;
            self.third_person.anim_time = 0.0;
            self.third_person.walk_weight = 0.0;
            // Place the boom camera NOW: the toggle can land between the game
            // tick and the render, and a frame rendered with the body visible
            // but the camera still at the eye looks out from inside the head.
            self.update_third_person(0.0);
        } else {
            self.third_person.cam = None;
        }
    }

    #[inline]
    pub fn third_person_enabled(&self) -> bool {
        self.third_person.enabled
    }

    /// The camera the frame renders with: the boom camera in third person, the
    /// first-person eye otherwise. Sim consumers keep reading `self.cam`.
    #[inline]
    pub(super) fn render_camera(&self) -> &Camera {
        match &self.third_person.cam {
            Some(cam) if self.third_person.enabled => cam,
            _ => &self.cam,
        }
    }

    /// Per-frame third-person update, after player movement and the camera-eye
    /// sync: advance the walk phase, follow the body yaw, and place the boom
    /// camera clamped against block collision.
    pub(super) fn update_third_person(&mut self, dt: f32) {
        if !self.third_person.enabled {
            return;
        }

        // Asleep: the body lies in the bed (head toward the pillow) and the
        // camera looks DOWN at the player from above the foot end — a boom
        // behind the pillow-height eye would end up under the bed.
        if self.sleep.is_some() {
            let head_yaw = self.sleep_head_yaw().unwrap_or(self.player.yaw);
            self.third_person.body_yaw = head_yaw;
            self.third_person.moving = false;
            self.third_person.walk_weight = 0.0;
            let mut cam = self.cam.clone();
            cam.yaw = head_yaw;
            cam.pitch = SLEEP_CAM_PITCH;
            let target = Vec3::new(
                self.player.pos.x,
                self.player.pos.y + 0.5,
                self.player.pos.z,
            );
            let back = -cam.forward();
            let world = &self.world;
            let dist = crate::collision::clamp_padded_segment(
                [target.x, target.y, target.z],
                [back.x, back.y, back.z],
                BOOM_DIST,
                CAM_PAD,
                |x, y, z| world.collision_boxes_at(x, y, z),
            );
            cam.pos = target + back * dist;
            self.third_person.cam = Some(cam);
            return;
        }

        let hvel = Vec3::new(self.player.vel.x, 0.0, self.player.vel.z);
        let hspeed_sq = hvel.length_squared();
        self.third_person.moving = hspeed_sq > MOVING_SPEED_SQ && !self.player.is_spectator();
        // Walk↔stand blends instead of snapping: the weight eases toward the
        // moving state; the phase advances only while moving (a stopping body
        // fades its frozen mid-stride pose back to rest).
        let target = if self.third_person.moving { 1.0 } else { 0.0 };
        let settle = 1.0 - (-WALK_BLEND_RATE * dt.max(0.0)).exp();
        self.third_person.walk_weight += (target - self.third_person.walk_weight) * settle;
        if self.third_person.moving {
            // Start each fresh stride at phase 0 (but never mid-blend, so a quick
            // stop-start doesn't pop the legs).
            if self.third_person.walk_weight < 0.05 {
                self.third_person.anim_time = 0.0;
            }
            self.third_person.anim_time += dt * hspeed_sq.sqrt() * WALK_CYCLES_PER_BLOCK;
        } else if self.third_person.walk_weight < 0.01 {
            self.third_person.walk_weight = 0.0;
        }

        self.third_person.body_yaw = follow_body_yaw(
            self.third_person.body_yaw,
            self.player.yaw,
            self.third_person.moving,
            dt,
        );

        // Boom camera: retreat from the eye opposite the look direction, stopped
        // early by any block collision box so the camera never enters geometry.
        let mut cam = self.cam.clone();
        let back = -cam.forward();
        let world = &self.world;
        let dist = crate::collision::clamp_padded_segment(
            [cam.pos.x, cam.pos.y, cam.pos.z],
            [back.x, back.y, back.z],
            BOOM_DIST,
            CAM_PAD,
            |x, y, z| world.collision_boxes_at(x, y, z),
        );
        cam.pos += back * dist;
        self.third_person.cam = Some(cam);
    }
}

/// One frame of the body-yaw follow rule: the head (look) turns freely within
/// [`HEAD_YAW_LIMIT`] of the body; past it the body is dragged along so the
/// neck never over-twists, and while walking the body eases toward the look
/// direction (a walking body faces where it goes).
fn follow_body_yaw(body_yaw: f32, head_yaw: f32, moving: bool, dt: f32) -> f32 {
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

/// Wrap an angle difference into `(-π, π]`.
fn wrap_angle(a: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    let mut d = a % TAU;
    if d > PI {
        d -= TAU;
    } else if d < -PI {
        d += TAU;
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_look_turns_move_only_the_head() {
        // Idle, head 30° off the body: under the 45° threshold the body stays put.
        let body = follow_body_yaw(0.0, 30f32.to_radians(), false, 0.016);
        assert!(
            body.abs() < 1e-6,
            "body untouched under the threshold: {body}"
        );
    }

    #[test]
    fn past_the_threshold_the_body_is_dragged_along() {
        // Idle, head 80° off: the body is pulled so the head-body offset is
        // exactly the 45° limit.
        let head = 80f32.to_radians();
        let body = follow_body_yaw(0.0, head, false, 0.016);
        assert!(
            (wrap_angle(head - body) - HEAD_YAW_LIMIT).abs() < 1e-5,
            "offset clamps to the limit"
        );
        // Same on the other side.
        let body = follow_body_yaw(0.0, -head, false, 0.016);
        assert!((wrap_angle(-head - body) + HEAD_YAW_LIMIT).abs() < 1e-5);
    }

    #[test]
    fn walking_realigns_the_body_to_the_look() {
        // While moving the body converges to the head across frames, even when
        // the offset never crosses the drag threshold.
        let head = 30f32.to_radians();
        let mut body = 0.0;
        for _ in 0..120 {
            body = follow_body_yaw(body, head, true, 1.0 / 60.0);
        }
        assert!(
            wrap_angle(head - body).abs() < 0.01,
            "body aligned while walking: {body}"
        );
    }

    #[test]
    fn follow_handles_the_yaw_wrap_seam() {
        // Head just past +π, body just under -π: the true offset is tiny, so the
        // body must not spin the long way round.
        let head = std::f32::consts::PI - 0.05;
        let body0 = -std::f32::consts::PI + 0.05;
        let body = follow_body_yaw(body0, head, false, 0.016);
        assert!(
            wrap_angle(head - body).abs() <= HEAD_YAW_LIMIT + 1e-5,
            "wrap seam does not over-rotate"
        );
    }
}
