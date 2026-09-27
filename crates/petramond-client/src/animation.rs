mod body;
mod claims;
mod first_person;
mod footsteps;
mod inputs;
pub(crate) mod locomotion;
mod motion;
mod pose;
#[cfg(test)]
mod tests;

use glam::Mat4;
use petramond::player::rigs::{self, Presenter};
use petramond::player::{AnimatorClaims, AnimatorPlay, RigId};
use petramond_math::view_volume::ViewVolume;
use petramond_math::world_pos::WorldPos;
use petramond_render::{
    ArenaRange, HeldItemEase, HeldItemFrame, HeldItemView, LocalFrame, PlayerBodyRender,
    PlayerRenderInstance,
};
use petramond_world::light::BlockLight6;

pub use body::LOCAL_BODY;
pub use claims::{AnimatorInputs, AnimatorParamRow, AnimatorRanges, NameCache};
pub use first_person::FirstPersonAnimator;
pub use footsteps::FootstepSource;
pub use motion::{AimTarget, BodyState, BoneOffset, LocalMotion, LocomotionBlend, SwimBlend};

use body::BodyAnimators;

const BODY_CULL_PAD: glam::Vec3 = glam::Vec3::new(1.0, 2.2, 1.0);

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct BodyInput {
    pub pos: WorldPos,
    pub emitter_tint: [f32; 3],
    pub emitter_self_lit: f32,
    pub skylight: u8,
    pub blocklight: BlockLight6,
    pub state: BodyState,
    pub bones: ArenaRange,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct RemoteBody {
    pub key: u32,
    pub body: BodyInput,
    pub held: [HeldItemView; 2],
    pub frames: [HeldItemFrame; 2],
    pub animator: AnimatorRanges,
}

#[derive(Default)]
pub struct BodyFrame {
    pub local: Option<BodyInput>,
    pub remotes: Vec<RemoteBody>,
    pub bone_offsets: Vec<BoneOffset>,
    pub params: Vec<AnimatorParamRow>,
    pub plays: Vec<AnimatorPlay>,
    pub events: Vec<(RigId, u16)>,
    pub names: NameCache,
}

impl BodyFrame {
    pub fn clear(&mut self) {
        self.local = None;
        self.remotes.clear();
        self.bone_offsets.clear();
        self.params.clear();
        self.plays.clear();
        self.events.clear();
    }

    pub fn push_bones(&mut self, bones: &[BoneOffset]) -> ArenaRange {
        let range = ArenaRange::next(&self.bone_offsets, bones.len());
        self.bone_offsets.extend_from_slice(bones);
        range
    }

    pub fn push_animator(
        &mut self,
        claims: &AnimatorClaims,
        plays: &[AnimatorPlay],
        events: &[(RigId, u16)],
    ) -> AnimatorRanges {
        let ranges = AnimatorRanges {
            params: ArenaRange::next(&self.params, claims.params.len()),
            plays: ArenaRange::next(&self.plays, plays.len()),
            events: ArenaRange::next(&self.events, events.len()),
        };
        self.names.rows(&claims.params, &mut self.params);
        self.plays.extend_from_slice(plays);
        self.events.extend_from_slice(events);
        ranges
    }
}

pub struct LocalInput<'a> {
    pub hands: [HeldItemFrame; 2],
    pub claims: &'a AnimatorClaims,
    pub events: &'a [(RigId, u16)],
    pub motion: LocalMotion,
    pub hurt_flash: f32,
}

#[derive(Default)]
struct LocalState {
    frames: Option<[HeldItemFrame; 2]>,
    held: [HeldItemView; 2],
    hurt_flash: f32,
    ease: [HeldItemEase; 2],
    params: Vec<AnimatorParamRow>,
    plays: Vec<AnimatorPlay>,
    events: Vec<(RigId, u16)>,
    names: NameCache,
}

impl LocalState {
    fn inputs(&self) -> AnimatorInputs<'_> {
        AnimatorInputs {
            params: &self.params,
            plays: &self.plays,
            events: &self.events,
        }
    }
}

pub struct PlayerAnimation {
    first_person: Option<FirstPersonAnimator>,
    bodies: BodyAnimators,
    local: LocalState,
    dt: f32,
    pub frame: BodyFrame,
    rows: Vec<PlayerBodyRender>,
    poses: Vec<Mat4>,
}

impl Default for PlayerAnimation {
    fn default() -> Self {
        Self::new(FirstPersonAnimator::shipped(), BodyAnimators::shipped())
    }
}

impl PlayerAnimation {
    fn new(first_person: Option<FirstPersonAnimator>, bodies: BodyAnimators) -> Self {
        Self {
            first_person,
            bodies,
            local: LocalState::default(),
            dt: 0.0,
            frame: BodyFrame::default(),
            rows: Vec::new(),
            poses: Vec::new(),
        }
    }

