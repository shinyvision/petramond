use serde::Deserialize;

use super::super::brain::{AiBehavior, AiCtx, BehaviorOutput};
use super::super::{EntityRef, Mob, MobDef, Noise};
use super::chase::goal_cell_near;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChaseSoundParams {
    radius: f64,
    memory_ticks: u32,
    #[serde(default)]
    mob_chance: f64,
    #[serde(default)]
    mob_targets: Vec<String>,
}

pub struct ChaseSoundAi {
    radius: f32,
    memory_ticks: u32,
    mob_chance: f32,
    mob_targets: Vec<Mob>,
    target: Option<EntityRef>,
    silent_ticks: u32,
}

impl ChaseSoundAi {
    pub fn new(radius: f32, memory_ticks: u32, mob_chance: f32, mob_targets: Vec<Mob>) -> Self {
        ChaseSoundAi {
            radius,
            memory_ticks: memory_ticks.max(1),
            mob_chance,
            mob_targets,
            target: None,
            silent_ticks: 0,
        }
    }

    pub(super) fn from_params(params: &serde_json::Value, all: &[MobDef]) -> Result<Self, String> {
        let p: ChaseSoundParams =
            serde_json::from_value(params.clone()).map_err(|e| e.to_string())?;
        if p.radius.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater) {
            return Err("radius must be > 0".into());
        }
        if p.memory_ticks == 0 {
            return Err("memory_ticks must be >= 1".into());
        }
        if !(0.0..=1.0).contains(&p.mob_chance) {
            return Err("mob_chance must be within 0..=1".into());
        }
        let mob_targets = p
            .mob_targets
            .iter()
            .map(|key| {
                all.iter()
                    .position(|d| d.key == key)
                    .map(|i| Mob(i as u8))
                    .ok_or_else(|| format!("unknown mob_targets species '{key}'"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ChaseSoundAi::new(
            p.radius as f32,
            p.memory_ticks,
            p.mob_chance as f32,
            mob_targets,
        ))
    }

    fn acquire(&mut self, ctx: &mut AiCtx) {
        let nearest_player = nearest(ctx, self.radius, |n: &Noise| {
            matches!(n.source, EntityRef::Player(_))
        });
        if let Some(source) = nearest_player {
            self.target = Some(source);
            self.silent_ticks = 0;
            return;
        }

        if self.mob_chance <= 0.0 || self.mob_targets.is_empty() {
            return;
        }
        let nearest_mob = nearest(ctx, self.radius, |n: &Noise| {
            let EntityRef::Mob(id) = n.source else {
                return false;
            };
            id != ctx.mob_id
                && ctx
                    .live_mob(id)
                    .is_some_and(|m| self.mob_targets.contains(&m.kind))
        });
        if let Some(source) = nearest_mob {
            if ctx.rng.next_f32() < self.mob_chance {
                self.target = Some(source);
                self.silent_ticks = 0;
            }
        }
    }
}

fn nearest(ctx: &AiCtx, radius: f32, eligible: impl Fn(&Noise) -> bool) -> Option<EntityRef> {
    ctx.noises
        .near(ctx.pos, radius)
        .filter(|(_, n)| eligible(n))
        .map(|(i, n)| ((n.pos - ctx.pos).length_squared(), i, n.source))
        .min_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)))
        .map(|(_, _, source)| source)
}

impl AiBehavior for ChaseSoundAi {
    fn tick(&mut self, ctx: &mut AiCtx) -> BehaviorOutput {
        if self.target.is_some_and(|t| !ctx.entity_alive(t)) {
            self.target = None;
        }
        if let Some(locked) = self.target {
            let heard = ctx
                .noises
                .near(ctx.pos, self.radius)
                .any(|(_, n)| n.source == locked);
            if heard {
                self.silent_ticks = 0;
            } else {
                self.silent_ticks = self.silent_ticks.saturating_add(1);
                if self.silent_ticks >= self.memory_ticks {
                    self.target = None;
                }
            }
        }
        if self.target.is_none() {
            self.acquire(ctx);
        }
        let Some(pos) = self.target.and_then(|t| ctx.entity_pos(t)) else {
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
    use crate::mob::{brain::AiMob, MobRng, NoiseField, NoiseKind, PlayerAnchor};
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

    fn anchor(id: u8, pos: WorldPos) -> PlayerAnchor {
        PlayerAnchor {
            id: PlayerId(id),
            pos,
            ..Default::default()
        }
    }

    fn step(pos: WorldPos, source: EntityRef) -> Noise {
        Noise {
            pos,
            kind: NoiseKind::Step,
            source,
        }
    }

    fn ctx<'a>(
        world: &'a ServerWorld,
        rng: &'a mut MobRng,
        pos: WorldPos,
        players: &'a [PlayerAnchor],
        noises: &'a NoiseField,
        mobs: &'a MobSnapshot,
    ) -> AiCtx<'a> {
        let mut c = crate::mob::behavior::test_support::ctx_at(world, rng, pos);
        c.half_width = 0.22;
        c.player_id = players.first().map(|a| a.id).unwrap_or_default();
        c.player_pos = players.first().map(|a| a.pos).unwrap_or(WorldPos::ZERO);
        c.players = players;
        c.noises = noises;
        c.mobs = mobs;
        c
    }

