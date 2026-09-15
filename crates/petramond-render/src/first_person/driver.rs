//! Feeds the first-person animator: the body's motion and the two hands'
//! frames become the graph's params and events, by name. Beside the shared
//! body core ([`crate::animation_inputs`]) it publishes `speed forward
//! strafe vertical grounded sneaking sprinting swimming climbing pitch
//! yaw_rate pitch_rate stride stride_weight hurt target` (`target`: 0 nothing,
//! 1 a block, 2 a creature), `impact` (the fall speed of the last landing),
//! and the events `hurt jump land`. What a clip's markers fire is the
//! graph's own `markers` table.

use std::sync::Arc;

use petramond::player::RigId;
use petramond_world::animation::{Animator, EventId, Graph, ParamId};

use crate::animation_inputs::{flag, BodyDriver, BodyMotion};
use crate::views::LocalMotion;
use crate::{AnimatorInputs, HeldItemFrame};

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

/// Upward speed (blocks per second) that reads as a jump rather than a step.
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
    /// The last landing's fall speed, written every frame so a released
    /// claim on `impact` uncovers it.
    impact_value: f32,
    /// Grounded last frame (`None` before the first), and the fastest fall
    /// since leaving the ground.
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

    /// One frame, `dt` seconds after the last.
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
