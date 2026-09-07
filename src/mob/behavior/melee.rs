//! Melee: strike the brain's current target when in reach.
//!
//! The node strikes ONLY the entity the brain's winning perception/chase node
//! locked last tick (`AiCtx::target`) — a player or another mob. No lock means
//! no strike: attack nodes execute on the perception layer's decision, they
//! never perceive on their own. That is what lets a blind hearing hunter share
//! a room with a silent player — nothing locked, nothing bitten — while a
//! sighted chaser (`chase_player`) publishes its lock long before melee range,
//! so classic hostiles fight exactly as before.
//!
//! A strike lands when the target is within `reach` of the mob's body (centre
//! distance minus the bodies' widths), the mob is roughly facing it, the strike
//! line is not blocked by world collision, and the per-node cooldown has
//! elapsed — then the node emits an [`AttackIntent`] naming the target. It
//! never touches the target itself: the intent flows instance → manager →
//! `Game`, where the damage runs through the target's own pipeline
//! (`player_damage_pre` for players — a cancel drops the knockback too — or
//! the mob damage pipeline for mobs).
//!
//! Cooldown state lives here (deterministic tick counting); a mob under knockback
//! stagger still counts its cooldown down like any other tick.

use std::f32::consts::{FRAC_PI_2, PI, TAU};

use serde::Deserialize;

use petramond_math::math::Vec3;

use super::super::brain::{AiBehavior, AiCtx, AttackIntent, BehaviorOutput};
use super::super::{def, EntityRef};
use super::los;

/// Widest angle (radians) the player may sit off the mob's facing for a strike to
/// land — 90° either side ("rough facing"): a chasing mob turns toward its travel,
/// so this only stops hits from a mob walking squarely away.
const MAX_FACING_OFF: f32 = FRAC_PI_2;

/// `melee_attack` params as written in a `mobs.json` brain row.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MeleeParams {
    /// Strike range in blocks, measured from the mob's body edge.
    reach: f64,
    /// Damage per strike, in half-heart points.
    damage: f64,
    /// Horizontal knockback speed (m/s) imparted on the player.
    knockback: f64,
    /// Game ticks between strikes.
    cooldown_ticks: u32,
    /// Ticks between announcing a strike (its clip starts, the target is
    /// latched) and landing it; 0 = land on the announcing tick. Must be
    /// shorter than the cooldown so a strike lands before the next may start.
    #[serde(default)]
    windup_ticks: u32,
    /// Model clip started when a strike is announced (the wind-up read).
    #[serde(default)]
    animation: Option<String>,
}

pub struct MeleeAttackAi {
    reach: f32,
    damage: f32,
    knockback: f32,
    cooldown_ticks: u32,
    /// Ticks until the next strike may land.
    cooldown: u32,
    windup_ticks: u32,
    pending: Option<(EntityRef, u32)>,
    animation: Option<String>,
}

impl MeleeAttackAi {
    pub fn new(reach: f32, damage: f32, knockback: f32, cooldown_ticks: u32) -> Self {
        MeleeAttackAi {
            reach,
            damage,
            knockback,
            cooldown_ticks: cooldown_ticks.max(1),
            cooldown: 0,
            windup_ticks: 0,
            pending: None,
            animation: None,
        }
    }

    /// Build from a brain row's `params` — the `melee_attack` node factory core.
    pub(super) fn from_params(params: &serde_json::Value) -> Result<Self, String> {
        let p: MeleeParams = serde_json::from_value(params.clone()).map_err(|e| e.to_string())?;
        // `partial_cmp` (not `<=`) so a NaN reach is rejected too.
        if p.reach.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater) {
            return Err("reach must be > 0".into());
        }
        if p.cooldown_ticks == 0 {
            return Err("cooldown_ticks must be >= 1".into());
        }
        if p.windup_ticks >= p.cooldown_ticks {
            return Err("windup_ticks must be less than cooldown_ticks".into());
        }
        if p.animation
            .as_deref()
            .is_some_and(|name| !crate::mob::anim::valid_clip_name(name))
        {
            return Err(format!(
                "animation must be a nonempty name of at most {} bytes",
                mod_api::MAX_MOB_ANIM_NAME_BYTES
            ));
        }
        let mut ai = MeleeAttackAi::new(
            p.reach as f32,
            p.damage as f32,
            p.knockback as f32,
            p.cooldown_ticks,
        );
        ai.windup_ticks = p.windup_ticks;
        ai.animation = p.animation;
        Ok(ai)
    }
}