    #[test]
    fn a_heard_player_is_locked_and_tracked_through_walls() {
        let mut world = flat_world();
        let mut rng = MobRng::new(1);
        let mut ai = ChaseSoundAi::new(12.0, 40, 0.0, Vec::new());
        let mob = WorldPos::new(2.5, 64.0, 2.5);
        let player = WorldPos::new(9.5, 64.9, 2.5);
        let players = [anchor(3, player)];

        for y in 64..=66 {
            assert!(world.set_block_world(5, y, 2, Block::Stone));
        }

        let noises = NoiseField::from_noises([step(player, EntityRef::Player(PlayerId(3)))]);
        let out = ai.tick(&mut ctx(
            &world,
            &mut rng,
            mob,
            &players,
            &noises,
            MobSnapshot::empty(),
        ));
        assert!(out.goal.is_some(), "a heard step locks and chases");
        assert_eq!(out.target, Some(EntityRef::Player(PlayerId(3))));

        for t in 1..40 {
            let out = ai.tick(&mut ctx(
                &world,
                &mut rng,
                mob,
                &players,
                NoiseField::empty(),
                MobSnapshot::empty(),
            ));
            assert!(out.goal.is_some(), "still locked at silent tick {t}");
        }
        let out = ai.tick(&mut ctx(
            &world,
            &mut rng,
            mob,
            &players,
            NoiseField::empty(),
            MobSnapshot::empty(),
        ));
        assert_eq!(out.goal, None, "40 silent ticks drop the lock");
        assert_eq!(out.target, None);
    }

    #[test]
    fn every_target_noise_in_range_resets_the_silence_countdown() {
        let world = flat_world();
        let mut rng = MobRng::new(1);
        let mut ai = ChaseSoundAi::new(12.0, 40, 0.0, Vec::new());
        let mob = WorldPos::new(2.5, 64.0, 2.5);
        let player = WorldPos::new(9.5, 64.9, 2.5);
        let players = [anchor(3, player)];
        let noise = NoiseField::from_noises([step(player, EntityRef::Player(PlayerId(3)))]);

        assert!(ai
            .tick(&mut ctx(
                &world,
                &mut rng,
                mob,
                &players,
                &noise,
                MobSnapshot::empty()
            ))
            .goal
            .is_some());
        for _ in 0..39 {
            assert!(ai
                .tick(&mut ctx(
                    &world,
                    &mut rng,
                    mob,
                    &players,
                    NoiseField::empty(),
                    MobSnapshot::empty()
                ))
                .goal
                .is_some());
        }
        assert!(ai
            .tick(&mut ctx(
                &world,
                &mut rng,
                mob,
                &players,
                &noise,
                MobSnapshot::empty()
            ))
            .goal
            .is_some());
        for t in 1..40 {
            assert!(
                ai.tick(&mut ctx(
                    &world,
                    &mut rng,
                    mob,
                    &players,
                    NoiseField::empty(),
                    MobSnapshot::empty()
                ))
                .goal
                .is_some(),
                "reset countdown holds at tick {t}"
            );
        }
        assert!(ai
            .tick(&mut ctx(
                &world,
                &mut rng,
                mob,
                &players,
                NoiseField::empty(),
                MobSnapshot::empty()
            ))
            .goal
            .is_none());
    }

