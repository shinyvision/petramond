use super::*;
use crate::mob::{MobRng, PlayerAnchor};
use crate::player::PlayerId;
use crate::world::World;
use petramond_math::math::Vec3;
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

#[test]
fn flees_the_actual_archer_with_varied_reachable_legs_and_then_calms() {
    let world = flat_world();
    let mut rng = MobRng::new(43);
    let mut ai = PanicAi::from_params(&serde_json::json!({})).unwrap();
    let players = [
        PlayerAnchor {
            id: PlayerId(1),
            pos: Vec3::new(9.0, 64.9, 8.5),
            ..Default::default()
        },
        PlayerAnchor {
            id: PlayerId(7),
            pos: Vec3::new(2.5, 64.9, 8.5),
            ..Default::default()
        },
    ];
    let mut ctx =
        crate::mob::behavior::test_support::ctx_at(&world, &mut rng, Vec3::new(8.5, 64.0, 8.5));
    ctx.players = &players;
    ctx.player_pos = players[0].pos;
    ctx.attacker = Some((EntityRef::Player(PlayerId(7)), 0));
    let mut goals = std::collections::HashSet::new();
    for _ in 0..12 {
        let out = ai.tick(&mut ctx);
        assert_eq!(out.claims, PanicAi::HOLDS);
        assert!(out.target.is_none() && out.attack.is_none());
        let goal = out.goal.expect("open ground offers an escape");
        assert!(
            goal.x >= ctx.cell.x,
            "flee the distant shooter, not the nearer bystander"
        );
        goals.insert(goal);
    }
    assert!(
        goals.len() > 1,
        "panic changes legs instead of fixing one straight destination"
    );
    assert!(goals.iter().any(|g| g.z != ctx.cell.z));
    ctx.attacker = Some((EntityRef::Player(PlayerId(7)), 121));
    assert_eq!(ai.tick(&mut ctx), BehaviorOutput::default());
    ctx.attacker = Some((EntityRef::Player(PlayerId(99)), 0));
    assert_eq!(
        ai.tick(&mut ctx),
        BehaviorOutput::default(),
        "a disconnected shooter ends the response"
    );
}

#[test]
fn an_escape_leg_is_held_while_navigation_follows_it() {
    let world = flat_world();
    let mut rng = MobRng::new(9);
    let players = [PlayerAnchor {
        id: PlayerId(7),
        pos: Vec3::new(2.5, 64.9, 8.5),
        ..Default::default()
    }];
    let mut ai = PanicAi::from_params(&serde_json::json!({})).unwrap();
    let mut ctx =
        crate::mob::behavior::test_support::ctx_at(&world, &mut rng, Vec3::new(8.5, 64.0, 8.5));
    ctx.players = &players;
    ctx.attacker = Some((EntityRef::Player(PlayerId(7)), 0));
    let first = ai.tick(&mut ctx).goal.unwrap();
    ctx.nav_idle = false;
    for _ in 0..8 {
        assert_eq!(ai.tick(&mut ctx).goal, Some(first));
    }
}

#[test]
fn invalid_panic_parameters_are_refused() {
    for params in [
        serde_json::json!({"radius":1}),
        serde_json::json!({"radius":200}),
        serde_json::json!({"memory_ticks":0}),
        serde_json::json!({"speed_scale":-1.0}),
        serde_json::json!({"speed_scale":mod_api::MAX_MOB_SPEED_SCALE + 1.0}),
    ] {
        assert!(PanicAi::from_params(&params).is_err());
    }
}

#[test]
fn boxed_in_panic_does_not_choose_an_unreachable_escape() {
    let mut world = flat_world();
    for x in 7..=9 {
        for z in 7..=9 {
            if x == 8 && z == 8 {
                continue;
            }
            for y in 64..=66 {
                assert!(world.set_block_world(x, y, z, Block::Stone));
            }
        }
    }
    let mut rng = MobRng::new(13);
    let players = [PlayerAnchor {
        id: PlayerId(7),
        pos: Vec3::new(2.5, 64.9, 8.5),
        ..Default::default()
    }];
    let mut ai = PanicAi::from_params(&serde_json::json!({})).unwrap();
    let mut ctx =
        crate::mob::behavior::test_support::ctx_at(&world, &mut rng, Vec3::new(8.5, 64.0, 8.5));
    ctx.players = &players;
    ctx.attacker = Some((EntityRef::Player(PlayerId(7)), 0));
    let out = ai.tick(&mut ctx);
    assert!(out.goal.is_none());
    assert!(
        out.claims.contains(DecisionChannel::Goal),
        "boxed in, the mob stands: no lower node hands it a destination"
    );
}
