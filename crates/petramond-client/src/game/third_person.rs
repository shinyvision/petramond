//! Third-person view state: the collision-clamped boom camera and the player
//! body's presentation pose (body yaw vs head yaw, walk-cycle phase) — and the
//! client mods' VIEW CLAIMS layered over both.
//!
//! All of it is per-frame presentation layered over the unchanged sim: `LocalPlayer::cam`
//! stays the authoritative first-person EYE (every raycast, streaming and
//! reach consumer keeps reading it), and the boom camera or a claimed camera
//! exists only as the render/frame camera returned by [`Game::render_camera`],
//! the ONE place the frame's camera is decided. The body pose is the
//! shared [`BodyPose`] helper (`game/body_pose.rs`) driven from the per-frame
//! player state, which is already smooth — no tick interpolation needed. Remote
//! players drive the SAME helper from interpolated replicated rows
//! (`game/remote_players.rs`), so there is exactly one pose implementation.
//!
//! The camera resolves in this order: a mod's camera claim, then the boom when
//! the PRESENTED perspective is third person (a mod's perspective claim, else
//! the player's own toggle), then the eye.

use petramond::modding::client::view::{ViewCameraClaim, ViewChromeClaim, ViewFold};
use petramond::world::environment::ShaderParamMap;
use petramond_math::math::Vec3;
use petramond_math::world_pos::WorldPos;
use petramond_render::camera::Camera;

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
    /// The player's OWN perspective toggle. A perspective claim presents over
    /// it without touching it, so releasing the claim restores it exactly.
    pub(super) enabled: bool,
    /// The body's presentation pose (body yaw, walk phase/blend) — the shared
    /// helper remote players also drive.
    pub(super) pose: BodyPose,
    /// The boom camera computed this frame, when enabled.
    pub(super) cam: Option<Camera>,
    /// The client mods' view claims, resolved for this frame by
    /// [`Game::apply_view_fold`].
    claimed: ClaimedView,
}

#[derive(Default)]
struct ClaimedView {
    third_person: Option<bool>,
    camera: Option<Camera>,
    chrome: ViewChromeClaim,
    env: ShaderParamMap,
    frame_size: Option<[u32; 2]>,
    subject: Option<mod_api::PlayerId>,
    /// The subject's view as this frame's camera: their eye, or the boom
    /// behind them in third person.
    subject_cam: Option<Camera>,
}

impl ThirdPerson {
    /// The claimed view subject, whoever it is.
    pub(super) fn subject(&self) -> Option<mod_api::PlayerId> {
        self.claimed.subject
    }
}

impl Game {
    /// The player's perspective key. While a mod holds a perspective claim the
    /// key does NOTHING: the claim owns what presents, and a toggle that only
    /// took effect on release would change the view with no key pressed.
    pub fn toggle_third_person(&mut self) {
        if self.local.third_person.claimed.third_person.is_some() {
            return;
        }
        let was = self.presentation_state();
        self.local.third_person.enabled = !self.local.third_person.enabled;
        self.present_transition(was);
    }

    /// Whether the PRESENTED perspective is third person: a mod's
    /// perspective claim, else the player's own toggle.
    #[inline]
    pub fn third_person_enabled(&self) -> bool {
        self.local
            .third_person
            .claimed
            .third_person
            .unwrap_or(self.local.third_person.enabled)
    }

    /// Whether the frame's camera comes from a mod's view claim.
    #[inline]
    pub fn camera_claimed(&self) -> bool {
        self.local.third_person.claimed.camera.is_some()
    }

    /// The resolved chrome claims (`None` per field = no mod has an opinion).
    #[inline]
    pub fn view_chrome(&self) -> ViewChromeClaim {
        self.local.third_person.claimed.chrome
    }

    /// Whether the local body draws: whenever the camera is not the eye —
    /// the boom once it is placed, or any claimed camera — and the body is
    /// one others would see (a spectator has none, exactly as its replicated
    /// row is hidden).
    pub(super) fn presents_local_body(&self) -> bool {
        !self.local.player.is_spectator()
            && (self.camera_claimed()
                || (self.third_person_enabled() && self.local.third_person.cam.is_some()))
    }