    pub fn clear(&mut self) {
        if let Some(first_person) = &mut self.first_person {
            first_person.reset();
        }
        self.bodies.clear();
        let names = std::mem::take(&mut self.local.names);
        self.local = LocalState {
            names,
            ..Default::default()
        };
        self.dt = 0.0;
        self.frame.clear();
        self.rows.clear();
        self.poses.clear();
    }

    pub fn begin_frame(&mut self, input: LocalInput<'_>, dt: f32) -> LocalFrame<'_> {
        let local = &mut self.local;
        for ((held, ease), frame) in local.held.iter_mut().zip(&mut local.ease).zip(&input.hands) {
            *held = ease.update(frame, dt);
        }
        local.frames = Some(input.hands);
        local.hurt_flash = input.hurt_flash;
        local.params.clear();
        local.names.rows(&input.claims.params, &mut local.params);
        local.plays.clear();
        local.plays.extend_from_slice(&input.claims.plays);
        local.events.clear();
        local.events.extend_from_slice(input.events);
        self.dt = dt;
        if let Some(first_person) = &mut self.first_person {
            first_person.advance(&input.hands, &input.motion, local.inputs(), dt);
        }
        LocalFrame {
            held: local.held,
            first_person: self.first_person.as_ref().map_or(&[][..], |fp| fp.bones()),
        }
    }

    pub fn pose_bodies(&mut self, view: &ViewVolume) {
        let dt = std::mem::take(&mut self.dt);
        let Self {
            bodies,
            local,
            frame,
            rows,
            poses,
            ..
        } = self;
        rows.clear();
        poses.clear();
        bodies.retain(frame.remotes.iter().map(|r| r.key));
        let rig = rigs::presented(Presenter::Body).map(|(_, rig)| rig);
        let seen = |body: &BodyInput| {
            rig.is_some() && view.aabb_visible(body.pos - BODY_CULL_PAD, body.pos + BODY_CULL_PAD)
        };

        let local_frames = local.frames.as_ref();
        let local_body = frame.local.map(|mut body| {
            body.state.hurt = local.hurt_flash;
            body
        });
        match local_body.filter(seen).zip(rig) {
            Some((body, rig)) => {
                let drive = bodies.body(LOCAL_BODY).map(|animator| pose::BodyDrive {
                    animator,
                    frames: local_frames,
                    inputs: local.inputs(),
                    dt,
                });
                let row = pose_row(rig, &body, local.held, &frame.bone_offsets, drive, poses);
                rows.push(row);
            }
            None => {
                if let Some(animator) = bodies.body(LOCAL_BODY) {
                    let state = local_body.as_ref().map(|body| &body.state);
                    animator.advance(state, local_frames, local.inputs(), dt);
                }
            }
        }
        for remote in &frame.remotes {
            let inputs = remote
                .animator
                .of(&frame.params, &frame.plays, &frame.events);
            match rig.filter(|_| seen(&remote.body)) {
                Some(rig) => {
                    let drive = bodies.body(remote.key).map(|animator| pose::BodyDrive {
                        animator,
                        frames: Some(&remote.frames),
                        inputs,
                        dt,
                    });
                    let row = pose_row(
                        rig,
                        &remote.body,
                        remote.held,
                        &frame.bone_offsets,
                        drive,
                        poses,
                    );
                    rows.push(row);
                }
                None => {
                    if let Some(animator) = bodies.body(remote.key) {
                        animator.advance(
                            Some(&remote.body.state),
                            Some(&remote.frames),
                            inputs,
                            dt,
                        );
                    }
                }
            }
        }
        local.events.clear();
    }

    pub fn bodies(&self) -> &[PlayerBodyRender] {
        &self.rows
    }

    pub fn poses(&self) -> &[Mat4] {
        &self.poses
    }
}

fn pose_row(
    rig: &petramond::player::rigs::Rig,
    body: &BodyInput,
    held: [HeldItemView; 2],
    bone_offsets: &[BoneOffset],
    drive: Option<pose::BodyDrive<'_>>,
    poses: &mut Vec<Mat4>,
) -> PlayerBodyRender {
    let start = poses.len();
    let placement = pose::pose_body(rig, &body.state, body.bones.of(bone_offsets), drive, poses);
    PlayerBodyRender {
        body: PlayerRenderInstance {
            emitter_tint: body.emitter_tint,
            emitter_self_lit: body.emitter_self_lit,
            pos: body.pos,
            sleeping: body.state.sleeping,
            hurt: body.state.hurt,
            skylight: body.skylight,
            blocklight: body.blocklight,
            pose: ArenaRange::next(&poses[..start], poses.len() - start),
            placement,
        },
        held: held[0],
        held_off: held[1],
    }
}
