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

use crate::mathh::Vec3;

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
        self.cooldown = self.cooldown_ticks;
        BehaviorOutput {
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
mod tests {
    use super::*;
    use crate::block::Block;
    use crate::chunk::ChunkPos;
    use crate::mob::MobRng;
    use crate::world::World;

    /// A ctx whose brain has the (default-id) player LOCKED — melee only
    /// strikes a published lock, so the classic strike tests provide one.
    /// The anchor slice is leaked: test-only, and `AiCtx` borrows it.
    fn ctx<'a>(
        world: &'a World,
        rng: &'a mut MobRng,
        pos: Vec3,
        yaw: f32,
        player: Vec3,
    ) -> AiCtx<'a> {
        let players: &'static [crate::mob::PlayerAnchor] =
            Box::leak(Box::new([crate::mob::PlayerAnchor {
                pos: player,
                ..Default::default()
            }]));
        let mut c = crate::mob::behavior::test_support::ctx_at(world, rng, pos);
        c.yaw = yaw;
        c.head_height = 1.3;
        c.half_width = 0.45;
        c.player_pos = player;
        c.players = players;
        c.target = Some(EntityRef::Player(Default::default()));
        c.head = 2;
        c
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
                target: EntityRef::Player(Default::default()),
                damage: 2.0,
                knockback: 5.0
            },
            "the intent names the locked player"
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
    fn no_lock_means_no_strike_even_in_reach() {
        // Attack executes on perception's decision; it never perceives on its
        // own. An unlocked mob standing on top of the player swings at nothing
        // — this is what makes a silent player safe beside a blind hunter.
        let world = World::new(0, 1);
        let mut rng = MobRng::new(1);
        let mut ai = MeleeAttackAi::new(1.5, 2.0, 5.0, 10);
        let (pos, yaw, player) = in_reach();
        let mut c = ctx(&world, &mut rng, pos, yaw, player);
        c.target = None;
        assert!(ai.tick(&mut c).attack.is_none());
        // And the non-strike never armed the cooldown.
        assert!(ai
            .tick(&mut ctx(&world, &mut rng, pos, yaw, player))
            .attack
            .is_some());
    }

    #[test]
    fn a_locked_mob_target_is_struck_and_a_vanished_one_fizzles() {
        use super::super::super::brain::AiMob;
        let world = World::new(0, 1);
        let mut rng = MobRng::new(1);
        let mut ai = MeleeAttackAi::new(1.5, 4.0, 5.0, 10);
        let pos = Vec3::new(8.5, 64.0, 8.5);
        // A victim mob one block in front of the striker's face (-Z), well
        // inside reach once its own half-width pads the gap. The player is far
        // away — a locked mob target must NOT fall back to the player.
        let victim = AiMob {
            id: 7,
            kind: crate::mob::Mob::Sheep,
            pos: Vec3::new(8.5, 64.0, 7.2),
            active: true,
            tags: Default::default(),
        };
        let far_player = Vec3::new(80.0, 64.9, 80.0);

        let mut c = ctx(&world, &mut rng, pos, 0.0, far_player);
        let mobs = [victim];
        c.mobs = &mobs;
        c.target = Some(EntityRef::Mob(7));
        let intent = ai.tick(&mut c).attack.expect("locked mob target in reach");
        assert_eq!(
            intent.target,
            EntityRef::Mob(7),
            "the intent names the locked mob"
        );

        // The same lock with the victim gone (dead / despawned): no strike, no
        // player fallback, and the miss never armed the cooldown.
        let mut c = ctx(&world, &mut rng, pos, 0.0, far_player);
        c.target = Some(EntityRef::Mob(7));
        assert!(
            ai.tick(&mut c).attack.is_none(),
            "a vanished lock fizzles instead of striking the player"
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
