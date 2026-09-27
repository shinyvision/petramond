use std::collections::VecDeque;

use petramond::capture::view::{CueHand, CueMotion, CueShake, ViewCue};
use petramond::player::{AnimatorClaims, RigId};
use petramond_math::world_pos::WorldPos;
use petramond_render::camera::Camera;
use petramond_render::HeldItemFrame;

use super::frame::ClientHeldItem;
use super::replicated::SelfView;
use super::Game;
use crate::animation::{AimTarget, LocalMotion};

pub(super) struct ViewCues {
    cues: VecDeque<ViewCue>,
    events_through: f64,
    pub(super) own: Option<SelfView>,
    presented: Option<CapturedView>,
}

impl Default for ViewCues {
    fn default() -> Self {
        Self {
            cues: VecDeque::new(),
            events_through: f64::NEG_INFINITY,
            own: None,
            presented: None,
        }
    }
}

pub(super) struct CapturedView {
    pub subject: mod_api::PlayerId,
    pub pos: WorldPos,
    pub yaw: f32,
    pub pitch: f32,
    pub roll: f32,
    pub fov_y: f32,
    pub shake: CueShake,
    pub motion: LocalMotion,
    pub hands: [HeldItemFrame; 2],
    pub animator: AnimatorClaims,
    pub events: Vec<(RigId, u16)>,
}

impl Game {
    fn presented_tick(&self) -> f64 {
        self.replica.entities.committed_tick() as f64 - 1.0 + f64::from(self.tick_alpha())
    }

    pub fn eye_view_cue(
        &self,
        hands: [&ClientHeldItem; 2],
        animator: &AnimatorClaims,
        events: &[(RigId, u16)],
        shake: CueShake,
        motion: &LocalMotion,
    ) -> ViewCue {
        let cam = &self.local.cam;
        ViewCue {
            at: self.presented_tick(),
            pos: cam.pos,
            yaw: cam.yaw,
            pitch: cam.pitch,
            roll: cam.roll,
            fov_y: cam.fov_y,
            shake,
            motion: cue_motion(motion),
            hands: hands.map(cue_hand),
            hotbar: self.replica.self_view.inventory.active_slot(),
            animator: animator.clone(),
            events: events.to_vec(),
        }
    }

    pub(super) fn receive_view_cue(&mut self, cue: ViewCue) {
        let cues = &mut self.presenting.view.cues;
        if cues.back().is_none_or(|last| last.at < cue.at) {
            cues.push_back(cue);
        }
    }

    pub(super) fn present_captured_view(&mut self) {
        let (Some(at), Some(subject)) = (self.presentation_position(), self.captured_player())
        else {
            self.presenting.view.cues.clear();
            self.presenting.view.presented = None;
            return;
        };
        let view = &mut self.presenting.view;
        view.presented = None;
        if at < view.events_through || view.events_through == f64::NEG_INFINITY {
            view.events_through = at;
        }
        let mut events = Vec::new();
        for cue in view
            .cues
            .iter()
            .filter(|c| c.at > view.events_through && c.at <= at)
        {
            events.extend(cue.events.iter().copied());
        }
        view.events_through = at;
        while view.cues.len() >= 2 && view.cues[1].at <= at {
            view.cues.pop_front();
        }
        let Some(a) = view.cues.front().filter(|a| a.at <= at) else {
            return;
        };
        let b = view.cues.get(1);
        let t = b.map_or(0.0, |b| {
            ((at - a.at) / (b.at - a.at).max(1e-6)).clamp(0.0, 1.0)
        });
        let b = b.unwrap_or(a);
        let tf = t as f32;
        let lerp = |x: f32, y: f32| x + (y - x) * tf;
        if let Some(own) = view.own.as_mut() {
            own.inventory.set_active(a.hotbar);
        }
        let pos = a.pos.as_dvec3().lerp(b.pos.as_dvec3(), t);
        view.presented = Some(CapturedView {
            subject,
            pos: WorldPos::new(pos.x, pos.y, pos.z),
            yaw: a.yaw + wrap_angle(b.yaw - a.yaw) * tf,
            pitch: lerp(a.pitch, b.pitch),
            roll: lerp(a.roll, b.roll),
            fov_y: lerp(a.fov_y, b.fov_y),
            shake: CueShake {
                look: [
                    lerp(a.shake.look[0], b.shake.look[0]),
                    lerp(a.shake.look[1], b.shake.look[1]),
                ],
                hand: [
                    lerp(a.shake.hand[0], b.shake.hand[0]),
                    lerp(a.shake.hand[1], b.shake.hand[1]),
                ],
                flash: lerp(a.shake.flash, b.shake.flash),
            },
            motion: local_motion(&a.motion),
            hands: [held_frame(&a.hands[0]), held_frame(&a.hands[1])],
            animator: a.animator.clone(),
            events,
        });
    }

