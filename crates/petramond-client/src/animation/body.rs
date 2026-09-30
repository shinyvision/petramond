use std::sync::Arc;

use petramond::player::rigs::{self, Presenter};
use petramond::player::RigId;
use petramond_anim::{Graph, LocalPose};
use petramond_render::HeldItemFrame;
use rustc_hash::FxHashMap;

use super::claims::AnimatorInputs;
use super::inputs::{flag, BodyDriver, BodyMotion};
use super::motion::BodyState;

type BodyInput = fn(&BodyState) -> f32;

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

impl BodyMotion for BodyState {
    fn hurt(&self) -> f32 {
        self.hurt
    }
}

pub const LOCAL_BODY: u32 = u32::MAX;

pub(crate) struct BodyAnimator {
    driver: BodyDriver<BodyState>,
    listed: u64,
}

impl BodyAnimator {
    pub fn update(
        &mut self,
        state: &BodyState,
        frames: Option<&[HeldItemFrame; 2]>,
        inputs: AnimatorInputs<'_>,
        dt: f32,
        ground: &LocalPose,
    ) {
        self.drive(Some(state), frames, inputs);
        self.driver.animator.update_over(dt, ground);
    }

    pub fn advance(
        &mut self,
        state: Option<&BodyState>,
        frames: Option<&[HeldItemFrame; 2]>,
        inputs: AnimatorInputs<'_>,
        dt: f32,
    ) {
        self.drive(state, frames, inputs);
        self.driver.animator.advance(dt);
    }

    fn drive(
        &mut self,
        state: Option<&BodyState>,
        frames: Option<&[HeldItemFrame; 2]>,
        inputs: AnimatorInputs<'_>,
    ) {
        self.driver.begin(state);
        if let Some(frames) = frames {
            self.driver.publish_hands(frames);
        }
        self.driver.claim(inputs);
    }

    pub(super) fn present_hands(
        &self,
        held: [petramond_render::HeldItemView; 2],
    ) -> [petramond_render::HeldItemView; 2] {
        super::inputs::present_hands(&self.driver.animator, held)
    }

    pub fn pose(&self) -> &LocalPose {
        self.driver.animator.pose()
    }
}

pub(crate) struct BodyAnimators {
    graph: Option<(RigId, Arc<Graph>)>,
    bodies: FxHashMap<u32, BodyAnimator>,
    generation: u64,
}

impl BodyAnimators {
    pub fn new(graph: Option<(RigId, Arc<Graph>)>) -> Self {
        Self {
            graph,
            bodies: FxHashMap::default(),
            generation: 0,
        }
    }

    pub fn shipped() -> Self {
        Self::new(
            rigs::presented(Presenter::Body)
                .and_then(|(id, rig)| Some((id, Arc::clone(rig.graph.as_ref()?)))),
        )
    }

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