    #[test]
    fn noise_beyond_the_hearing_radius_neither_locks_nor_refreshes() {
        let world = flat_world();
        let mut rng = MobRng::new(1);
        let mut ai = ChaseSoundAi::new(12.0, 40, 0.0, Vec::new());
        let mob = WorldPos::new(2.5, 64.0, 2.5);
        let far = WorldPos::new(20.5, 64.9, 2.5);
        let players = [anchor(3, far)];
        let noises = NoiseField::from_noises([step(far, EntityRef::Player(PlayerId(3)))]);

        assert_eq!(
            ai.tick(&mut ctx(
                &world,
                &mut rng,
                mob,
                &players,
                &noises,
                MobSnapshot::empty()
            ))
            .goal,
            None,
            "an out-of-range noise does not exist to this mob"
        );

        let near = WorldPos::new(9.5, 64.9, 2.5);
        let near_players = [anchor(3, near)];
        let near_noise = NoiseField::from_noises([step(near, EntityRef::Player(PlayerId(3)))]);
        assert!(ai
            .tick(&mut ctx(
                &world,
                &mut rng,
                mob,
                &near_players,
                &near_noise,
                MobSnapshot::empty()
            ))
            .goal
            .is_some());
        for _ in 0..40 {
            ai.tick(&mut ctx(
                &world,
                &mut rng,
                mob,
                &players,
                &noises,
                MobSnapshot::empty(),
            ));
        }
        assert_eq!(
            ai.tick(&mut ctx(
                &world,
                &mut rng,
                mob,
                &players,
                &noises,
                MobSnapshot::empty()
            ))
            .goal,
            None,
            "a target that outran hearing is lost after memory_ticks"
        );
    }

