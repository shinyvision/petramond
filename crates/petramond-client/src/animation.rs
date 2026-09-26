//! Player animation, evaluated on the client and handed to the renderer as
//! finished pose arrays.
//!
//! Every stateful piece of a player's animation lives here — the body
//! animators (one per body on the roster), the locomotion layer table, the
//! animator claims, montages and fired events, the first-person viewmodel's
//! animator and its camera bone, and the eased held pose of the local hands —
//! and advances with the frame's `dt`. The renderer only skins what comes
//! out: model-space bone matrices per body, a placement, and the viewmodel's
//! bones. Nothing here needs a GPU, so a second viewport, a paused renderer
//! or a test can run it all headlessly.
//!
//! A frame runs in the one order the data needs, and the types enforce it:
//! [`PlayerAnimation::begin_frame`] takes the local player's input (both
//! hands' frames, the resolved claims and fired events, the first-person
//! motion) and answers the renderer's [`LocalFrame`] — which the world view
//! must have before it is set up, since it wears the viewmodel's camera
//! bone. The presentation gather then fills [`BodyFrame`] with every body it
//! sees, and [`PlayerAnimation::pose_bodies`] advances every animator on the
//! roster — off-screen and first-person bodies included, so nothing they did
//! unseen replays as a stale edge — and poses the ones the view can see into
//! render rows.

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

/// The half-extent of the box a body is posed for when any of it is in view
/// (feet-relative, so the top reaches past a standing head).
const BODY_CULL_PAD: glam::Vec3 = glam::Vec3::new(1.0, 2.2, 1.0);

/// One player body this frame as the presentation gather reads it: where it
/// stands and how it is lit, what its animation reads, and the claimed bone
/// offsets composed over the pose.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct BodyInput {
    /// Feet centre (model `y=0`).
    pub pos: WorldPos,
    /// Multiply body tint from the body's active emitter bundles (its
    /// conditions' stage emitters), composed with the hurt flash.
    pub emitter_tint: [f32; 3],
    /// Body self-lighting from the active bundles (`0..=1`, strongest wins).
    pub emitter_self_lit: f32,
    pub skylight: u8,
    pub blocklight: BlockLight6,
    pub state: BodyState,
    /// This body's bone offsets, as a range into [`BodyFrame::bone_offsets`].
    pub bones: ArenaRange,
}

/// One REMOTE player's body: its [`BodyInput`] plus that remote's own eased
/// held views, hand frames and animator claims (the local body's are the
/// local frame's).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct RemoteBody {
    /// Stable across frames (the player id), so this body keeps its animator.
    pub key: u32,
    pub body: BodyInput,
    /// Each hand's eased held view (`[main, off]`).
    pub held: [HeldItemView; 2],
    /// The two hands' frames (`[main, off]`) the body animator reads.
    pub frames: [HeldItemFrame; 2],
    /// This body's claims and fired events, as ranges into the frame's arenas.
    pub animator: AnimatorRanges,
}

/// Every player body the presentation gather saw this frame, with the arenas
/// their ranges index into. Cleared and refilled each frame, capacity kept.
#[derive(Default)]
pub struct BodyFrame {
    /// The local third-person body; `None` in first person.
    pub local: Option<BodyInput>,
    /// Every visible remote player.
    pub remotes: Vec<RemoteBody>,
    /// Every body's claimed bone offsets, back to back.
    pub bone_offsets: Vec<BoneOffset>,
    /// Every remote body's animator claims and fired graph events, back to
    /// back like the bone offsets.
    pub params: Vec<AnimatorParamRow>,
    pub plays: Vec<AnimatorPlay>,
    pub events: Vec<(RigId, u16)>,
    /// Name-valued claims, interned once per distinct name.
    pub names: NameCache,
}

impl BodyFrame {
    /// Empty every row and arena for the next gather.
    pub fn clear(&mut self) {
        self.local = None;
        self.remotes.clear();
        self.bone_offsets.clear();
        self.params.clear();
        self.plays.clear();
        self.events.clear();
    }

    /// Append one body's bone offsets and return the range addressing them.
    pub fn push_bones(&mut self, bones: &[BoneOffset]) -> ArenaRange {
        let range = ArenaRange::next(&self.bone_offsets, bones.len());
        self.bone_offsets.extend_from_slice(bones);
        range
    }

    /// Append one remote body's claims and fired events and return the
    /// ranges addressing them.
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

/// The local player's animation input for one frame.
pub struct LocalInput<'a> {
    /// Both hands' frames, `[main, off]`.
    pub hands: [HeldItemFrame; 2],
    /// The local body's resolved animator claims for both rigs.
    pub claims: &'a AnimatorClaims,
    /// The graph events fired on the local rigs since the last frame.
    pub events: &'a [(RigId, u16)],
    /// What the body is doing, as the first-person animator reads it.
    pub motion: LocalMotion,
    /// The hurt-flash envelope (`0..1`, the one driving the screen
    /// vignette), which flashes the third-person body red like a hurt mob.
    pub hurt_flash: f32,
}

/// The local player's frame state between [`PlayerAnimation::begin_frame`]
/// and [`PlayerAnimation::pose_bodies`].
#[derive(Default)]
struct LocalState {
    /// Both hands' frames; `None` before the first frame of a world.
    frames: Option<[HeldItemFrame; 2]>,
    /// Both hands' eased held views.
    held: [HeldItemView; 2],
    /// The body's hurt flash (see [`LocalInput::hurt_flash`]).
    hurt_flash: f32,
    /// Each hand's pose ease.
    ease: [HeldItemEase; 2],
    /// The resolved claims and the fired events, for both local rigs.
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

/// Every player animator on the client and the frame's posed output.
pub struct PlayerAnimation {
    /// The viewmodel's animator; `None` without the rig or its graph.
    first_person: Option<FirstPersonAnimator>,
    /// Every roster body's animator.
    bodies: BodyAnimators,
    local: LocalState,
    /// Seconds since the last frame, set by `begin_frame` and consumed by
    /// `pose_bodies`: a second pose pass before the next frame advances
    /// nothing.
    dt: f32,
    /// This frame's bodies, as the presentation gather fills them.
    pub frame: BodyFrame,
    /// The posed render rows and the pose arena their ranges index into.
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

    /// Drop every world-scoped animation state: each animator, the local
    /// hands' eases, and the frame's rows. A stale pose or a pending edge
    /// must not survive into the next world.
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

    /// Open the frame, `dt` seconds after the last: ease both local hands'
    /// held poses, keep the local claims and events for the body pass, and
    /// advance the first-person animator. Answers what the renderer needs of
    /// the local player before the world view is set up.
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

    /// Advance every body animator on the roster by this frame's `dt` and
    /// pose each body `view` can see into a render row. The local body with
    /// no body to read (first person) and every culled body still advance,
    /// unposed. Consumes the frame's local events and `dt`, so a second pass
    /// before the next [`begin_frame`](Self::begin_frame) fires nothing
    /// again.
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
        // Edges, consumed by this pass: a redraw before the next frame's
        // claims must not fire them again.
        local.events.clear();
    }

    /// The posed bodies of the last [`pose_bodies`](Self::pose_bodies), the
    /// local one first when it was posed.
    pub fn bodies(&self) -> &[PlayerBodyRender] {
        &self.rows
    }

    /// The pose arena the posed bodies' ranges index into.
    pub fn poses(&self) -> &[Mat4] {
        &self.poses
    }
}

/// Pose `body` into `poses` and build its render row.
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
