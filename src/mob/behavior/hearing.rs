//! Hearing chase (`chase_sound`): hunt by NOISE, not sight.
//!
//! The node listens to the tick's gameplay noises (see `mob::noise`) within
//! `radius` of the mob. A player noise locks the player who made it; a mob noise
//! may — with per-heard-tick probability `mob_chance`, and only for species in
//! the `mob_targets` whitelist — lock that mob instead. While locked, the node
//! emits the target's LIVE position as the navigation goal every tick and
//! publishes the lock as the brain's target, with **no line-of-sight anywhere**:
//! a hearing hunter tracks through walls. What it can never do is hear the
//! inaudible — noise *emission* already excludes sneaking players, so sneaking
//! is invisible to this node by construction, not by a radius penalty.
//!
//! The lock decays on silence: `memory_ticks` consecutive ticks without a
//! qualifying noise FROM THE LOCKED TARGET within `radius` drops it (each such
//! noise resets the countdown). The lock is committed — other entities' noises
//! neither refresh nor steal it — so a multiplayer decoy can't retarget a chase
//! mid-hunt; once the lock drops, the loudest world wins again.
//!
//! Player noises always beat mob noises at acquisition (players are the point
//! of a hostile), and the `mob_chance` roll is drawn from the mob's own
//! deterministic RNG only on ticks where an eligible mob noise was actually
//! heard, so quiet worlds don't consume the stream.

use serde::Deserialize;

use crate::mathh::Vec3;

use super::super::brain::{AiBehavior, AiCtx, BehaviorOutput};
use super::super::{EntityRef, Mob, MobDef};
use super::chase::goal_cell_near;

/// `chase_sound` params as written in a `mobs.json` brain row.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChaseSoundParams {
    /// Hearing range in blocks (3-D) — noises beyond it don't exist to this mob.
    radius: f64,
    /// Consecutive silent ticks (no qualifying noise from the locked target)
    /// before the lock drops.
    memory_ticks: u32,
    /// Per-heard-tick probability of locking onto an eligible MOB noise source
    /// while unlocked. Optional; defaults to 0 (never hunts mobs).
    #[serde(default)]
    mob_chance: f64,
    /// Species keys eligible for mob targeting (`["monsters:zombie"]`).
    /// Optional; empty means no mob is ever a target regardless of chance.
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

    /// Build from a brain row's `params` — the `chase_sound` node factory core.
    /// Species keys resolve against `all` — the def table handed down by the
    /// factory seam (the IN-FLIGHT table during load validation; calling
    /// `defs()` here would re-enter its initializing LazyLock and deadlock the
    /// load) — so a typo'd or missing-pack whitelist entry fails the load,
    /// never the first spawn.
    pub(super) fn from_params(params: &serde_json::Value, all: &[MobDef]) -> Result<Self, String> {
        let p: ChaseSoundParams =
            serde_json::from_value(params.clone()).map_err(|e| e.to_string())?;
        // `partial_cmp` (not `<=`) so a NaN radius is rejected too.
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

    /// Try to acquire a lock from this tick's audible noises. Player noises
    /// win outright (nearest first); mob noises need the whitelist AND the
    /// per-tick chance roll (drawn once, only when an eligible one was heard).
    fn acquire(&mut self, ctx: &mut AiCtx) {
        let r2 = self.radius * self.radius;
        let audible = |pos: Vec3| (pos - ctx.pos).length_squared() <= r2;

        let nearest_player = ctx
            .noises
            .iter()
            .filter(|n| matches!(n.source, EntityRef::Player(_)) && audible(n.pos))
            .min_by(|a, b| {
                let da = (a.pos - ctx.pos).length_squared();
                let db = (b.pos - ctx.pos).length_squared();
                da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
            });
        if let Some(noise) = nearest_player {
            self.target = Some(noise.source);
            self.silent_ticks = 0;
            return;
        }

        if self.mob_chance <= 0.0 || self.mob_targets.is_empty() {
            return;
        }
        let eligible_mob = |source: EntityRef| {
            let EntityRef::Mob(id) = source else {
                return false;
            };
            // Never its own footsteps, and only whitelisted, still-live species.
            id != ctx.mob_id
                && ctx
                    .mobs
                    .iter()
                    .any(|m| m.id == id && m.active && self.mob_targets.contains(&m.kind))
        };
        let nearest_mob = ctx
            .noises
            .iter()
            .filter(|n| audible(n.pos) && eligible_mob(n.source))
            .min_by(|a, b| {
                let da = (a.pos - ctx.pos).length_squared();
                let db = (b.pos - ctx.pos).length_squared();
                da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
            });
        // The roll draws only when something eligible was actually heard, so
        // the RNG stream is untouched on quiet ticks (like the despawn roll).
        if let Some(noise) = nearest_mob {
            if ctx.rng.next_f32() < self.mob_chance {
                self.target = Some(noise.source);
                self.silent_ticks = 0;
            }
        }
    }
}

