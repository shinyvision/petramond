use petramond::player::{AnimatorClaims, RigId};
use petramond_math::math::Vec3;
use petramond_render::camera::Camera;
use petramond_render::HeldItemFrame;

use super::Game;
use crate::animation::{AimTarget, LocalMotion};

pub struct SubjectHands {
    pub frames: [HeldItemFrame; 2],
    pub animator: AnimatorClaims,
    pub events: Vec<(RigId, u16)>,
    pub motion: LocalMotion,
}

impl Game {
    pub fn view_subject(&self) -> Option<mod_api::PlayerId> {
        self.local.third_person.subject()
    }

    fn subject_remote(&self) -> Option<&super::remote_players::RemotePlayer> {
        let id = self.view_subject()?;
        self.replica
            .entities
            .players()
            .iter_with_ids()
            .find(|(pid, _)| pid.0 == id.0)
            .map(|(_, remote)| remote)
            .filter(|remote| remote.curr.visible)
    }

    pub(super) fn subject_eye_camera(&self) -> Option<Camera> {
        if let Some(cam) = self.captured_eye_camera() {
            return Some(cam);
        }
        let remote = self.subject_remote()?;
        let (feet, yaw, pitch) =
            super::remote_players::interpolate(&remote.prev, &remote.curr, self.tick_alpha());
        let mut cam = self.local.cam.clone();
        cam.pos = feet + Vec3::new(0.0, petramond::player::EYE, 0.0);
        cam.yaw = yaw;
        cam.pitch = pitch;
        cam.roll = 0.0;
        Some(cam)
    }

    pub(crate) fn hidden_remote_body(&self) -> Option<petramond::player::PlayerId> {
        let subject = self.view_subject()?;
        (!self.camera_claimed() && !self.third_person_enabled())
            .then_some(petramond::player::PlayerId(subject.0))
    }

    pub fn subject_hands(&self) -> Option<SubjectHands> {
        if self.camera_claimed() || self.third_person_enabled() {
            return None;
        }
        if let Some(view) = self.captured_subject_view() {
            return Some(SubjectHands {
                frames: view.hands,
                animator: view.animator.clone(),
                events: view.events.clone(),
                motion: view.motion,
            });
        }
        let remote = self.subject_remote()?;
        let row = &remote.curr;
        let vel = row.transform.vel;
        let yaw = row.transform.yaw;
        let forward = Vec3::new(yaw.sin(), 0.0, yaw.cos());
        let right = Vec3::new(-yaw.cos(), 0.0, yaw.sin());
        Some(SubjectHands {
            frames: remote.frames,
            animator: AnimatorClaims {
                params: row.animator.params.clone(),
                plays: remote.plays.clone(),
            },
            events: remote.events.clone(),
            motion: LocalMotion {
                speed: Vec3::new(vel.x, 0.0, vel.z).length(),
                forward: vel.dot(forward),
                strafe: vel.dot(right),
                vertical: vel.y,
                grounded: row.on_ground,
                sneaking: row.sneaking,
                sprinting: false,
                swimming: false,
                climbing: false,
                pitch: row.transform.pitch.to_degrees(),
                yaw_rate: 0.0,
                pitch_rate: 0.0,
                stride: 0.0,
                stride_weight: 0.0,
                hurt: remote.hurt_flash01(),
                target: AimTarget::Nothing,
            },
        })
    }
}