    #[test]
    fn the_lock_is_committed_until_lost() {
        let world = flat_world();
        let mut rng = MobRng::new(1);
        let mut ai = ChaseSoundAi::new(12.0, 40, 0.0, Vec::new());
        let mob = WorldPos::new(2.5, 64.0, 2.5);
        let a = WorldPos::new(9.5, 64.9, 2.5);
        let b = WorldPos::new(4.5, 64.9, 2.5);
        let players = [anchor(3, a), anchor(4, b)];

        let only_a = NoiseField::from_noises([step(a, EntityRef::Player(PlayerId(3)))]);
        let out = ai.tick(&mut ctx(
            &world,
            &mut rng,
            mob,
            &players,
            &only_a,
            MobSnapshot::empty(),
        ));
        assert_eq!(out.target, Some(EntityRef::Player(PlayerId(3))));

        let both = NoiseField::from_noises([
            step(b, EntityRef::Player(PlayerId(4))),
            step(a, EntityRef::Player(PlayerId(3))),
        ]);
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
            "a committed lock is not stolen by a nearer noise"
        );
    }

    #[test]
    fn mob_noises_need_whitelist_and_chance_and_never_self() {
        let world = flat_world();
        let mut rng = MobRng::new(1);
        let mob = WorldPos::new(2.5, 64.0, 2.5);
        let prey_pos = WorldPos::new(7.5, 64.0, 2.5);
        let mobs = MobSnapshot::from_mobs([
            AiMob {
                id: 1,
                kind: Mob::Owl,
                pos: mob,
                active: true,
                tags: Default::default(),
            },
            AiMob {
                id: 9,
                kind: Mob::Sheep,
                pos: prey_pos,
                active: true,
                tags: Default::default(),
            },
        ]);

        let mut ai = ChaseSoundAi::new(12.0, 40, 1.0, vec![Mob::Sheep]);
        let noises = NoiseField::from_noises([
            step(mob, EntityRef::Mob(1)),
            step(prey_pos, EntityRef::Mob(9)),
        ]);
        let out = ai.tick(&mut ctx(&world, &mut rng, mob, &[], &noises, &mobs));
        assert_eq!(out.target, Some(EntityRef::Mob(9)));
        assert!(out.goal.is_some(), "locked prey is chased");

        let mut deaf = ChaseSoundAi::new(12.0, 40, 1.0, Vec::new());
        let out = deaf.tick(&mut ctx(&world, &mut rng, mob, &[], &noises, &mobs));
        assert_eq!(out.target, None);

        let mut timid = ChaseSoundAi::new(12.0, 40, 0.0, vec![Mob::Sheep]);
        let out = timid.tick(&mut ctx(&world, &mut rng, mob, &[], &noises, &mobs));
        assert_eq!(out.target, None);
    }

    #[test]
    fn a_player_noise_outranks_a_mob_noise_at_acquisition() {
        let world = flat_world();
        let mut rng = MobRng::new(1);
        let mob = WorldPos::new(2.5, 64.0, 2.5);
        let prey_pos = WorldPos::new(4.5, 64.0, 2.5);
        let player = WorldPos::new(9.5, 64.9, 2.5);
        let players = [anchor(3, player)];
        let mobs = MobSnapshot::from_mobs([AiMob {
            id: 9,
            kind: Mob::Sheep,
            pos: prey_pos,
            active: true,
            tags: Default::default(),
        }]);
        let noises = NoiseField::from_noises([
            step(prey_pos, EntityRef::Mob(9)),
            step(player, EntityRef::Player(PlayerId(3))),
        ]);
        let mut ai = ChaseSoundAi::new(12.0, 40, 1.0, vec![Mob::Sheep]);
        let out = ai.tick(&mut ctx(&world, &mut rng, mob, &players, &noises, &mobs));
        assert_eq!(
            out.target,
            Some(EntityRef::Player(PlayerId(3))),
            "players outrank mob prey even when the prey is nearer"
        );
    }

    #[test]
    fn a_dead_target_unlocks_immediately() {
        let world = flat_world();
        let mut rng = MobRng::new(1);
        let mob = WorldPos::new(2.5, 64.0, 2.5);
        let prey_pos = WorldPos::new(7.5, 64.0, 2.5);
        let alive = MobSnapshot::from_mobs([AiMob {
            id: 9,
            kind: Mob::Sheep,
            pos: prey_pos,
            active: true,
            tags: Default::default(),
        }]);
        let dead = MobSnapshot::from_mobs([AiMob {
            id: 9,
            kind: Mob::Sheep,
            pos: prey_pos,
            active: false,
            tags: Default::default(),
        }]);
        let noises = NoiseField::from_noises([step(prey_pos, EntityRef::Mob(9))]);
        let mut ai = ChaseSoundAi::new(12.0, 40, 1.0, vec![Mob::Sheep]);
        assert!(ai
            .tick(&mut ctx(&world, &mut rng, mob, &[], &noises, &alive))
            .goal
            .is_some());
        assert_eq!(
            ai.tick(&mut ctx(&world, &mut rng, mob, &[], &noises, &dead))
                .goal,
            None,
            "a corpse stops being a target the tick it dies"
        );
    }

    #[test]
    fn params_are_validated_at_load() {
        assert!(ChaseSoundAi::from_params(
            &serde_json::json!({"radius": 12.0, "memory_ticks": 40}),
            crate::mob::defs()
        )
        .is_ok());
        assert!(
            ChaseSoundAi::from_params(
                &serde_json::json!({"radius": 0.0, "memory_ticks": 40}),
                crate::mob::defs()
            )
            .is_err(),
            "zero radius is refused"
        );
        assert!(
            ChaseSoundAi::from_params(
                &serde_json::json!({"radius": 12.0, "memory_ticks": 0}),
                crate::mob::defs()
            )
            .is_err(),
            "zero memory is refused"
        );
        assert!(
            ChaseSoundAi::from_params(
                &serde_json::json!({
                    "radius": 12.0, "memory_ticks": 40, "mob_chance": 1.5
                }),
                crate::mob::defs()
            )
            .is_err(),
            "an out-of-range chance is refused"
        );
        assert!(
            ChaseSoundAi::from_params(
                &serde_json::json!({
                    "radius": 12.0, "memory_ticks": 40, "mob_chance": 0.5,
                    "mob_targets": ["petramond:sheep"]
                }),
                crate::mob::defs()
            )
            .is_ok(),
            "a real species key resolves"
        );
        assert!(
            ChaseSoundAi::from_params(
                &serde_json::json!({
                    "radius": 12.0, "memory_ticks": 40, "mob_chance": 0.5,
                    "mob_targets": ["nope:missing"]
                }),
                crate::mob::defs()
            )
            .is_err(),
            "an unknown species key fails the load"
        );
        assert!(
            ChaseSoundAi::from_params(
                &serde_json::json!({
                    "radius": 12.0, "memory_ticks": 40, "bogus": 1
                }),
                crate::mob::defs()
            )
            .is_err(),
            "unknown params are refused"
        );
    }
}
