use std::f32::consts::{FRAC_PI_2, PI, TAU};

use serde::Deserialize;

use petramond_math::math::Vec3;

use super::super::brain::{AiBehavior, AiCtx, AttackIntent, BehaviorOutput};
use super::super::{def, EntityRef};
use super::los;

const MAX_FACING_OFF: f32 = FRAC_PI_2;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MeleeParams {
    reach: f64,
    damage: f64,
    knockback: f64,
    cooldown_ticks: u32,
    #[serde(default)]
    windup_ticks: u32,
    #[serde(default)]
    animation: Option<String>,
}

pub struct MeleeAttackAi {
    reach: f32,
    damage: f32,
    knockback: f32,
    cooldown_ticks: u32,
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

    pub(super) fn from_params(params: &serde_json::Value) -> Result<Self, String> {
        let p: MeleeParams = serde_json::from_value(params.clone()).map_err(|e| e.to_string())?;
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
                match ctx.live_mob(id) {
                    Some(m) => {
                        let size = def(m.kind).size;
                        let centre = m.pos + Vec3::new(0.0, size.height * 0.5, 0.0);
                        (EntityRef::Mob(id), centre, size.half_width)
                    }
                    None => return BehaviorOutput::default(),
                }
            }
        };
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

fn facing_target(
    yaw: f32,
    pos: petramond_math::world_pos::WorldPos,
    player: petramond_math::world_pos::WorldPos,
) -> bool {
    let d = player - pos;
    let (dx, dz) = (d.x, d.z);
    if dx * dx + dz * dz <= 1e-6 {
        return true;
    }
    let target = (-dx).atan2(-dz);
    wrap_angle(target - yaw).abs() <= MAX_FACING_OFF
}

fn wrap_angle(a: f32) -> f32 {
    (a + PI).rem_euclid(TAU) - PI
}

#[cfg(test)]
mod tests;
