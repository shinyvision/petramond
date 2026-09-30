//! Retaliate: fight back at whatever last hurt this mob, after a WARMUP.
//!
//! Damage pipeline stashes the attacker on the struck instance. Node reads that while it's fresh
//! (younger than `memory_ticks`) and the attacker's alive.
//!
//! Three phases:
//! 1. Scan (the warmup) - stand there, track the attacker with your head, ease your body round like
//!    you're looking for what hit you. Holds goal/target/attack so nothing else hunts or bites
//!    meanwhile.
//! 2. Pursue - warmup's over, attacker in collision LOS, chase its live position, publish as target
//!    for `melee_attack`. Getting hit is its own kind of perception - even a hearing-only mob knows
//!    who bit it, sneaking player or not.
//! 3. Escape - can't see the attacker (archer behind cover)? Run the shared [`EscapeRoute`], still
//!    looking toward the shots, until LOS comes back.
//!
//! Warmup anchors on the first hit on purpose, off the node's own clock. Otherwise a re-hitting
//! attacker keeps resetting the timer and never eats a counter. Re-hits just renew the memory, a
//! new attacker restarts warmup.
//!
//! Whether a species retaliates is just row data; wire the node in or don't. Priority sits above
//! the attack slot: a mob under attack drops its hunt and deals with the attacker first, and only
//! from up there can scan and escape hold a stale strike shut.

use std::f32::consts::{PI, TAU};

use serde::Deserialize;

use super::super::brain::{
    AiBehavior, AiCtx, BehaviorOutput, ChannelClaims, DecisionChannel, HeadLook,
};
use super::super::EntityRef;
use super::chase::goal_cell_near;
use super::escape::{EscapeParams, EscapeRoute};
use super::los;
use petramond_math::math::Vec3;

const DEFAULT_MEMORY_TICKS: u32 = 200;
const DEFAULT_WARMUP_TICKS: u32 = 20;
const EYE_HEIGHT_FRACTION: f32 = 0.8;
const SCAN_SWEEP_RATE: f32 = 0.32;
const SCAN_SWEEP_AMPLITUDE: f32 = 0.55;
const HEAD_TRACK_LIMIT: f32 = 0.55;
const HEAD_YAW_LIMIT: f32 = 0.9;
const HEAD_PITCH_LIMIT: f32 = 0.5;
const BODY_SWEEP_SHARE: f32 = 0.5;
const FLAT_EPSILON: f32 = 0.001;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RetaliateParams {
    #[serde(default = "default_memory")]
    memory_ticks: u32,
    #[serde(default = "default_warmup")]
    warmup_ticks: u32,
    #[serde(default)]
    radius: Option<u32>,
    #[serde(default)]
    ignore_same_species: bool,
}

fn default_memory() -> u32 {
    DEFAULT_MEMORY_TICKS
}

fn default_warmup() -> u32 {
    DEFAULT_WARMUP_TICKS
}

pub struct RetaliateAi {
    memory_ticks: u32,
    warmup_ticks: u32,
    ignore_same_species: bool,
    grudge: Option<EntityRef>,
    grudge_ticks: u32,
    escape: EscapeRoute,
}

impl RetaliateAi {
    const SCAN_HOLDS: ChannelClaims = ChannelClaims::of(&[
        DecisionChannel::Goal,
        DecisionChannel::Target,
        DecisionChannel::Attack,
    ]);

    pub(super) fn new(memory_ticks: u32, warmup_ticks: u32, escape: EscapeRoute) -> Self {
        RetaliateAi {
            memory_ticks: memory_ticks.max(1),
            warmup_ticks,
            ignore_same_species: false,
            grudge: None,
            grudge_ticks: 0,
            escape,
        }
    }

    pub(super) fn from_params(params: &serde_json::Value) -> Result<Self, String> {
        let p: RetaliateParams =
            serde_json::from_value(params.clone()).map_err(|e| e.to_string())?;
        if p.memory_ticks == 0 {
            return Err("memory_ticks must be >= 1".into());
        }
        if p.warmup_ticks >= p.memory_ticks {
            return Err("warmup_ticks must be < memory_ticks".into());
        }
        let escape = EscapeRoute::from_params(EscapeParams::with_radius(p.radius))?;
        let mut ai = RetaliateAi::new(p.memory_ticks, p.warmup_ticks, escape);
        ai.ignore_same_species = p.ignore_same_species;
        Ok(ai)
    }
}

impl AiBehavior for RetaliateAi {
    fn tick(&mut self, ctx: &mut AiCtx) -> BehaviorOutput {
        let fresh = ctx
            .attacker
            .filter(|&(_, ticks_ago)| ticks_ago <= self.memory_ticks)
            .filter(|&(who, _)| {
                !self.ignore_same_species
                    || match who {
                        EntityRef::Player(_) => true,
                        EntityRef::Mob(id) => ctx
                            .live_mob(ctx.mob_id)
                            .zip(ctx.live_mob(id))
                            .is_none_or(|(me, attacker)| me.kind != attacker.kind),
                    }
            });
        let Some((who, _)) = fresh else {
            self.grudge = None;
            self.grudge_ticks = 0;
            self.escape.reset();
            return BehaviorOutput::default();
        };
        if self.grudge != Some(who) {
            self.grudge = Some(who);
            self.grudge_ticks = 0;
            self.escape.reset();
        }
        let Some(pos) = ctx.entity_pos(who) else {
            self.grudge = None;
            self.escape.reset();
            return BehaviorOutput::default();
        };
        self.grudge_ticks = self.grudge_ticks.saturating_add(1);
        let eye = ctx.pos + Vec3::new(0.0, ctx.head_height * EYE_HEIGHT_FRACTION, 0.0);
        let to = pos - eye;
        let attacker_yaw = (-to.x).atan2(-to.z);
        let bearing = (attacker_yaw - ctx.yaw + PI).rem_euclid(TAU) - PI;
        let scanning = self.grudge_ticks <= self.warmup_ticks;
        let sweep = if scanning {
            (self.grudge_ticks as f32 * SCAN_SWEEP_RATE).sin() * SCAN_SWEEP_AMPLITUDE
        } else {
            0.0
        };
        let look = Some(HeadLook {
            yaw: (bearing.clamp(-HEAD_TRACK_LIMIT, HEAD_TRACK_LIMIT) + sweep)
                .clamp(-HEAD_YAW_LIMIT, HEAD_YAW_LIMIT),
            pitch: to
                .y
                .atan2(to.x.hypot(to.z).max(FLAT_EPSILON))
                .clamp(-HEAD_PITCH_LIMIT, HEAD_PITCH_LIMIT),
        });
        if scanning {
            return BehaviorOutput {
                head_look: look,
                facing: Some(attacker_yaw + sweep * BODY_SWEEP_SHARE),
                claims: Self::SCAN_HOLDS,
                ..Default::default()
            };
        }
        if !los::line_clear(ctx.world, eye, pos) {
            return BehaviorOutput {
                goal: self.escape.goal(ctx, pos),
                head_look: look,
                claims: EscapeRoute::HOLDS,
                ..Default::default()
            };
        }
        self.escape.reset();
        BehaviorOutput {
            goal: goal_cell_near(ctx, pos),
            head_look: look,
            target: Some(who),
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests;
