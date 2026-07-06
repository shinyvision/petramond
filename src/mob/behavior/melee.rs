//! Melee: strike the player when in reach.
//!
//! When the player is within `reach` of the mob's body (centre distance minus the
//! body half-width), the mob is roughly facing them, the strike line is not blocked
//! by world collision, and the per-node cooldown has elapsed, the node emits an
//! [`AttackIntent`]. It never touches the player itself:
//! the intent flows instance → manager → `Game`, where the damage runs through the
//! `player_damage_pre` pipeline (a cancel drops the knockback too). Targeting stays
//! deliberately simple: range, rough facing, and a short collision line-of-sight ray.
//!
//! Cooldown state lives here (deterministic tick counting); a mob under knockback
//! stagger still counts its cooldown down like any other tick.

use std::f32::consts::{FRAC_PI_2, PI, TAU};

use serde::Deserialize;

use crate::mathh::Vec3;

use super::super::brain::{AiBehavior, AiCtx, AttackIntent, BehaviorOutput};
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
}

pub struct MeleeAttackAi {
    reach: f32,
    damage: f32,
    knockback: f32,
    cooldown_ticks: u32,
    /// Ticks until the next strike may land.
    cooldown: u32,
}

impl MeleeAttackAi {
    pub fn new(reach: f32, damage: f32, knockback: f32, cooldown_ticks: u32) -> Self {
        MeleeAttackAi {
            reach,
            damage,
            knockback,
            cooldown_ticks: cooldown_ticks.max(1),
            cooldown: 0,
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
        Ok(MeleeAttackAi::new(
            p.reach as f32,
            p.damage as f32,
            p.knockback as f32,
            p.cooldown_ticks,
        ))
    }
}

impl AiBehavior for MeleeAttackAi {
    fn tick(&mut self, ctx: &mut AiCtx) -> BehaviorOutput {
        self.cooldown = self.cooldown.saturating_sub(1);
        if self.cooldown > 0 {
            return BehaviorOutput::default();
        }
        // Body distance: from the mob's body centre to the player's, less the mob's
        // horizontal half-width, so reach is measured from the body edge and a wide
        // mob doesn't need to overlap the player to connect.
        let centre = ctx.pos + Vec3::new(0.0, ctx.head_height * 0.5, 0.0);
        let gap = (ctx.player_pos - centre).length() - ctx.half_width;
        if gap > self.reach
            || !facing_player(ctx.yaw, ctx.pos, ctx.player_pos)
            || !los::line_clear(ctx.world, centre, ctx.player_pos)
        {
            return BehaviorOutput::default();
        }
        self.cooldown = self.cooldown_ticks;
        BehaviorOutput {
            attack: Some(AttackIntent {
                damage: self.damage,
                knockback: self.knockback,
            }),
            ..Default::default()
        }
    }
}

/// Rough facing check: the player sits within [`MAX_FACING_OFF`] of the mob's body
/// yaw. A player directly on top of the mob (no horizontal offset) always counts.
fn facing_player(yaw: f32, pos: Vec3, player: Vec3) -> bool {
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
mod tests {
    use super::*;
    use crate::block::Block;
    use crate::chunk::ChunkPos;
    use crate::mob::MobRng;
    use crate::world::World;

    fn ctx<'a>(
        world: &'a World,
        rng: &'a mut MobRng,
        pos: Vec3,
        yaw: f32,
        player: Vec3,
    ) -> AiCtx<'a> {
        AiCtx {
            mob_id: 1,
            pos,
            cell: crate::mathh::voxel_at(pos),
            yaw,
            head_height: 1.3,
            half_width: 0.45,
            world,
            player_pos: player,
            nav_idle: true,
            in_water: false,
            head: 2,
            idle_anims: &[],
            mob_index: 0,
            mobs: &[],
            rng,
        }
    }

    /// A player one block in front of the mob's face (-Z), inside a 1.5 reach.
    fn in_reach() -> (Vec3, f32, Vec3) {
        (Vec3::new(8.5, 64.0, 8.5), 0.0, Vec3::new(8.5, 64.9, 7.2))
    }

