//! Retaliate: turn on whatever last damaged this mob — after a WARMUP.
//!
//! The damage pipeline records the attacking entity (player or mob) on the
//! struck instance; this node reads that memory and — while it is fresher than
//! `memory_ticks` and the attacker is still alive — answers it in three phases:
//!
//! 1. **Scan** (the warmup): the mob stands, tracks the attacker's bearing with
//!    its head and eases its body round, sweeping its head as if looking for
//!    what hit it. It reels instead of counter-striking on the very tick it
//!    was hit, and holds the goal/target/attack channels so nothing below it
//!    hunts or bites meanwhile.
//! 2. **Pursue**: once the warmup has elapsed and the attacker is in
//!    collision line of sight, chase its live position and publish it as the
//!    brain's target, so a co-resident `melee_attack` strikes back. Being hit
//!    is perception in its own right: even a mob whose ordinary senses can't
//!    find the attacker (a hearing hunter axed by a silent, sneaking player)
//!    knows exactly who bit it.
//! 3. **Escape**: an attacker it cannot see (an archer behind cover) cannot be
//!    charged blindly, so the mob runs the shared [`EscapeRoute`] instead,
//!    still looking toward where the shots come from, until sight returns.
//!
//! The warmup anchors on the FIRST hit deliberately: it counts on the node's
//! own clock, so an attacker re-hitting inside the window cannot keep resetting
//! it and fight a mob that never fights back. Re-hits only renew the memory. A
//! NEW attacker restarts the warmup.
//!
//! Whether a species fights back at all is row data — compose the node into its
//! brain or don't. Its canonical priority sits ABOVE the attack slot: a mob
//! under attack drops its current hunt and answers the attacker first, and only
//! from up there can the scan and escape phases hold a stale strike shut.

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

/// Default forget window: 10 s at 20 TPS — long enough to finish a fight,
/// short enough that a fled attacker is eventually forgiven.
const DEFAULT_MEMORY_TICKS: u32 = 200;
/// Default boil-over delay: 1 s at 20 TPS between the first hit and the mob
/// turning on its attacker.
const DEFAULT_WARMUP_TICKS: u32 = 20;
/// Where the mob looks FROM, as a fraction of its head height above the feet:
/// the eye line of a quadruped/biped rig sits a little under the head top.
const EYE_HEIGHT_FRACTION: f32 = 0.8;
/// Scan sweep: the head oscillates at this angular rate (radians per tick,
/// ~one full sweep per second at 20 TPS) ...
const SCAN_SWEEP_RATE: f32 = 0.32;
/// ... with this amplitude (radians) either side of the attacker's bearing.
const SCAN_SWEEP_AMPLITUDE: f32 = 0.55;
/// How far (radians) the head alone tracks the attacker off the body axis
/// before the body is expected to turn.
const HEAD_TRACK_LIMIT: f32 = 0.55;
/// Hard yaw limit (radians) of head over body, sweep included — past this a
/// rig's neck breaks visually.
const HEAD_YAW_LIMIT: f32 = 0.9;
/// Hard pitch limit (radians) up/down toward the attacker.
const HEAD_PITCH_LIMIT: f32 = 0.5;
/// Share of the head sweep the BODY follows while scanning, so the whole
/// mob visibly casts about rather than only its head.
const BODY_SWEEP_SHARE: f32 = 0.5;
/// Horizontal distance (blocks) under which the attacker is directly above or
/// below and the pitch is clamped instead of computed from a degenerate atan.
const FLAT_EPSILON: f32 = 0.001;

/// `retaliate` params as written in a `mobs.json` brain row (plus the shared
/// escape `radius`).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RetaliateParams {
    /// Ticks since the LAST hit before the grudge is forgotten.
    #[serde(default = "default_memory")]
    memory_ticks: u32,
    /// Ticks after the FIRST hit before the mob turns on the attacker.
    #[serde(default = "default_warmup")]
    warmup_ticks: u32,
    /// Escape leg reach (blocks) when the attacker is out of sight — see
    /// [`EscapeParams`].
    #[serde(default)]
    radius: Option<u32>,
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
    /// The attacker the current grudge is against.
    grudge: Option<EntityRef>,
    /// Node ticks since this grudge began (its FIRST hit) — the warmup clock.
    grudge_ticks: u32,
    escape: EscapeRoute,
}

impl RetaliateAi {
    /// Channels held while scanning: stand (no destination from below), no
    /// lock, no strike — the mob is still working out what hit it.
    const SCAN_HOLDS: ChannelClaims = ChannelClaims::of(&[
        DecisionChannel::Goal,
        DecisionChannel::Target,
        DecisionChannel::Attack,
    ]);

    pub(super) fn new(memory_ticks: u32, warmup_ticks: u32, escape: EscapeRoute) -> Self {
        RetaliateAi {
            memory_ticks: memory_ticks.max(1),
            warmup_ticks,
            grudge: None,
            grudge_ticks: 0,
            escape,
        }
    }

    /// Build from a brain row's `params` — the `retaliate` node factory core.
    pub(super) fn from_params(params: &serde_json::Value) -> Result<Self, String> {
        let p: RetaliateParams =
            serde_json::from_value(params.clone()).map_err(|e| e.to_string())?;
        if p.memory_ticks == 0 {
            return Err("memory_ticks must be >= 1".into());
        }
        if p.warmup_ticks >= p.memory_ticks {
            // A single un-renewed hit would age out of memory before the
            // warmup elapsed — the node could never fire.
            return Err("warmup_ticks must be < memory_ticks".into());
        }
        let escape = EscapeRoute::from_params(EscapeParams::with_radius(p.radius))?;
        Ok(RetaliateAi::new(p.memory_ticks, p.warmup_ticks, escape))
    }
}

impl AiBehavior for RetaliateAi {
    fn tick(&mut self, ctx: &mut AiCtx) -> BehaviorOutput {
        let fresh = ctx
            .attacker
            .filter(|&(_, ticks_ago)| ticks_ago <= self.memory_ticks);
        let Some((who, _)) = fresh else {
            self.grudge = None;
            self.grudge_ticks = 0;
            self.escape.reset();
            return BehaviorOutput::default();
        };
        // A new attacker starts a new grudge (and a new warmup); re-hits from
        // the same one only keep the MEMORY fresh — the warmup clock is this
        // node's own and never rewinds.
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
        // Same convention as the instance: the model faces -Z at yaw 0.
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