    /// The camera the frame renders with: a claimed camera, else the boom in
    /// third person, else the first-person eye. Sim consumers keep reading
    /// `self.local.cam`.
    #[inline]
    pub(crate) fn render_camera(&self) -> &Camera {
        if let Some(cam) = &self.local.third_person.claimed.camera {
            return cam;
        }
        if let Some(cam) = &self.local.third_person.claimed.subject_cam {
            return cam;
        }
        match &self.local.third_person.cam {
            Some(cam) if self.third_person_enabled() => cam,
            _ => &self.local.cam,
        }
    }

    /// Replicated shader params with this frame's client overrides on top.
    ///
    /// Overriding the engine's `petramond:time` scrubs the sky: its light is
    /// re-derived through [`sky_params`](petramond::rules::daynight::sky_params)
    /// — the one sky model — unless `petramond:light` is overridden too.
    pub(super) fn presented_shader_params(
        &self,
        replicated: &std::sync::Arc<ShaderParamMap>,
    ) -> std::sync::Arc<ShaderParamMap> {
        use petramond::rules::daynight::{sky_params, SKY_LIGHT_PARAM, SKY_TIME_PARAM};
        let env = &self.local.third_person.claimed.env;
        if env.is_empty() {
            return replicated.clone();
        }
        let mut params = (**replicated).clone();
        if let Some(time) = env.get(SKY_TIME_PARAM) {
            let (time, light) = sky_params(time[0], time[2]);
            params.insert(SKY_TIME_PARAM.into(), time);
            params.insert(SKY_LIGHT_PARAM.into(), light);
        }
        for (key, value) in env {
            if key != SKY_TIME_PARAM {
                params.insert(key.clone(), *value);
            }
        }
        std::sync::Arc::new(params)
    }

    /// Resolve this frame's view claims from every client mod. Called once
    /// per frame, before anything reads the render camera.
    pub fn present_view_claims(&mut self) {
        self.publish_presented_entities();
        self.present_captured_view();
        let fold = self.client_mods.view_claims();
        self.apply_view_fold(fold);
    }

    /// The frame size a mod claims for the world, if any.
    pub fn view_frame_size(&self) -> Option<[u32; 2]> {
        self.local.third_person.claimed.frame_size
    }

    /// [`present_view_claims`](Self::present_view_claims) over an already
    /// folded set — the seam tests drive without a mod.
    pub(crate) fn apply_view_fold(&mut self, fold: ViewFold) {
        let was = self.presentation_state();
        self.local.third_person.claimed.third_person = fold.third_person;
        self.anchor_missing = false;
        self.local.third_person.claimed.camera = match fold.camera {
            Some(claim) => self.claimed_camera(claim),
            None => None,
        };
        self.local.third_person.claimed.chrome = fold.chrome;
        self.local.third_person.claimed.env = fold.env;
        self.local.third_person.claimed.frame_size = fold.frame_size;
        // Our own view is simply the view: a subject is someone else.
        self.local.third_person.claimed.subject = fold
            .subject
            .filter(|id| id.0 != self.replica.entities.self_id().0);
        self.local.third_person.claimed.subject_cam = None;
        if self.local.third_person.claimed.camera.is_none() {
            if let Some(eye) = self.subject_eye_camera() {
                let cam = if self.third_person_enabled() {
                    self.boom_behind(eye)
                } else {
                    eye
                };
                self.local.third_person.claimed.subject_cam = Some(cam);
            }
        }
        self.present_transition(was);
    }

    /// Stand in for client mod `mod_id`'s own view calls.
    #[cfg(test)]
    pub(crate) fn claim_view_for_test(
        &mut self,
        mod_id: &str,
        claims: petramond::modding::client::view::ViewClaims,
    ) -> bool {
        self.client_mods.set_view_claims_for_test(mod_id, claims)
    }

    /// The claim as a camera. The eye lends what the claim does not carry —
    /// aspect, clip planes and, with no `fov_y`, the player's LIVE field of
    /// view (speed coupling included), re-read every frame so a claim never
    /// freezes it.
    fn claimed_camera(&mut self, claim: ViewCameraClaim) -> Option<Camera> {
        let border = f64::from(petramond_world::border::WORLD_BORDER);
        let [x, y, z] = match claim.anchor {
            Some(anchor) => {
                let feet = self.anchor_feet(anchor)?;
                let [dx, dy, dz] = claim.pos;
                let at = feet.as_dvec3() + glam::DVec3::new(dx, dy, dz);
                [
                    at.x.clamp(-border, border),
                    at.y,
                    at.z.clamp(-border, border),
                ]
            }
            None => claim.pos,
        };
        let mut cam = self.local.cam.clone();
        // The border clamp at the call bounds x/z only; a view point also
        // needs y inside the i32 cell range its render origin snaps in.
        cam.pos = WorldPos::new(x, y.clamp(-border, border), z);
        cam.yaw = claim.yaw;
        cam.pitch = claim.pitch;
        cam.roll = claim.roll;
        cam.fov_y = claim.fov_y.unwrap_or(self.local.cam.fov_y);
        Some(cam)
    }

