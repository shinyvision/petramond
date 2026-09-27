use petramond::modding::client::view::{ViewCameraClaim, ViewChromeClaim, ViewFold};
use petramond::world::environment::ShaderParamMap;
use petramond_math::math::Vec3;
use petramond_math::world_pos::WorldPos;
use petramond_render::camera::Camera;

use super::body_pose::BodyPose;
use super::Game;

const BOOM_DIST: f32 = 4.0;
const CAM_PAD: f32 = 0.2;
const SLEEP_CAM_PITCH: f32 = -0.9;

#[derive(Default)]
pub(super) struct ThirdPerson {
    pub(super) enabled: bool,
    pub(super) pose: BodyPose,
    pub(super) cam: Option<Camera>,
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
    subject_cam: Option<Camera>,
}

impl ThirdPerson {
    pub(super) fn subject(&self) -> Option<mod_api::PlayerId> {
        self.claimed.subject
    }
}

impl Game {
    pub fn toggle_third_person(&mut self) {
        if self.local.third_person.claimed.third_person.is_some() {
            return;
        }
        let was = self.presentation_state();
        self.local.third_person.enabled = !self.local.third_person.enabled;
        self.present_transition(was);
    }

    #[inline]
    pub fn third_person_enabled(&self) -> bool {
        self.local
            .third_person
            .claimed
            .third_person
            .unwrap_or(self.local.third_person.enabled)
    }

    #[inline]
    pub fn camera_claimed(&self) -> bool {
        self.local.third_person.claimed.camera.is_some()
    }

    #[inline]
    pub fn view_chrome(&self) -> ViewChromeClaim {
        self.local.third_person.claimed.chrome
    }

    pub(super) fn presents_local_body(&self) -> bool {
        !self.local.player.is_spectator()
            && (self.camera_claimed()
                || (self.third_person_enabled() && self.local.third_person.cam.is_some()))
    }

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

    pub fn present_view_claims(&mut self) {
        self.publish_presented_entities();
        self.present_captured_view();
        let fold = self.client_mods.view_claims();
        self.apply_view_fold(fold);
    }

    pub fn view_frame_size(&self) -> Option<[u32; 2]> {
        self.local.third_person.claimed.frame_size
    }

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

    #[cfg(test)]
    pub(crate) fn claim_view_for_test(
        &mut self,
        mod_id: &str,
        claims: petramond::modding::client::view::ViewClaims,
    ) -> bool {
        self.client_mods.set_view_claims_for_test(mod_id, claims)
    }

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
        cam.pos = WorldPos::new(x, y.clamp(-border, border), z);
        cam.yaw = claim.yaw;
        cam.pitch = claim.pitch;
        cam.roll = claim.roll;
        cam.fov_y = claim.fov_y.unwrap_or(self.local.cam.fov_y);
        Some(cam)
    }

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

    fn presents_body_pose(&self) -> bool {
        self.third_person_enabled() || self.camera_claimed()
    }

    fn present_transition(&mut self, (boom_was, body_was): (bool, bool)) {
        let (boom, body) = self.presentation_state();
        if body && !body_was {
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