    #[test]
    fn strikes_in_reach_then_is_gated_by_the_cooldown() {
        let world = World::new(0, 1);
        let mut rng = MobRng::new(1);
        let mut ai = MeleeAttackAi::new(1.5, 2.0, 5.0, 10);
        let (pos, yaw, player) = in_reach();

        let intent = ai
            .tick(&mut ctx(&world, &mut rng, pos, yaw, player))
            .attack
            .expect("in-reach facing strike lands");
        assert_eq!(
            intent,
            AttackIntent {
                damage: 2.0,
                knockback: 5.0
            }
        );

        // The next strike only lands once the cooldown has fully elapsed.
        for i in 0..9 {
            assert!(
                ai.tick(&mut ctx(&world, &mut rng, pos, yaw, player))
                    .attack
                    .is_none(),
                "tick {i} is inside the cooldown"
            );
        }
        assert!(
            ai.tick(&mut ctx(&world, &mut rng, pos, yaw, player))
                .attack
                .is_some(),
            "the cooldown elapsed, the next strike lands"
        );
    }

    #[test]
    fn out_of_reach_or_facing_away_lands_nothing() {
        let world = World::new(0, 1);
        let mut rng = MobRng::new(1);
        let mut ai = MeleeAttackAi::new(1.5, 2.0, 5.0, 10);
        let pos = Vec3::new(8.5, 64.0, 8.5);

        // 5 blocks away: out of reach.
        let far = Vec3::new(8.5, 64.9, 3.5);
        assert!(ai
            .tick(&mut ctx(&world, &mut rng, pos, 0.0, far))
            .attack
            .is_none());

        // In reach at -Z but the mob faces +Z (yaw PI): squarely behind it.
        let behind = Vec3::new(8.5, 64.9, 7.2);
        assert!(
            ai.tick(&mut ctx(&world, &mut rng, pos, PI, behind))
                .attack
                .is_none(),
            "a player behind the mob is not struck"
        );
        // The cooldown was never armed by those misses.
        assert!(
            ai.tick(&mut ctx(&world, &mut rng, pos, 0.0, behind))
                .attack
                .is_some(),
            "a miss does not arm the cooldown"
        );
    }

    #[test]
    fn block_between_mob_and_player_prevents_strike_without_cooldown() {
        let mut world = World::new(0, 1);
        world.insert_empty_column_for_test(ChunkPos::new(0, 0));
        assert!(world.set_block_world(8, 64, 7, Block::Stone));
        let mut rng = MobRng::new(1);
        let mut ai = MeleeAttackAi::new(3.0, 2.0, 5.0, 10);
        let pos = Vec3::new(8.5, 64.0, 8.5);
        let player = Vec3::new(8.5, 64.9, 5.8);

        assert!(
            ai.tick(&mut ctx(&world, &mut rng, pos, 0.0, player))
                .attack
                .is_none(),
            "a colliding block between mob and player blocks melee"
        );

        assert!(world.set_block_world(8, 64, 7, Block::Air));
        assert!(
            ai.tick(&mut ctx(&world, &mut rng, pos, 0.0, player))
                .attack
                .is_some(),
            "the blocked attempt did not arm cooldown"
        );
    }

    #[test]
    fn params_are_validated_at_load() {
        let ok = serde_json::json!({"reach": 1.5, "damage": 2.0, "knockback": 5.0, "cooldown_ticks": 20});
        assert!(MeleeAttackAi::from_params(&ok).is_ok());
        assert!(
            MeleeAttackAi::from_params(&serde_json::json!({"reach": 1.5})).is_err(),
            "missing fields are refused"
        );
        assert!(
            MeleeAttackAi::from_params(
                &serde_json::json!({"reach": 0.0, "damage": 2.0, "knockback": 5.0, "cooldown_ticks": 20})
            )
            .is_err(),
            "zero reach is refused"
        );
        assert!(
            MeleeAttackAi::from_params(
                &serde_json::json!({"reach": 1.5, "damage": 2.0, "knockback": 5.0, "cooldown_ticks": 0})
            )
            .is_err(),
            "a zero cooldown is refused"
        );
    }
}