    /// The engine's boom behind an eye: retreat opposite the look, stopped
    /// short of any collision box so the camera never enters geometry.
    fn boom_behind(&self, mut cam: Camera) -> Camera {
        let back = -cam.forward();
        let world = &self.replica.world;
        let dist = petramond_world::collision::clamp_padded_segment(
            [cam.pos.x, cam.pos.y, cam.pos.z],
            [back.x, back.y, back.z],
            BOOM_DIST,
            CAM_PAD,
            |x, y, z| world.data().collision_boxes_at(x, y, z),
        );
        cam.pos += back * dist;
        cam
    }

    fn presentation_state(&self) -> (bool, bool) {
        (self.third_person_enabled(), self.presents_body_pose())
    }

    /// Whether the body pose is live: third person or a claimed camera.
    fn presents_body_pose(&self) -> bool {
        self.third_person_enabled() || self.camera_claimed()
    }

    /// Present a change of perspective or body visibility on THIS frame: the
    /// change can land between the game tick and the render, and a frame
    /// rendered with the body visible but the boom still at the eye looks out
    /// from inside the head.
    fn present_transition(&mut self, (boom_was, body_was): (bool, bool)) {
        let (boom, body) = self.presentation_state();
        if body && !body_was {
            // Face the body where the player looks and restart the walk
            // cycle, so the model never pops in mid-turn.
            self.local
                .third_person
                .pose
                .reset_facing(self.local.player.yaw);
        }
        if !boom {
            self.local.third_person.cam = None;
        }
        if (boom && !boom_was) || (body && !body_was) {
            self.update_third_person(0.0);
        }
    }

    /// Per-frame third-person update, after player movement and the camera-eye
    /// sync: advance the walk phase, follow the body yaw, and place the boom
    /// camera clamped against block collision. A claimed camera shows the
    /// body too, so the pose runs whenever either presents.
    pub(super) fn update_third_person(&mut self, dt: f32) {
        if !self.presents_body_pose() {
            return;
        }
        let boom = self.third_person_enabled();

        // Asleep: the body lies in the bed (head toward the pillow) and the
        // camera looks DOWN at the player from above the foot end — a boom
        // behind the pillow-height eye would end up under the bed. The asleep
        // flag reads the replicated self view; `sleep_head_yaw` derives from
        // the session's bed cell against the REPLICA's model group.
        if self.replica.self_view.sleeping.is_some() {
            let head_yaw = self
                .replica
                .sleep_head_yaw()
                .unwrap_or(self.local.player.yaw);
            self.local.third_person.pose.lie(head_yaw);
            if !boom {
                return;
            }
            let mut cam = self.local.cam.clone();
            cam.yaw = head_yaw;
            cam.pitch = SLEEP_CAM_PITCH;
            let target = self.local.player.pos + Vec3::new(0.0, 0.5, 0.0);
            let back = -cam.forward();
            let world = &self.replica.world;
            let dist = petramond_world::collision::clamp_padded_segment(
                target.to_array(),
                [back.x, back.y, back.z],
                BOOM_DIST,
                CAM_PAD,
                |x, y, z| world.data().collision_boxes_at(x, y, z),
            );
            cam.pos = target + back * dist;
            self.local.third_person.cam = Some(cam);
            return;
        }

        self.local.third_person.pose.advance(
            dt,
            super::body_pose::MotionFrame {
                position: self.local.player.pos,
                velocity: self.local.player.vel,
                yaw: self.local.player.yaw,
                grounded: self.local.player.on_ground,
                medium: super::body_pose::movement_medium(
                    self.replica.world.data(),
                    self.local.player.pos,
                ),
                enabled: !self.local.player.is_spectator()
                    && self.replica.entities.own_mount().is_none(),
                sneaking: self.local.predicted_input.sneak,
            },
        );

        if !boom {
            return;
        }
        self.local.third_person.cam = Some(self.boom_behind(self.local.cam.clone()));
    }
}
