//! Retaliate: turn on whatever last damaged this mob — after a WARMUP.
//!
//! The damage pipeline records the attacking entity (player or mob) on the
//! struck instance; this node reads that memory and — while it is fresher than
//! `memory_ticks` and the attacker is still alive — chases the attacker's live
//! position and publishes it as the brain's target, so a co-resident
//! `melee_attack` strikes back. Being hit is perception in its own right: even
//! a mob whose ordinary senses can't find the attacker (a hearing hunter axed
//! by a silent, sneaking player) knows exactly who bit it.
//!
//! The grudge takes `warmup_ticks` to boil over: for that long after the FIRST
//! hit of an engagement the node stays quiet — the mob reels instead of
//! counter-striking on the very tick it was hit. The warmup anchors on the
//! first hit deliberately: it counts on the node's own clock, so an attacker
//! re-hitting inside the window cannot keep resetting it and fight a mob that
//! never fights back. A NEW attacker restarts the warmup.
//!
//! Whether a species fights back at all is row data — compose the node into its
//! brain or don't. Its canonical priority sits ABOVE the chase slot: a mob
//! under attack drops its current hunt and answers the attacker first.

use serde::Deserialize;

use super::super::brain::{AiBehavior, AiCtx, BehaviorOutput};
use super::super::EntityRef;
use super::chase::goal_cell_near;

/// Default forget window: 10 s at 20 TPS — long enough to finish a fight,
/// short enough that a fled attacker is eventually forgiven.
const DEFAULT_MEMORY_TICKS: u32 = 200;
/// Default boil-over delay: 1 s at 20 TPS between the first hit and the mob
/// turning on its attacker.
const DEFAULT_WARMUP_TICKS: u32 = 20;

/// `retaliate` params as written in a `mobs.json` brain row.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RetaliateParams {
    /// Ticks since the LAST hit before the grudge is forgotten.
    #[serde(default = "default_memory")]
    memory_ticks: u32,
    /// Ticks after the FIRST hit before the mob turns on the attacker.
    #[serde(default = "default_warmup")]
    warmup_ticks: u32,
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
}

impl RetaliateAi {
    pub fn new(memory_ticks: u32, warmup_ticks: u32) -> Self {
        RetaliateAi {
            memory_ticks: memory_ticks.max(1),
            warmup_ticks,
            grudge: None,
            grudge_ticks: 0,
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
        Ok(RetaliateAi::new(p.memory_ticks, p.warmup_ticks))
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
            return BehaviorOutput::default();
        };
        // A new attacker starts a new grudge (and a new warmup); re-hits from
        // the same one only keep the MEMORY fresh — the warmup clock is this
        // node's own and never rewinds.
        if self.grudge != Some(who) {
            self.grudge = Some(who);
            self.grudge_ticks = 0;
        }
        self.grudge_ticks = self.grudge_ticks.saturating_add(1);
        if self.grudge_ticks <= self.warmup_ticks {
            // Still reeling: no counter-strike on (or right after) the tick
            // the hit landed.
            return BehaviorOutput::default();
        }
        // The attacker's live body-centre; a dead or vanished attacker ends
        // the grudge by simply resolving to nothing.
        let Some(pos) = ctx.entity_pos(who) else {
            return BehaviorOutput::default();
        };
        BehaviorOutput {
            goal: goal_cell_near(ctx, pos),
            target: Some(who),
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::Block;
    use crate::chunk::{Chunk, ChunkPos, CHUNK_SX, CHUNK_SZ};
    use crate::mathh::Vec3;
    use crate::mob::{brain::AiMob, Mob, MobRng, PlayerAnchor};
    use crate::server::player::PlayerId;
    use crate::world::World;

    fn flat_world() -> World {
        let mut world = World::new(0, 1);
        let mut chunk = Chunk::new(0, 0);
        for z in 0..CHUNK_SZ {
            for x in 0..CHUNK_SX {
                chunk.set_block(x, 63, z, Block::Grass);
            }
        }
        world.insert_chunk_for_test(ChunkPos::new(0, 0), chunk);
        world
    }

    fn ctx<'a>(
        world: &'a World,
        rng: &'a mut MobRng,
        pos: crate::mathh::Vec3,
        players: &'a [PlayerAnchor],
        mobs: &'a [AiMob],
        attacker: Option<(EntityRef, u32)>,
    ) -> AiCtx<'a> {
        let mut c = crate::mob::behavior::test_support::ctx_at(world, rng, pos);
        c.half_width = 0.22;
        c.players = players;
        c.mobs = mobs;
        c.attacker = attacker;
        c
    }

