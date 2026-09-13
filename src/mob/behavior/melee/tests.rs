use super::*;
use crate::mob::MobRng;
use crate::world::World;
use petramond_math::world_pos::WorldPos;
use petramond_world::block::Block;
use petramond_world::chunk::ChunkPos;

/// A ctx whose brain has the (default-id) player LOCKED — melee only
/// strikes a published lock, so the classic strike tests provide one.
/// The anchor slice is leaked: test-only, and `AiCtx` borrows it.
fn ctx<'a>(
    world: &'a World,
    rng: &'a mut MobRng,
    pos: WorldPos,
    yaw: f32,
    player: WorldPos,
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
fn in_reach() -> (WorldPos, f32, WorldPos) {
    (
        WorldPos::new(8.5, 64.0, 8.5),
        0.0,
        WorldPos::new(8.5, 64.9, 7.2),
    )
}

#[test]
fn windup_announces_once_then_hits_on_its_impact_tick() {
    let world = World::new(0, 1);
    let mut rng = MobRng::new(1);
    let mut ai = MeleeAttackAi::from_params(&serde_json::json!({
        "reach":1.5,"damage":2.0,"knockback":5.0,"cooldown_ticks":10,
        "windup_ticks":3,"animation":"swipe"
    }))
    .unwrap();
    let (pos, yaw, player) = in_reach();
    let out = ai.tick(&mut ctx(&world, &mut rng, pos, yaw, player));
    assert_eq!(out.animation.as_deref(), Some("swipe"));
    assert!(out.attack.is_none());
    for _ in 0..2 {
        let out = ai.tick(&mut ctx(&world, &mut rng, pos, yaw, player));
        assert!(out.attack.is_none() && out.animation.is_none());
    }
    let out = ai.tick(&mut ctx(&world, &mut rng, pos, yaw, player));
    assert!(out.attack.is_some() && out.animation.is_none());
    assert!(ai
        .tick(&mut ctx(&world, &mut rng, pos, yaw, player))
        .attack
        .is_none());
}

#[test]
fn windup_rechecks_reach_and_cannot_transfer_to_a_new_target() {
    let world = World::new(0, 1);
    let mut rng = MobRng::new(1);
    let (pos, yaw, player) = in_reach();
    for situation in 0..3 {
        let mut ai = MeleeAttackAi::new(1.5, 2.0, 5.0, 10);
        ai.windup_ticks = 1;
        assert!(ai
            .tick(&mut ctx(&world, &mut rng, pos, yaw, player))
            .attack
            .is_none());
        let mut c = ctx(
            &world,
            &mut rng,
            pos,
            yaw,
            player + Vec3::new(0.0, 0.0, -8.0),
        );
        if situation == 1 {
            c.target = None;
        } else if situation == 2 {
            c.target = Some(EntityRef::Mob(999));
        }
        assert!(ai.tick(&mut c).attack.is_none());
        assert!(ai.pending.is_none());
        assert!(
            ai.tick(&mut ctx(&world, &mut rng, pos, yaw, player))
                .attack
                .is_none(),
            "a missed swing still spends its cooldown"
        );
    }
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
    let pos = WorldPos::new(8.5, 64.0, 8.5);

    // 5 blocks away: out of reach.
    let far = WorldPos::new(8.5, 64.9, 3.5);
    assert!(ai
        .tick(&mut ctx(&world, &mut rng, pos, 0.0, far))
        .attack
        .is_none());

    // In reach at -Z but the mob faces +Z (yaw PI): squarely behind it.
    let behind = WorldPos::new(8.5, 64.9, 7.2);
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
    let pos = WorldPos::new(8.5, 64.0, 8.5);
    let player = WorldPos::new(8.5, 64.9, 5.8);

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
    let pos = WorldPos::new(8.5, 64.0, 8.5);
    // A victim mob one block in front of the striker's face (-Z), well
    // inside reach once its own half-width pads the gap. The player is far
    // away — a locked mob target must NOT fall back to the player.
    let victim = AiMob {
        id: 7,
        kind: crate::mob::Mob::Sheep,
        pos: WorldPos::new(8.5, 64.0, 7.2),
        active: true,
        tags: Default::default(),
    };
    let far_player = WorldPos::new(80.0, 64.9, 80.0);

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
    let ok =
        serde_json::json!({"reach": 1.5, "damage": 2.0, "knockback": 5.0, "cooldown_ticks": 20});
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
