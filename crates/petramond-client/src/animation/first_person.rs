use std::sync::Arc;

use glam::Mat4;
use petramond::player::rigs::{self, Presenter, Rig};
use petramond::player::RigId;
use petramond_anim::{Animator, EventId, Graph, ParamId};
use petramond_render::HeldItemFrame;

use super::claims::AnimatorInputs;
use super::inputs::{flag, BodyDriver, BodyMotion};
use super::motion::LocalMotion;

type BodyInput = fn(&LocalMotion) -> f32;

const BODY: &[(&str, BodyInput)] = &[
    ("speed", |m| m.speed),
    ("forward", |m| m.forward),
    ("strafe", |m| m.strafe),
    ("vertical", |m| m.vertical),
    ("grounded", |m| flag(m.grounded)),
    ("sneaking", |m| flag(m.sneaking)),
    ("sprinting", |m| flag(m.sprinting)),
    ("swimming", |m| flag(m.swimming)),
    ("climbing", |m| flag(m.climbing)),
    ("pitch", |m| m.pitch),
    ("yaw_rate", |m| m.yaw_rate),
    ("pitch_rate", |m| m.pitch_rate),
    ("stride", |m| m.stride),
    ("stride_weight", |m| m.stride_weight),
    ("hurt", |m| m.hurt),
    ("target", |m| m.target as u8 as f32),
];

const JUMP_SPEED: f32 = 3.0;

impl BodyMotion for LocalMotion {
    fn hurt(&self) -> f32 {
        self.hurt
    }
}

pub(crate) struct Driver {
    body: BodyDriver<LocalMotion>,
    jump: Option<EventId>,
    land: Option<EventId>,
    impact: Option<ParamId>,
    impact_value: f32,
    grounded: Option<bool>,
    fall: f32,
}

impl Driver {
    pub fn new(rig: RigId, graph: Arc<Graph>) -> Self {
        Self {
            jump: graph.event("jump"),
            land: graph.event("land"),
            impact: graph.param("impact"),
            impact_value: 0.0,
            body: BodyDriver::new(rig, graph, 0x5EED, BODY),
            grounded: None,
            fall: 0.0,
        }
    }

    pub fn reset(&mut self) {
        self.body.reset();
        self.impact_value = 0.0;
        self.grounded = None;
        self.fall = 0.0;
    }

    pub fn animator(&self) -> &Animator {
        &self.body.animator
    }

    pub fn update(
        &mut self,
        frames: &[HeldItemFrame; 2],
        motion: &LocalMotion,
        inputs: AnimatorInputs<'_>,
        dt: f32,
    ) {
        self.body.begin(Some(motion));
        let animator = &mut self.body.animator;
        match self.grounded {
            Some(true) if !motion.grounded => {
                self.fall = 0.0;
                if motion.vertical > JUMP_SPEED {
                    if let Some(event) = self.jump {
                        animator.fire(event);
                    }
                }
            }
            Some(false) if motion.grounded => {
                self.impact_value = -self.fall;
                if let Some(event) = self.land {
                    animator.fire(event);
                }
            }
            _ => {}
        }
        if let Some(impact) = self.impact {
            animator.set(impact, self.impact_value);
        }
        if !motion.grounded {
            self.fall = self.fall.min(motion.vertical);
        }
        self.grounded = Some(motion.grounded);
        self.body.publish_hands(frames);
        self.body.claim(inputs);
        self.body.animator.update(dt);
    }
}

pub struct FirstPersonAnimator {
    row: &'static Rig,
    bones: Vec<Mat4>,
    driver: Driver,
}

impl FirstPersonAnimator {
    pub fn shipped() -> Option<Self> {
        let (id, row) = rigs::presented(Presenter::Viewmodel)?;
        let graph = row.graph.as_ref()?;
        if row.model.bones().is_empty() {
            return None;
        }
        Some(Self {
            row,
            bones: row.model.resolve_local(&[], &[]),
            driver: Driver::new(id, Arc::clone(graph)),
        })
    }

    pub fn reset(&mut self) {
        self.driver.reset();
        self.row.model.resolve_local_into(&[], &[], &mut self.bones);
    }

    pub fn advance(
        &mut self,
        frames: &[HeldItemFrame; 2],
        motion: &LocalMotion,
        inputs: AnimatorInputs<'_>,
        dt: f32,
    ) {
        self.driver.update(frames, motion, inputs, dt);
        self.driver
            .animator()
            .pose()
            .resolve_into(&self.row.model, &mut self.bones);
    }

    pub fn bones(&self) -> &[Mat4] {
        &self.bones
    }
}

#[cfg(test)]
mod tests;