impl AiBehavior for MeleeAttackAi {
    fn tick(&mut self, ctx: &mut AiCtx) -> BehaviorOutput {
        self.cooldown = self.cooldown.saturating_sub(1);
        let impact = if let Some((target, remaining)) = self.pending {
            if ctx.target != Some(target) {
                self.pending = None;
                return BehaviorOutput::default();
            }
            if remaining > 1 {
                self.pending = Some((target, remaining - 1));
                return BehaviorOutput::default();
            }
            self.pending = None;
            true
        } else {
            false
        };
        if self.cooldown > 0 && !impact {
            return BehaviorOutput::default();
        }
        // Resolve the strike target: the brain's current lock ONLY — no lock,
        // no strike (perception decides, this node executes). A locked target
        // that has vanished or died strikes nothing this tick. `pad` widens
        // the reach by a mob target's own half-width; players keep the
        // historical body-centre rule.
        let (target, target_pos, pad) = match ctx.target {
            None => return BehaviorOutput::default(),
            Some(EntityRef::Player(pid)) => match ctx.players.iter().find(|a| a.id == pid) {
                Some(a) => (EntityRef::Player(pid), a.pos, 0.0),
                None => return BehaviorOutput::default(),
            },
            Some(EntityRef::Mob(id)) => {
                if id == ctx.mob_id {
                    return BehaviorOutput::default();
                }
                match ctx.mobs.iter().find(|m| m.id == id && m.active) {
                    Some(m) => {
                        let size = def(m.kind).size;
                        // `AiMob::pos` is feet; strike geometry wants the body centre.
                        let centre = m.pos + Vec3::new(0.0, size.height * 0.5, 0.0);
                        (EntityRef::Mob(id), centre, size.half_width)
                    }
                    None => return BehaviorOutput::default(),
                }
            }
        };
        // Body distance: from the mob's body centre to the target's, less both
        // bodies' horizontal extent, so reach is measured edge-to-edge and a
        // wide mob (or target) doesn't need to overlap to connect.
        let centre = ctx.pos + Vec3::new(0.0, ctx.head_height * 0.5, 0.0);
        let gap = (target_pos - centre).length() - ctx.half_width - pad;
        if gap > self.reach
            || !facing_target(ctx.yaw, ctx.pos, target_pos)
            || !los::line_clear(ctx.world, centre, target_pos)
        {
            return BehaviorOutput::default();
        }
        if !impact {
            self.cooldown = self.cooldown_ticks;
            if self.windup_ticks > 0 {
                self.pending = Some((target, self.windup_ticks));
                return BehaviorOutput {
                    animation: self.animation.clone(),
                    ..Default::default()
                };
            }
        }
        BehaviorOutput {
            animation: if impact { None } else { self.animation.clone() },
            attack: Some(AttackIntent {
                target,
                damage: self.damage,
                knockback: self.knockback,
            }),
            ..Default::default()
        }
    }
}

/// Rough facing check: the target sits within [`MAX_FACING_OFF`] of the mob's body
/// yaw. A target directly on top of the mob (no horizontal offset) always counts.
fn facing_target(yaw: f32, pos: Vec3, player: Vec3) -> bool {
    let (dx, dz) = (player.x - pos.x, player.z - pos.z);
    if dx * dx + dz * dz <= 1e-6 {
        return true;
    }
    // Same convention as the instance: the model faces -Z at yaw 0.
    let target = (-dx).atan2(-dz);
    wrap_angle(target - yaw).abs() <= MAX_FACING_OFF
}

/// Wrap an angle into `[-PI, PI]`.
fn wrap_angle(a: f32) -> f32 {
    (a + PI).rem_euclid(TAU) - PI
}

#[cfg(test)]
mod tests;