    #[test]
    fn a_fresh_grudge_chases_the_attacker_and_ages_out() {
        let world = flat_world();
        let mut rng = MobRng::new(1);
        let mut ai = RetaliateAi::new(200, 0);
        let mob = Vec3::new(2.5, 64.0, 2.5);
        let biter = AiMob {
            id: 9,
            kind: Mob::Sheep,
            pos: Vec3::new(7.5, 64.0, 2.5),
            active: true,
            tags: Default::default(),
        };
        let mobs = [biter];
        let grudge = Some((EntityRef::Mob(9), 0));

        let out = ai.tick(&mut ctx(&world, &mut rng, mob, &[], &mobs, grudge));
        assert_eq!(out.target, Some(EntityRef::Mob(9)));
        assert!(out.goal.is_some(), "the grudge chases the biter");

        let stale = Some((EntityRef::Mob(9), 201));
        let out = ai.tick(&mut ctx(&world, &mut rng, mob, &[], &mobs, stale));
        assert_eq!(out.target, None, "an aged-out grudge is forgotten");
    }

    #[test]
    fn a_dead_or_absent_attacker_ends_the_grudge() {
        let world = flat_world();
        let mut rng = MobRng::new(1);
        let mut ai = RetaliateAi::new(200, 0);
        let mob = Vec3::new(2.5, 64.0, 2.5);
        let corpse = [AiMob {
            id: 9,
            kind: Mob::Sheep,
            pos: Vec3::new(7.5, 64.0, 2.5),
            active: false,
            tags: Default::default(),
        }];
        let grudge = Some((EntityRef::Mob(9), 0));
        let out = ai.tick(&mut ctx(&world, &mut rng, mob, &[], &corpse, grudge));
        assert_eq!(out.target, None, "no vengeance on a corpse");

        // A player attacker who disconnected resolves to nothing the same way.
        let gone = Some((EntityRef::Player(PlayerId(7)), 0));
        let out = ai.tick(&mut ctx(&world, &mut rng, mob, &[], &[], gone));
        assert_eq!(out.target, None);
    }

    #[test]
    fn a_player_attacker_is_chased_by_live_anchor_position() {
        let world = flat_world();
        let mut rng = MobRng::new(1);
        let mut ai = RetaliateAi::new(200, 0);
        let mob = Vec3::new(2.5, 64.0, 2.5);
        let players = [PlayerAnchor {
            id: PlayerId(7),
            pos: Vec3::new(9.5, 64.9, 2.5),
            sneaking: true, // sneaking does not hide an attacker from their victim
            ..Default::default()
        }];
        let grudge = Some((EntityRef::Player(PlayerId(7)), 3));
        let out = ai.tick(&mut ctx(&world, &mut rng, mob, &players, &[], grudge));
        assert_eq!(out.target, Some(EntityRef::Player(PlayerId(7))));
        assert!(out.goal.is_some());
    }

    #[test]
    fn the_warmup_delays_the_counter_and_rehits_cannot_rewind_it() {
        let world = flat_world();
        let mut rng = MobRng::new(1);
        let mut ai = RetaliateAi::new(200, 20);
        let mob = Vec3::new(2.5, 64.0, 2.5);
        let biter = [AiMob {
            id: 9,
            kind: Mob::Sheep,
            pos: Vec3::new(7.5, 64.0, 2.5),
            active: true,
            tags: Default::default(),
        }];

        // The hit tick and the following warmup window: the mob reels, no
        // target — it cannot answer on the tick it was struck. The attacker
        // re-hits midway (age snaps back to 0): the warmup clock is the
        // node's own and must NOT rewind.
        for tick in 0..20u32 {
            let age = if tick < 10 { tick } else { tick - 10 };
            let grudge = Some((EntityRef::Mob(9), age));
            let out = ai.tick(&mut ctx(&world, &mut rng, mob, &[], &biter, grudge));
            assert_eq!(out.target, None, "still warming up at tick {tick}");
        }
        let out = ai.tick(&mut ctx(
            &world,
            &mut rng,
            mob,
            &[],
            &biter,
            Some((EntityRef::Mob(9), 11)),
        ));
        assert_eq!(
            out.target,
            Some(EntityRef::Mob(9)),
            "the warmup elapsed exactly once despite the re-hit"
        );

        // A DIFFERENT attacker restarts the warmup from zero.
        let out = ai.tick(&mut ctx(
            &world,
            &mut rng,
            mob,
            &[],
            &biter,
            Some((EntityRef::Player(PlayerId(4)), 0)),
        ));
        assert_eq!(out.target, None, "a new attacker starts a new warmup");
    }

    #[test]
    fn params_are_validated_at_load() {
        assert!(RetaliateAi::from_params(&serde_json::json!({})).is_ok());
        assert!(RetaliateAi::from_params(&serde_json::json!({"memory_ticks": 100})).is_ok());
        assert!(RetaliateAi::from_params(
            &serde_json::json!({"memory_ticks": 100, "warmup_ticks": 10})
        )
        .is_ok());
        assert!(
            RetaliateAi::from_params(&serde_json::json!({"memory_ticks": 0})).is_err(),
            "zero memory is refused"
        );
        assert!(
            RetaliateAi::from_params(
                &serde_json::json!({"memory_ticks": 100, "warmup_ticks": 100})
            )
            .is_err(),
            "a warmup the memory cannot outlive is refused"
        );
        assert!(
            RetaliateAi::from_params(&serde_json::json!({"bogus": 1})).is_err(),
            "unknown params are refused"
        );
    }
}
