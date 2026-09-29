use super::*;
use crate::mob::{Despawn, Mob, Mobs};
use crate::world::ServerWorld;

fn anchor_at(x: f64, z: f64) -> PlayerAnchor {
    PlayerAnchor {
        pos: WorldPos::new(x, 64.0, z),
        ..Default::default()
    }
}

#[test]
fn chunk_distance_is_chebyshev_to_the_nearest_player() {
    let anchors = [anchor_at(8.0, 8.0), anchor_at(-40.0, 200.0)];
    assert_eq!(chunk_distance(&anchors, WorldPos::new(15.9, 64.0, 0.0)), 0);
    assert_eq!(chunk_distance(&anchors, WorldPos::new(16.0, 64.0, 0.0)), 1);
    assert_eq!(chunk_distance(&anchors, WorldPos::new(-0.5, 64.0, 70.0)), 4);
    assert_eq!(
        chunk_distance(&anchors, WorldPos::new(-40.0, 64.0, 180.0)),
        1
    );
    assert_eq!(chunk_distance(&[], WorldPos::ZERO), u32::MAX);
}

#[test]
fn the_policy_tiers_mobs_by_distance_on_a_deterministic_staggered_schedule() {
    let policy = SimDistance::default();
    let anchors = [anchor_at(8.0, 8.0)];
    let near = Instance::new(Mob::Sheep, WorldPos::new(40.0, 64.0, 8.0), 0.0, 5);
    let mid = Instance::new(
        Mob::Sheep,
        WorldPos::new(8.0 + 16.0 * 8.0, 64.0, 8.0),
        0.0,
        5,
    );
    let far = Instance::new(
        Mob::Sheep,
        WorldPos::new(8.0 + 16.0 * 20.0, 64.0, 8.0),
        0.0,
        5,
    );
    for now in 0..16u64 {
        assert_eq!(policy.step(&near, &anchors, now), SimStep::Think);
        assert_eq!(policy.step(&far, &anchors, now), SimStep::Frozen);
        let expected = if (now + 5).is_multiple_of(4) {
            SimStep::Think
        } else {
            SimStep::Coast
        };
        assert_eq!(policy.step(&mid, &anchors, now), expected, "tick {now}");
    }
    for now in 0..4 {
        assert_eq!(
            SimDistance::UNLIMITED.step(&far, &anchors, now),
            SimStep::Think,
            "an unlimited policy never throttles"
        );
    }
}

#[test]
fn a_mod_driven_body_keeps_the_full_tick_anywhere() {
    let mut mobs = Mobs::new(0);
    mobs.set_sim_distance(SimDistance::default());
    assert!(mobs.spawn(Mob::Owl, WorldPos::new(8.0 + 16.0 * 30.0, 64.0, 8.0), 0.0));
    let id = mobs.instances()[0].id();
    assert!(mobs.set_mob_drive(id, Some([2.0, 0.0]), None, Some(1.0), false, false));
    let anchors = [anchor_at(8.0, 8.0)];
    assert_eq!(
        mobs.sim_distance().step(&mobs.instances()[0], &anchors, 0),
        SimStep::Think
    );
}

#[test]
fn a_nonsensical_policy_is_sanitized() {
    let policy = SimDistance {
        full_chunks: 8,
        reduced_chunks: 2,
        reduced_interval: 0,
    }
    .sanitized();
    assert_eq!(policy.reduced_chunks, 8);
    assert_eq!(policy.reduced_interval, 1);
    let parsed: SimDistance = serde_json::from_str(r#"{"full_chunks": 3}"#).expect("parses");
    assert_eq!(parsed.full_chunks, 3);
    assert_eq!(parsed.reduced_chunks, SimDistance::default().reduced_chunks);
}

#[test]
fn frozen_mobs_do_not_simulate_while_near_ones_do() {
    let world = ServerWorld::new(0, 1);
    let mut mobs = Mobs::new(0);
    mobs.set_sim_distance(SimDistance::default());
    let near_spot = WorldPos::new(8.0, 64.0, 8.0);
    let far_spot = WorldPos::new(8.0 + 16.0 * 20.0, 64.0, 8.0);
    assert!(mobs.spawn(Mob::Owl, near_spot, 0.0));
    assert!(mobs.spawn(Mob::Owl, far_spot, 0.0));
    let anchors = [anchor_at(8.0, 8.0)];
    for _ in 0..10 {
        mobs.tick(0.05, &world, &anchors, false);
    }
    let near = &mobs.instances()[0];
    let far = &mobs.instances()[1];
    assert!(near.pos.y < near_spot.y, "the near owl simulates (falls)");
    assert_eq!(far.pos, far_spot, "the frozen owl does not move");
    assert_eq!(
        far.interp.pos, far.pos,
        "its pose is held, not re-interpolated"
    );

    let mut mobs = Mobs::new(0);
    assert!(mobs.spawn(Mob::Owl, far_spot, 0.0));
    for _ in 0..10 {
        mobs.tick(0.05, &world, &anchors, false);
    }
    assert!(mobs.instances()[0].pos.y < far_spot.y);
}

#[test]
fn a_frozen_mob_is_still_distance_despawned() {
    let mut mob = Instance::new(Mob::Owl, WorldPos::new(500.0, 64.0, 0.0), 0.0, 1);
    let rule = Despawn {
        radius: 128.0,
        random: true,
    };
    mob.tick_frozen(WorldPos::new(0.0, 64.0, 0.0), Some(rule));
    assert!(mob.is_distance_despawned());
    mob.tick_frozen(WorldPos::new(0.0, 64.0, 0.0), None);
    assert!(
        !mob.is_distance_despawned(),
        "species without a radius persist"
    );
}

#[test]
fn a_coasting_mob_keeps_moving_on_its_last_decision() {
    let world = ServerWorld::new(0, 1);
    let mut mobs = Mobs::new(0);
    mobs.set_sim_distance(SimDistance {
        full_chunks: 0,
        reduced_chunks: 100,
        reduced_interval: 1000,
    });
    let spot = WorldPos::new(8.0 + 16.0 * 5.0, 64.0, 8.0);
    assert!(mobs.spawn(Mob::Owl, spot, 0.0));
    let anchors = [anchor_at(8.0, 8.0)];
    let mut last = spot.y;
    for _ in 0..5 {
        mobs.tick(0.05, &world, &anchors, false);
        let y = mobs.instances()[0].pos.y;
        assert!(y < last, "physics runs on coasting ticks too");
        last = y;
    }
}