impl AiBehavior for ChaseSoundAi {
    fn tick(&mut self, ctx: &mut AiCtx) -> BehaviorOutput {
        // A dead/vanished target unlocks immediately.
        if self.target.is_some_and(|t| !ctx.entity_alive(t)) {
            self.target = None;
        }
        // The silence countdown: any qualifying noise FROM THE LOCKED TARGET
        // within hearing resets it; memory_ticks of silence drops the lock.
        if let Some(locked) = self.target {
            let r2 = self.radius * self.radius;
            let heard = ctx
                .noises
                .iter()
                .any(|n| n.source == locked && (n.pos - ctx.pos).length_squared() <= r2);
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
        // Chase the LIVE position, through walls — no line of sight anywhere.
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
    use crate::block::Block;
    use crate::chunk::{Chunk, ChunkPos, CHUNK_SX, CHUNK_SZ};
    use crate::mob::{brain::AiMob, MobRng, Noise, NoiseKind, PlayerAnchor};
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

    fn anchor(id: u8, pos: Vec3) -> PlayerAnchor {
        PlayerAnchor {
            id: PlayerId(id),
            pos,
            body: None,
            sneaking: false,
        }
    }

    fn step(pos: Vec3, source: EntityRef) -> Noise {
        Noise {
            pos,
            kind: NoiseKind::Step,
            source,
        }
    }

    fn ctx<'a>(
        world: &'a World,
        rng: &'a mut MobRng,
        pos: Vec3,
        players: &'a [PlayerAnchor],
        noises: &'a [Noise],
        mobs: &'a [AiMob],
    ) -> AiCtx<'a> {
        AiCtx {
            mob_id: 1,
            pos,
            cell: crate::mathh::voxel_at(pos),
            yaw: 0.0,
            head_height: 0.7,
            half_width: 0.22,
            world,
            player_id: players.first().map(|a| a.id).unwrap_or_default(),
            player_pos: players.first().map(|a| a.pos).unwrap_or(Vec3::ZERO),
            player_sneaking: false,
            players,
            noises,
            contacts: &[],
            target: None,
            attacker: None,
            nav_idle: true,
            in_water: false,
            head: 1,
            idle_anims: &[],
            mob_index: 0,
            mobs,
            rng,
        }
    }

    #[test]
    fn a_heard_player_is_locked_and_tracked_through_walls() {
        let mut world = flat_world();
        let mut rng = MobRng::new(1);
        let mut ai = ChaseSoundAi::new(12.0, 40, 0.0, Vec::new());
        let mob = Vec3::new(2.5, 64.0, 2.5);
        let player = Vec3::new(9.5, 64.9, 2.5); // 7 blocks: audible
        let players = [anchor(3, player)];

        // A solid wall between them — sight-based chase would refuse; hearing
        // doesn't care.
        for y in 64..=66 {
            assert!(world.set_block_world(5, y, 2, Block::Stone));
        }

        let noises = [step(player, EntityRef::Player(PlayerId(3)))];
        let out = ai.tick(&mut ctx(&world, &mut rng, mob, &players, &noises, &[]));
        assert!(out.goal.is_some(), "a heard step locks and chases");
        assert_eq!(out.target, Some(EntityRef::Player(PlayerId(3))));

        // Silent ticks: the chase persists on the LIVE position for
        // memory_ticks - 1 ticks, then the lock drops.
        for t in 1..40 {
            let out = ai.tick(&mut ctx(&world, &mut rng, mob, &players, &[], &[]));
            assert!(out.goal.is_some(), "still locked at silent tick {t}");
        }
        let out = ai.tick(&mut ctx(&world, &mut rng, mob, &players, &[], &[]));
        assert_eq!(out.goal, None, "40 silent ticks drop the lock");
        assert_eq!(out.target, None);
    }

    #[test]
    fn every_target_noise_in_range_resets_the_silence_countdown() {
        let world = flat_world();
        let mut rng = MobRng::new(1);
        let mut ai = ChaseSoundAi::new(12.0, 40, 0.0, Vec::new());
        let mob = Vec3::new(2.5, 64.0, 2.5);
        let player = Vec3::new(9.5, 64.9, 2.5);
        let players = [anchor(3, player)];
        let noise = [step(player, EntityRef::Player(PlayerId(3)))];

        assert!(ai
            .tick(&mut ctx(&world, &mut rng, mob, &players, &noise, &[]))
            .goal
            .is_some());
        // 39 silent ticks, then one more noise: the countdown restarts whole.
        for _ in 0..39 {
            assert!(ai
                .tick(&mut ctx(&world, &mut rng, mob, &players, &[], &[]))
                .goal
                .is_some());
        }
        assert!(ai
            .tick(&mut ctx(&world, &mut rng, mob, &players, &noise, &[]))
            .goal
            .is_some());
        for t in 1..40 {
            assert!(
                ai.tick(&mut ctx(&world, &mut rng, mob, &players, &[], &[]))
                    .goal
                    .is_some(),
                "reset countdown holds at tick {t}"
            );
        }
        assert!(ai
            .tick(&mut ctx(&world, &mut rng, mob, &players, &[], &[]))
            .goal
            .is_none());
    }

