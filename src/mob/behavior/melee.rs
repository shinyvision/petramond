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

use crate::mathh::{IVec3, Vec3};
use crate::world::World;

use super::super::brain::{AiBehavior, AiCtx, AttackIntent, BehaviorOutput};

/// Widest angle (radians) the player may sit off the mob's facing for a strike to
/// land — 90° either side ("rough facing"): a chasing mob turns toward its travel,
/// so this only stops hits from a mob walking squarely away.
const MAX_FACING_OFF: f32 = FRAC_PI_2;
const LOS_EPS: f32 = 0.001;

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
        if !(p.reach > 0.0) {
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
            || !melee_line_clear(ctx.world, centre, ctx.player_pos)
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

fn melee_line_clear(world: &World, from: Vec3, to: Vec3) -> bool {
    let delta = to - from;
    let dist = delta.length();
    if dist <= f32::EPSILON {
        return true;
    }
    !ray_hits_collision(world, from, delta / dist, dist)
}

fn ray_hits_collision(world: &World, eye: Vec3, dir: Vec3, max_t: f32) -> bool {
    let mut ix = eye.x.floor() as i32;
    let mut iy = eye.y.floor() as i32;
    let mut iz = eye.z.floor() as i32;
    let step = IVec3::new(sign(dir.x), sign(dir.y), sign(dir.z));
    let mut t_max = Vec3::new(
        boundary_t(eye.x, dir.x),
        boundary_t(eye.y, dir.y),
        boundary_t(eye.z, dir.z),
    );
    let t_delta = Vec3::new(inv_abs(dir.x), inv_abs(dir.y), inv_abs(dir.z));

    loop {
        if cell_hits_collision(world, eye, dir, max_t, IVec3::new(ix, iy, iz)) {
            return true;
        }
        let (axis, t_exit) = if t_max.x <= t_max.y && t_max.x <= t_max.z {
            (0, t_max.x)
        } else if t_max.y <= t_max.z {
            (1, t_max.y)
        } else {
            (2, t_max.z)
        };
        if t_exit > max_t {
            return false;
        }
        match axis {
            0 => {
                ix += step.x;
                t_max.x += t_delta.x;
            }
            1 => {
                iy += step.y;
                t_max.y += t_delta.y;
            }
            _ => {
                iz += step.z;
                t_max.z += t_delta.z;
            }
        }
    }
}

fn cell_hits_collision(world: &World, eye: Vec3, dir: Vec3, max_t: f32, cell: IVec3) -> bool {
    let base = Vec3::new(cell.x as f32, cell.y as f32, cell.z as f32);
    world
        .collision_boxes_at(cell.x, cell.y, cell.z)
        .iter()
        .any(|b| {
            let min = base + Vec3::from(b.min);
            let max = base + Vec3::from(b.max);
            ray_vs_aabb(eye, dir, min, max).is_some_and(|t| t > LOS_EPS && t < max_t - LOS_EPS)
        })
}

fn ray_vs_aabb(eye: Vec3, dir: Vec3, min: Vec3, max: Vec3) -> Option<f32> {
    let (e, d, lo, hi) = (
        eye.to_array(),
        dir.to_array(),
        min.to_array(),
        max.to_array(),
    );
    let mut t_near = f32::NEG_INFINITY;
    let mut t_far = f32::INFINITY;
    for i in 0..3 {
        if d[i].abs() < LOS_EPS {
            if e[i] < lo[i] - LOS_EPS || e[i] > hi[i] + LOS_EPS {
                return None;
            }
        } else {
            let inv = 1.0 / d[i];
            let mut t1 = (lo[i] - e[i]) * inv;
            let mut t2 = (hi[i] - e[i]) * inv;
            if t1 > t2 {
                std::mem::swap(&mut t1, &mut t2);
            }
            t_near = t_near.max(t1);
            t_far = t_far.min(t2);
            if t_near > t_far {
                return None;
            }
        }
    }
    (t_far >= 0.0).then_some(t_near.max(0.0))
}

/// Wrap an angle into `[-PI, PI]`.
fn wrap_angle(a: f32) -> f32 {
    (a + PI).rem_euclid(TAU) - PI
}

fn sign(v: f32) -> i32 {
    if v > 0.0 {
        1
    } else if v < 0.0 {
        -1
    } else {
        0
    }
}

fn inv_abs(v: f32) -> f32 {
    if v == 0.0 {
        f32::INFINITY
    } else {
        1.0 / v.abs()
    }
}

fn boundary_t(coord: f32, dir: f32) -> f32 {
    if dir > 0.0 {
        (coord.floor() + 1.0 - coord) / dir
    } else if dir < 0.0 {
        (coord - coord.floor()) / -dir
    } else {
        f32::INFINITY
    }
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
