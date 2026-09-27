use serde::Deserialize;

use super::super::brain::{AiBehavior, AiCtx, BehaviorOutput};
use super::super::EntityRef;
use super::chase::goal_cell_near;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChaseContactParams {
    memory_ticks: u32,
}

pub struct ChaseContactAi {
    memory_ticks: u32,
    target: Option<EntityRef>,
    untouched_ticks: u32,
}

impl ChaseContactAi {
    pub fn new(memory_ticks: u32) -> Self {
        ChaseContactAi {
            memory_ticks: memory_ticks.max(1),
            target: None,
            untouched_ticks: 0,
        }
    }

    pub(super) fn from_params(params: &serde_json::Value) -> Result<Self, String> {
        let p: ChaseContactParams =
            serde_json::from_value(params.clone()).map_err(|e| e.to_string())?;
        if p.memory_ticks == 0 {
            return Err("memory_ticks must be >= 1".into());
        }
        Ok(ChaseContactAi::new(p.memory_ticks))
    }
}

impl AiBehavior for ChaseContactAi {
    fn tick(&mut self, ctx: &mut AiCtx) -> BehaviorOutput {
        if let Some(locked) = self.target {
            if !ctx.entity_alive(locked) {
                self.target = None;
            }
        }
        if let Some(locked) = self.target {
            if ctx.contacts.contains(&locked) {
                self.untouched_ticks = 0;
            } else {
                self.untouched_ticks = self.untouched_ticks.saturating_add(1);
                if self.untouched_ticks >= self.memory_ticks {
                    self.target = None;
                }
            }
        }
        if self.target.is_none() {
            self.target = ctx
                .contacts
                .iter()
                .copied()
                .filter(|&who| who != EntityRef::Mob(ctx.mob_id) && ctx.entity_alive(who))
                .min_by(|&a, &b| {
                    let da = ctx
                        .entity_pos(a)
                        .map_or(f32::MAX, |p| (p - ctx.pos).length_squared());
                    let db = ctx
                        .entity_pos(b)
                        .map_or(f32::MAX, |p| (p - ctx.pos).length_squared());
                    da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
                });
            if self.target.is_some() {
                self.untouched_ticks = 0;
            }
        }
        let Some(pos) = self.target.and_then(|who| ctx.entity_pos(who)) else {
            return BehaviorOutput::default();
        };
        BehaviorOutput {
            goal: goal_cell_near(ctx, pos),
            target: self.target,
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mob::spatial::MobSnapshot;
    use crate::mob::{brain::AiMob, Mob, MobRng, PlayerAnchor};
    use crate::player::PlayerId;
    use crate::world::ServerWorld;
    use petramond_math::world_pos::WorldPos;
    use petramond_world::block::Block;
    use petramond_world::chunk::{Chunk, ChunkPos, CHUNK_SX, CHUNK_SZ};

    fn flat_world() -> ServerWorld {
        let mut world = ServerWorld::new(0, 1);
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
        world: &'a ServerWorld,
        rng: &'a mut MobRng,
        pos: WorldPos,
        players: &'a [PlayerAnchor],
        contacts: &'a [EntityRef],
        mobs: &'a MobSnapshot,
    ) -> AiCtx<'a> {
        let mut c = crate::mob::behavior::test_support::ctx_at(world, rng, pos);
        c.half_width = 0.45;
        c.player_id = players.first().map(|a| a.id).unwrap_or_default();
        c.player_pos = players.first().map(|a| a.pos).unwrap_or(WorldPos::ZERO);
        c.players = players;
        c.contacts = contacts;
        c.mobs = mobs;
        c
    }

    #[test]
    fn a_touching_player_is_locked_even_while_silent_and_the_lock_decays() {
        let world = flat_world();
        let mut rng = MobRng::new(1);
        let mut ai = ChaseContactAi::new(40);
        let mob = WorldPos::new(8.5, 64.0, 8.5);
        let players = [PlayerAnchor {
            id: PlayerId(3),
            pos: WorldPos::new(9.1, 64.9, 8.5),
            sneaking: true,
            ..Default::default()
        }];
        let bump = [EntityRef::Player(PlayerId(3))];

        let out = ai.tick(&mut ctx(
            &world,
            &mut rng,
            mob,
            &players,
            &bump,
            MobSnapshot::empty(),
        ));
        assert_eq!(out.target, Some(EntityRef::Player(PlayerId(3))));
        assert!(out.goal.is_some(), "the bump locks and chases");

        for t in 1..40 {
            let out = ai.tick(&mut ctx(
                &world,
                &mut rng,
                mob,
                &players,
                &[],
                MobSnapshot::empty(),
            ));
            assert!(out.target.is_some(), "still locked at untouched tick {t}");
        }
        let out = ai.tick(&mut ctx(
            &world,
            &mut rng,
            mob,
            &players,
            &[],
            MobSnapshot::empty(),
        ));
        assert_eq!(out.target, None, "40 untouched ticks drop the lock");
    }

    #[test]
    fn any_species_locks_on_touch_and_own_id_never_does() {
        let world = flat_world();
        let mut rng = MobRng::new(1);
        let mut ai = ChaseContactAi::new(40);
        let mob = WorldPos::new(8.5, 64.0, 8.5);
        let mobs = MobSnapshot::from_mobs([AiMob {
            id: 9,
            kind: Mob::Sheep,
            pos: WorldPos::new(9.2, 64.0, 8.5),
            active: true,
            tags: Default::default(),
        }]);
        let bumps = [EntityRef::Mob(1), EntityRef::Mob(9)];
        let out = ai.tick(&mut ctx(&world, &mut rng, mob, &[], &bumps, &mobs));
        assert_eq!(
            out.target,
            Some(EntityRef::Mob(9)),
            "touch needs no whitelist and never locks itself"
        );
    }

    #[test]
    fn a_live_lock_is_not_stolen_by_a_new_bump() {
        let world = flat_world();
        let mut rng = MobRng::new(1);
        let mut ai = ChaseContactAi::new(40);
        let mob = WorldPos::new(8.5, 64.0, 8.5);
        let players = [
            PlayerAnchor {
                id: PlayerId(3),
                pos: WorldPos::new(9.1, 64.9, 8.5),
                ..Default::default()
            },
            PlayerAnchor {
                id: PlayerId(4),
                pos: WorldPos::new(8.5, 64.9, 9.0),
                ..Default::default()
            },
        ];
        let first = [EntityRef::Player(PlayerId(3))];
        let both = [
            EntityRef::Player(PlayerId(4)),
            EntityRef::Player(PlayerId(3)),
        ];

        let out = ai.tick(&mut ctx(
            &world,
            &mut rng,
            mob,
            &players,
            &first,
            MobSnapshot::empty(),
        ));
        assert_eq!(out.target, Some(EntityRef::Player(PlayerId(3))));
        let out = ai.tick(&mut ctx(
            &world,
            &mut rng,
            mob,
            &players,
            &both,
            MobSnapshot::empty(),
        ));
        assert_eq!(
            out.target,
            Some(EntityRef::Player(PlayerId(3))),
            "a committed lock answers the first offender"
        );
    }

    #[test]
    fn params_are_validated_at_load() {
        assert!(ChaseContactAi::from_params(&serde_json::json!({"memory_ticks": 40})).is_ok());
        assert!(
            ChaseContactAi::from_params(&serde_json::json!({})).is_err(),
            "memory_ticks is required"
        );
        assert!(
            ChaseContactAi::from_params(&serde_json::json!({"memory_ticks": 0})).is_err(),
            "zero memory is refused"
        );
        assert!(
            ChaseContactAi::from_params(&serde_json::json!({"memory_ticks": 40, "bogus": 1}))
                .is_err(),
            "unknown params are refused"
        );
    }
}