    #[test]
    fn noise_beyond_the_hearing_radius_neither_locks_nor_refreshes() {
        let world = flat_world();
        let mut rng = MobRng::new(1);
        let mut ai = ChaseSoundAi::new(12.0, 40, 0.0, Vec::new());
        let mob = Vec3::new(2.5, 64.0, 2.5);
        let far = Vec3::new(20.5, 64.9, 2.5); // 18 blocks: out of hearing
        let players = [anchor(3, far)];
        let noises = [step(far, EntityRef::Player(PlayerId(3)))];

        assert_eq!(
            ai.tick(&mut ctx(&world, &mut rng, mob, &players, &noises, &[]))
                .goal,
            None,
            "an out-of-range noise does not exist to this mob"
        );

        // Lock from an in-range noise, then move the player out of hearing:
        // their far noises no longer refresh, and the lock times out even
        // though they keep stomping.
        let near = Vec3::new(9.5, 64.9, 2.5);
        let near_players = [anchor(3, near)];
        let near_noise = [step(near, EntityRef::Player(PlayerId(3)))];
        assert!(ai
            .tick(&mut ctx(
                &world,
                &mut rng,
                mob,
                &near_players,
                &near_noise,
                &[]
            ))
            .goal
            .is_some());
        for _ in 0..40 {
            ai.tick(&mut ctx(&world, &mut rng, mob, &players, &noises, &[]));
        }
        assert_eq!(
            ai.tick(&mut ctx(&world, &mut rng, mob, &players, &noises, &[]))
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
        let mob = Vec3::new(2.5, 64.0, 2.5);
        let a = Vec3::new(9.5, 64.9, 2.5);
        let b = Vec3::new(4.5, 64.9, 2.5); // B is NEARER than A
        let players = [anchor(3, a), anchor(4, b)];

        let only_a = [step(a, EntityRef::Player(PlayerId(3)))];
        let out = ai.tick(&mut ctx(&world, &mut rng, mob, &players, &only_a, &[]));
        assert_eq!(out.target, Some(EntityRef::Player(PlayerId(3))));

        // B stomps closer while A stays audible: the lock holds on A.
        let both = [
            step(b, EntityRef::Player(PlayerId(4))),
            step(a, EntityRef::Player(PlayerId(3))),
        ];
        let out = ai.tick(&mut ctx(&world, &mut rng, mob, &players, &both, &[]));
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
        let mob = Vec3::new(2.5, 64.0, 2.5);
        let prey_pos = Vec3::new(7.5, 64.0, 2.5);
        let mobs = [
            AiMob {
                id: 1, // the listener itself
                kind: Mob::Owl,
                pos: mob,
                active: true,
            },
            AiMob {
                id: 9,
                kind: Mob::Sheep,
                pos: prey_pos,
                active: true,
            },
        ];

        // Chance 1.0 with the sheep whitelisted: the first heard tick locks it.
        let mut ai = ChaseSoundAi::new(12.0, 40, 1.0, vec![Mob::Sheep]);
        let noises = [
            step(mob, EntityRef::Mob(1)), // its own footsteps: never a target
            step(prey_pos, EntityRef::Mob(9)),
        ];
        let out = ai.tick(&mut ctx(&world, &mut rng, mob, &[], &noises, &mobs));
        assert_eq!(out.target, Some(EntityRef::Mob(9)));
        assert!(out.goal.is_some(), "locked prey is chased");

        // An empty whitelist never locks a mob regardless of chance.
        let mut deaf = ChaseSoundAi::new(12.0, 40, 1.0, Vec::new());
        let out = deaf.tick(&mut ctx(&world, &mut rng, mob, &[], &noises, &mobs));
        assert_eq!(out.target, None);

        // Chance 0 never locks either.
        let mut timid = ChaseSoundAi::new(12.0, 40, 0.0, vec![Mob::Sheep]);
        let out = timid.tick(&mut ctx(&world, &mut rng, mob, &[], &noises, &mobs));
        assert_eq!(out.target, None);
    }

    #[test]
    fn a_player_noise_outranks_a_mob_noise_at_acquisition() {
        let world = flat_world();
        let mut rng = MobRng::new(1);
        let mob = Vec3::new(2.5, 64.0, 2.5);
        let prey_pos = Vec3::new(4.5, 64.0, 2.5); // mob noise NEARER
        let player = Vec3::new(9.5, 64.9, 2.5);
        let players = [anchor(3, player)];
        let mobs = [AiMob {
            id: 9,
            kind: Mob::Sheep,
            pos: prey_pos,
            active: true,
        }];
        let noises = [
            step(prey_pos, EntityRef::Mob(9)),
            step(player, EntityRef::Player(PlayerId(3))),
        ];
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
        let mob = Vec3::new(2.5, 64.0, 2.5);
        let prey_pos = Vec3::new(7.5, 64.0, 2.5);
        let alive = [AiMob {
            id: 9,
            kind: Mob::Sheep,
            pos: prey_pos,
            active: true,
        }];
        let dead = [AiMob {
            id: 9,
            kind: Mob::Sheep,
            pos: prey_pos,
            active: false,
        }];
        let noises = [step(prey_pos, EntityRef::Mob(9))];
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