    pub(super) fn captured_subject_view(&self) -> Option<&CapturedView> {
        let subject = self.view_subject()?;
        self.presenting
            .view
            .presented
            .as_ref()
            .filter(|view| view.subject == subject)
    }

    pub fn captured_shake(&self) -> Option<CueShake> {
        if self.camera_claimed() || self.third_person_enabled() {
            return None;
        }
        self.captured_subject_view().map(|view| view.shake)
    }

    pub(super) fn captured_eye_camera(&self) -> Option<Camera> {
        let view = self.captured_subject_view()?;
        let mut cam = self.local.cam.clone();
        cam.pos = view.pos;
        cam.yaw = view.yaw;
        cam.pitch = view.pitch;
        cam.roll = view.roll;
        cam.fov_y = view.fov_y;
        Some(cam)
    }

    pub fn hud_view(&self) -> Option<&SelfView> {
        if !self.in_presentation() {
            return Some(&self.replica.self_view);
        }
        match self.view_subject() {
            None => Some(&self.replica.self_view),
            Some(subject) if Some(subject) == self.captured_player() => {
                self.presenting.view.own.as_ref()
            }
            Some(_) => None,
        }
    }
}

fn wrap_angle(a: f32) -> f32 {
    let tau = std::f32::consts::TAU;
    (a + std::f32::consts::PI).rem_euclid(tau) - std::f32::consts::PI
}

fn cue_hand(held: &ClientHeldItem) -> CueHand {
    CueHand {
        item: held.item.map(|i| i.0),
        display: held.display.map(|i| i.0),
        data: petramond_world::item::variant::blob(held.variant).map(|b| b.to_vec()),
        mining: held.mining,
        eating: held.eating,
        pose: held.pose_target,
    }
}

fn held_frame(hand: &CueHand) -> HeldItemFrame {
    HeldItemFrame {
        item: hand.item.map(petramond_world::item::ItemType),
        display: hand.display.map(petramond_world::item::ItemType),
        variant: hand
            .data
            .as_deref()
            .and_then(|blob| petramond_world::item::variant::intern_blob(blob).ok())
            .unwrap_or_default(),
        block_state: Default::default(),
        mining: hand.mining,
        eating: hand.eating,
        pose_target: hand.pose.map(super::render_held_pose),
    }
}

fn cue_motion(m: &LocalMotion) -> CueMotion {
    CueMotion {
        speed: m.speed,
        forward: m.forward,
        strafe: m.strafe,
        vertical: m.vertical,
        grounded: m.grounded,
        sneaking: m.sneaking,
        sprinting: m.sprinting,
        swimming: m.swimming,
        climbing: m.climbing,
        pitch: m.pitch,
        yaw_rate: m.yaw_rate,
        pitch_rate: m.pitch_rate,
        stride: m.stride,
        stride_weight: m.stride_weight,
        hurt: m.hurt,
        target: m.target as u8,
    }
}

fn local_motion(m: &CueMotion) -> LocalMotion {
    LocalMotion {
        speed: m.speed,
        forward: m.forward,
        strafe: m.strafe,
        vertical: m.vertical,
        grounded: m.grounded,
        sneaking: m.sneaking,
        sprinting: m.sprinting,
        swimming: m.swimming,
        climbing: m.climbing,
        pitch: m.pitch,
        yaw_rate: m.yaw_rate,
        pitch_rate: m.pitch_rate,
        stride: m.stride,
        stride_weight: m.stride_weight,
        hurt: m.hurt,
        target: match m.target {
            1 => AimTarget::Block,
            2 => AimTarget::Creature,
            _ => AimTarget::Nothing,
        },
    }
}
