use super::*;
use crate::mob::{brain::AiMob, Mob, MobRng, PlayerAnchor};
use crate::player::PlayerId;
use crate::world::World;
use petramond_math::world_pos::WorldPos;
use petramond_world::block::Block;
use petramond_world::chunk::{Chunk, ChunkPos, CHUNK_SX, CHUNK_SZ};

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
    pos: WorldPos,
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
fn a_hidden_archer_causes_escape_until_sight_returns() {
    let mut world = flat_world();
    for y in 64..67 {
        assert!(world.set_block_world(5, y, 8, Block::Stone));
    }
    let mut rng = MobRng::new(5);
    let mut ai = RetaliateAi::new(200, 0, EscapeRoute::default());
    let pos = WorldPos::new(8.5, 64.0, 8.5);
    let players = [PlayerAnchor {
        id: PlayerId(7),
        pos: WorldPos::new(2.5, 64.9, 8.5),
        ..Default::default()
    }];
    let memory = Some((EntityRef::Player(PlayerId(7)), 0));
    let out = ai.tick(&mut ctx(&world, &mut rng, pos, &players, &[], memory));
    assert_eq!(out.claims, EscapeRoute::HOLDS);
    assert!(out.target.is_none());
    assert!(
        out.goal.is_some_and(|goal| goal.x > 8),
        "take a reachable route away from the blocked shooter"
    );
    for y in 64..67 {
        assert!(world.set_block_world(5, y, 8, Block::Air));
    }
    let out = ai.tick(&mut ctx(&world, &mut rng, pos, &players, &[], memory));
    assert!(
        out.claims.is_empty(),
        "in sight, nothing is held: melee may strike"
    );
    assert_eq!(out.target, Some(EntityRef::Player(PlayerId(7))));
    assert!(out.goal.is_some_and(|goal| goal.x < 8));
}

#[test]
fn a_fresh_grudge_chases_the_attacker_and_ages_out() {
    let world = flat_world();
    let mut rng = MobRng::new(1);
    let mut ai = RetaliateAi::new(200, 0, EscapeRoute::default());
    let mob = WorldPos::new(2.5, 64.0, 2.5);
    let biter = AiMob {
        id: 9,
        kind: Mob::Sheep,
        pos: WorldPos::new(7.5, 64.0, 2.5),
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
    let mut ai = RetaliateAi::new(200, 0, EscapeRoute::default());
    let mob = WorldPos::new(2.5, 64.0, 2.5);
    let corpse = [AiMob {
        id: 9,
        kind: Mob::Sheep,
        pos: WorldPos::new(7.5, 64.0, 2.5),
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
    let mut ai = RetaliateAi::new(200, 0, EscapeRoute::default());
    let mob = WorldPos::new(2.5, 64.0, 2.5);
    let players = [PlayerAnchor {
        id: PlayerId(7),
        pos: WorldPos::new(9.5, 64.9, 2.5),
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
    let mut ai = RetaliateAi::new(200, 20, EscapeRoute::default());
    let mob = WorldPos::new(2.5, 64.0, 2.5);
    let biter = [AiMob {
        id: 9,
        kind: Mob::Sheep,
        pos: WorldPos::new(7.5, 64.0, 2.5),
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
        RetaliateAi::from_params(&serde_json::json!({"memory_ticks": 100, "warmup_ticks": 100}))
            .is_err(),
        "a warmup the memory cannot outlive is refused"
    );
    assert!(
        RetaliateAi::from_params(&serde_json::json!({"bogus": 1})).is_err(),
        "unknown params are refused"
    );
    assert!(RetaliateAi::from_params(&serde_json::json!({"radius": 5})).is_ok());
    assert!(
        RetaliateAi::from_params(&serde_json::json!({"radius": 2})).is_err(),
        "the shared escape radius is validated here too"
    );
}
