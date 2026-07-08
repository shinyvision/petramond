//! Third-person view state: the collision-clamped boom camera and the player
//! body's presentation pose (body yaw vs head yaw, walk-cycle phase).
//!
//! All of it is per-frame presentation layered over the unchanged sim: `Game.cam`
//! stays the authoritative first-person EYE (every raycast, streaming, audio and
//! reach consumer keeps reading it), and the boom camera exists only as the
//! render/frame camera returned by [`Game::render_camera`]. The body pose is the
//! shared [`BodyPose`] helper (`game/body_pose.rs`) driven from the per-frame
//! player state, which is already smooth — no tick interpolation needed. Remote
//! players drive the SAME helper from interpolated replicated rows
//! (`game/remote_players.rs`), so there is exactly one pose implementation.

use crate::camera::Camera;
use crate::mathh::Vec3;

use super::body_pose::BodyPose;
use super::Game;

/// How far behind the eye the third-person camera wants to sit (blocks).
const BOOM_DIST: f32 = 4.0;
/// Clearance the boom keeps from any collision box, so the near plane (0.1)
/// never intersects a wall the camera is pushed against.
const CAM_PAD: f32 = 0.2;
/// Downward pitch of the sleep camera (radians, ~52°): it looks AT the lying
/// body from above the foot end, instead of booming behind the pillow-height
/// eye and ending up under the bed.
const SLEEP_CAM_PITCH: f32 = -0.9;

#[derive(Default)]
pub(super) struct ThirdPerson {
    pub(super) enabled: bool,
    /// The body's presentation pose (body yaw, walk phase/blend) — the shared
    /// helper remote players also drive.
    pub(super) pose: BodyPose,
    /// The boom camera computed this frame, when enabled.
    pub(super) cam: Option<Camera>,
}

impl Game {
    pub fn toggle_third_person(&mut self) {
        self.third_person.enabled = !self.third_person.enabled;
        if self.third_person.enabled {
            // Entering third person: face the body where the player looks and
            // restart the walk cycle, so the model never pops in mid-turn.
            self.third_person.pose.reset_facing(self.player.yaw);
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
        // behind the pillow-height eye would end up under the bed. The asleep
        // flag reads the replicated self view; `sleep_head_yaw` derives from
        // the session's bed cell against the REPLICA's model group.
        if self.self_view.sleeping.is_some() {
            let head_yaw = self
                .sleep_head_yaw()
                .unwrap_or(self.player.yaw);
            self.third_person.pose.lie(head_yaw);
            let mut cam = self.cam.clone();
            cam.yaw = head_yaw;
            cam.pitch = SLEEP_CAM_PITCH;
            let pos = self.player.pos;
            let target = Vec3::new(pos.x, pos.y + 0.5, pos.z);
            let back = -cam.forward();
            let world = &self.replica;
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

        let vel = self.player.vel;
        let hspeed = Vec3::new(vel.x, 0.0, vel.z).length();
        self.third_person.pose.advance(
            dt,
            hspeed,
            self.player.yaw,
            !self.player.is_spectator(),
        );

        // Boom camera: retreat from the eye opposite the look direction, stopped
        // early by any block collision box so the camera never enters geometry.
        let mut cam = self.cam.clone();
        let back = -cam.forward();
        let world = &self.replica;
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
