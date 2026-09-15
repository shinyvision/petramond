//! Player bodies' animators: one per body, keyed by a stable body key, fed
//! each frame by the body's locomotion blend and its two hands' frames, and
//! evaluated OVER the locomotion pose so actions override and add to the
//! walk instead of replacing it. Beside the shared body core
//! ([`crate::animation_inputs`]) a body graph may read `walking sneak run
//! backward strafe airborne falling landing swim seated sleeping pitch hurt`
//! (`pitch` in degrees) and the `hurt` event (the flash rising).
//!
//! Every body on the roster advances every frame, drawn or not: a frame that
//! culls a remote, or the local body while the view is first person, still
//! runs its inputs, claims, events and montages — only the pose waits — so
//! nothing it did off-screen replays as a stale edge when it comes back.

use std::sync::Arc;

use petramond::player::rigs::{self, Presenter};
use petramond::player::RigId;
use petramond_world::animation::{Graph, LocalPose};
use rustc_hash::FxHashMap;

use crate::animation_inputs::{flag, BodyDriver, BodyMotion};
use crate::{AnimatorInputs, HeldItemFrame, PlayerRenderInstance};

type BodyInput = fn(&PlayerRenderInstance) -> f32;

const BODY: &[(&str, BodyInput)] = &[
    ("walking", |i| i.walk_weight),
    ("sneak", |i| i.sneak_weight),
    ("run", |i| i.locomotion.run),
    ("backward", |i| i.locomotion.backward),
    ("strafe", |i| i.locomotion.strafe),
    ("airborne", |i| i.locomotion.airborne),
    ("falling", |i| i.locomotion.falling),
    ("landing", |i| i.locomotion.landing),
    ("swim", |i| i.locomotion.swim.weight),
    ("seated", |i| flag(i.seated)),
    ("sleeping", |i| flag(i.sleeping)),
    ("pitch", |i| i.head_pitch.to_degrees()),
    ("hurt", |i| i.hurt),
];

impl BodyMotion for PlayerRenderInstance {
    fn hurt(&self) -> f32 {
        self.hurt
    }
}

/// The local player's body key; remote bodies key by their player id.
pub(crate) const LOCAL_BODY: u32 = u32::MAX;

pub(crate) struct BodyAnimator {
    driver: BodyDriver<PlayerRenderInstance>,
    /// The roster generation that last listed this body.
    listed: u64,
}

impl BodyAnimator {
    /// Advance one drawn frame, `dt` seconds after the last, over `ground`
    /// (the body's locomotion pose).
    pub fn update(
        &mut self,
        inst: &PlayerRenderInstance,
        frames: Option<&[HeldItemFrame; 2]>,
        inputs: AnimatorInputs<'_>,
        dt: f32,
        ground: &LocalPose,
    ) {
        self.drive(Some(inst), frames, inputs);
        self.driver.animator.update_over(dt, ground);
    }

    /// Advance one frame that does not draw this body, without posing it.
    /// `inst` is `None` when there is no body to read (the local body in
    /// first person).
    pub fn advance(
        &mut self,
        inst: Option<&PlayerRenderInstance>,
        frames: Option<&[HeldItemFrame; 2]>,
        inputs: AnimatorInputs<'_>,
        dt: f32,
    ) {
        self.drive(inst, frames, inputs);
        self.driver.animator.advance(dt);
    }

    fn drive(
        &mut self,
        inst: Option<&PlayerRenderInstance>,
        frames: Option<&[HeldItemFrame; 2]>,
        inputs: AnimatorInputs<'_>,
    ) {
        self.driver.begin(inst);
        if let Some(frames) = frames {
            self.driver.publish_hands(frames);
        }
        self.driver.claim(inputs);
    }

    pub fn pose(&self) -> &LocalPose {
        self.driver.animator.pose()
    }
}

/// Every body's animator, kept for as long as the body stays on the roster
/// (the remote players handed to the frame, and the local body).
pub(crate) struct BodyAnimators {
    /// The body rig and its graph; `None` without a registered body graph.
    graph: Option<(RigId, Arc<Graph>)>,
    bodies: FxHashMap<u32, BodyAnimator>,
    generation: u64,
}

impl BodyAnimators {
    /// Animators over `graph`; with no graph, none exist and bodies play
    /// their locomotion alone.
    pub fn new(graph: Option<(RigId, Arc<Graph>)>) -> Self {
        Self {
            graph,
            bodies: FxHashMap::default(),
            generation: 0,
        }
    }

    /// Animators over the catalog's body rig.
    pub fn shipped() -> Self {
        Self::new(
            rigs::presented(Presenter::Body)
                .and_then(|(id, rig)| Some((id, Arc::clone(rig.graph.as_ref()?)))),
        )
    }

    /// Keep the animators of the bodies on `roster`, and the local body's;
    /// drop every other.
    pub fn retain(&mut self, roster: impl IntoIterator<Item = u32>) {
        self.generation += 1;
        for key in roster {
            if let Some(body) = self.bodies.get_mut(&key) {
                body.listed = self.generation;
            }
        }
        let generation = self.generation;
        self.bodies
            .retain(|key, body| *key == LOCAL_BODY || body.listed == generation);
    }

    /// `key`'s animator, made on first use; `None` without a graph.
    pub fn body(&mut self, key: u32) -> Option<&mut BodyAnimator> {
        let (rig, graph) = self.graph.as_ref()?;
        Some(self.bodies.entry(key).or_insert_with(|| BodyAnimator {
            driver: BodyDriver::new(*rig, Arc::clone(graph), key ^ 0xB0D1, BODY),
            listed: 0,
        }))
    }

    pub fn clear(&mut self) {
        self.bodies.clear();
    }
}

#[cfg(test)]
mod tests;
