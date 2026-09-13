use super::*;
use crate::entity::fluid_fixture::{self, block, BRINE, CINDER, SYRUP};
use crate::mob::nav::{self, NavObstacles, Navigator};
use crate::world::World;
use petramond_world::chunk::{Chunk, ChunkPos};

#[test]
fn navigation_hazards_follow_the_hazard_rows() {
    let root = fluid_fixture::stage("nav-hazards");
    crate::modding::tests::run_child_test(&root, "mob::nav::hazards::tests::hazards_inner");
}

#[test]
#[ignore = "child of navigation_hazards_follow_the_hazard_rows with fixture content"]
fn hazards_inner() {
    routes_detour_around_pools_shallow_flows_and_body_height_hazards();
    hazards_block_a_crossing_that_a_safe_fluid_allows();
    a_short_drop_into_a_hazard_is_rejected();
    a_bridge_over_a_hazard_remains_walkable();
    a_mob_already_in_a_hazard_can_route_to_the_shore();
    an_escape_takes_the_shortest_way_out();
    a_partial_block_bridge_does_not_trip_the_live_guard();
    a_live_hazard_stops_a_stale_route_and_forces_a_repath();
    crowd_veer_and_jump_intents_cannot_bypass_live_hazards();
    the_live_guard_allows_an_immersed_body_to_escape();
    a_persistent_live_refusal_repaths_once_then_waits_for_the_interval();
    alternating_refusals_wait_for_the_interval_too();
    every_segment_of_a_long_body_is_guarded();
    a_wide_body_beside_a_hazard_gets_no_escape_exemption();
}

fn syrup() -> Block {
    block(SYRUP)
}

/// The fixture's humanoid body: every route below is planned for it.
fn body() -> MobSize {
    crate::mob::def(crate::mob::by_key("bodyfluid:swim").unwrap()).size
}

fn world() -> World {
    let mut chunk = Chunk::new(0, 0);
    for x in 0..16 {
        for z in 0..16 {
            chunk.set_block(x, 63, z, Block::Stone);
        }
    }
    let mut world = World::new(0, 1);
    world.insert_chunk_for_test(ChunkPos::new(0, 0), chunk);
    world
}

fn route(world: &World, start: IVec3, goal: IVec3, half: f32, height: f32) -> Navigator {
    let mut nav = Navigator::new(height.ceil() as i32, half, height);
    nav.update_goal_when_supported(Some(goal), start, world, true, &NavObstacles::none());
    nav
}

fn routes_detour_around_pools_shallow_flows_and_body_height_hazards() {
    let start = IVec3::new(2, 64, 8);
    let goal = IVec3::new(13, 64, 8);
    for y in 63..=65 {
        let mut world = world();
        for z in 6..=10 {
            world.set_block_world(7, y, z, syrup());
            let meta = match y {
                63 => 0,
                64 => 7,
                _ => petramond_world::fluid_math::FALLING,
            };
            world
                .section_at_world_mut_for_test(7, y, z)
                .unwrap()
                .set_fluid(7, y.rem_euclid(16) as usize, z as usize, syrup(), meta);
        }
        let nav = route(&world, start, goal, 0.3, 1.8);
        assert_eq!(nav.path().last(), Some(&goal), "detour exists at y={y}");
        assert!(nav
            .path()
            .iter()
            .all(|c| c.x != 7 || !(6..=10).contains(&c.z)));
        assert_eq!(
            nav::destination_reachable(&world, start, goal, nav.params, 1.8, None),
            Some(true)
        );
    }
}

fn hazards_block_a_crossing_that_a_safe_fluid_allows() {
    let start = IVec3::new(2, 64, 8);
    let goal = IVec3::new(13, 64, 8);
    for (name, allowed) in [(BRINE, true), (SYRUP, false), (CINDER, false)] {
        let mut world = world();
        for x in 6..=9 {
            for z in 0..16 {
                world.set_block_world(x, 63, z, block(name));
            }
        }
        let nav = route(&world, start, goal, 0.3, 1.8);
        assert_eq!(nav.path().last() == Some(&goal), allowed, "{name}");
        assert_eq!(
            nav::destination_reachable(&world, start, goal, nav.params, 1.8, None),
            Some(allowed),
            "{name}"
        );
        if !allowed {
            assert!(nav.path().iter().all(|c| c.x < 6), "{name}");
        }
    }
}

fn a_short_drop_into_a_hazard_is_rejected() {
    let mut world = world();
    for x in 0..16 {
        for z in 0..16 {
            if x < 6 {
                for y in 64..=66 {
                    world.set_block_world(x, y, z, Block::Stone);
                }
            } else {
                world.set_block_world(x, 63, z, syrup());
            }
        }
    }
    let nav = route(&world, IVec3::new(3, 67, 8), IVec3::new(9, 64, 8), 0.3, 1.8);
    assert!(!nav.path().is_empty());
    assert!(nav.path().iter().all(|c| c.x < 6));
}

fn a_bridge_over_a_hazard_remains_walkable() {
    let mut world = world();
    for x in 6..=9 {
        for z in 0..16 {
            world.set_block_world(x, 62, z, syrup());
            if z != 8 {
                world.set_block_world(x, 63, z, syrup());
            }
        }
    }
    let start = IVec3::new(2, 64, 8);
    let goal = IVec3::new(13, 64, 8);
    let narrow = route(&world, start, goal, 0.3, 1.8);
    assert_eq!(narrow.path().last(), Some(&goal));
    assert!(narrow.path().iter().all(|c| c.z == 8));
    let wide = route(&world, start, goal, 0.7, 1.8);
    assert_ne!(
        wide.path().last(),
        Some(&goal),
        "body edges must fit the bridge"
    );
}

fn a_mob_already_in_a_hazard_can_route_to_the_shore() {
    let mut world = world();
    for x in 4..=9 {
        for z in 0..16 {
            world.set_block_world(x, 63, z, syrup());
        }
    }
    let start = IVec3::new(6, 64, 8);
    let goal = IVec3::new(13, 64, 8);
    let nav = route(&world, start, goal, 0.3, 1.8);
    assert_eq!(nav.path().last(), Some(&goal));
    assert_eq!(
        nav::destination_reachable(&world, start, goal, nav.params, 1.8, None),
        Some(true)
    );
}

/// A body in a pool beside its near bank, with the goal across the pool: the
/// route leaves by the near bank and walks around, instead of swimming the
/// straight line through the hazard.
fn an_escape_takes_the_shortest_way_out() {
    let mut world = world();
    let in_pool = |c: &IVec3| (3..=12).contains(&c.x) && (2..=13).contains(&c.z);
    for x in 3..=12 {
        for z in 2..=13 {
            world.set_block_world(x, 63, z, syrup());
        }
    }
    let start = IVec3::new(8, 64, 3);
    let goal = IVec3::new(13, 64, 8);
    let nav = route(&world, start, goal, 0.3, 1.8);
    assert_eq!(nav.path().last(), Some(&goal));
    let crossed = nav.path().iter().filter(|c| in_pool(c)).count();
    assert!(
        crossed <= 2,
        "the route stays in the pool only to reach the nearest bank: {:?}",
        nav.path()
    );
}

fn a_partial_block_bridge_does_not_trip_the_live_guard() {
    let mut world = world();
    for x in 2..=13 {
        world.set_block_world(x, 63, 8, syrup());
        world.set_block_world(x, 64, 8, Block::OakSlab);
    }
    let start = IVec3::new(5, 65, 8);
    let goal = IVec3::new(13, 65, 8);
    let mut nav = route(&world, start, goal, 0.3, 1.8);
    assert_eq!(nav.path().last(), Some(&goal));
    let cursor = world.cursor();
    let floor_y = 64.0 + super::super::floor_top(&cursor, start - IVec3::Y);
    let pos = WorldPos::new(5.5, f64::from(floor_y), 8.5);
    assert_eq!(
        nav.avoid_hazards(pos, 0.0, body(), Vec3::X, false, 0.2, &cursor),
        (Vec3::X, false)
    );
}

fn a_live_hazard_stops_a_stale_route_and_forces_a_repath() {
    let mut world = world();
    let start = IVec3::new(5, 64, 8);
    let goal = IVec3::new(13, 64, 8);
    let mut nav = route(&world, start, goal, 0.3, 1.8);
    let pos = WorldPos::new(5.5, 64.0, 8.5);
    let (wish, jump) = nav.follow_steered(pos, true, &world);
    world.set_block_world(6, 63, 8, syrup());
    assert_eq!(
        nav.avoid_hazards(pos, 0.0, body(), wish, jump, 0.2, &world.cursor()),
        (Vec3::ZERO, false),
        "stop before falling below the hazard's surface"
    );
    let recomputes = nav.recomputes();
    nav.update_goal_when_supported(Some(goal), start, &world, true, &NavObstacles::none());
    assert_eq!(nav.recomputes(), recomputes + 1);
    assert_eq!(nav.path().last(), Some(&goal));
    assert!(!nav.path().contains(&IVec3::new(6, 64, 8)));
}

fn crowd_veer_and_jump_intents_cannot_bypass_live_hazards() {
    let mut world = world();
    let start = IVec3::new(5, 64, 8);
    let goal = IVec3::new(13, 64, 8);
    let mut nav = route(&world, start, goal, 0.3, 1.8);
    let pos = WorldPos::new(5.5, 64.0, 8.5);
    world.set_block_world(5, 64, 9, syrup());
    assert_eq!(
        nav.avoid_hazards(pos, 0.0, body(), Vec3::X, false, 0.2, &world.cursor()),
        (Vec3::X, false)
    );
    let veered = Vec3::new(0.5, 0.0, 0.8660254);
    for jump in [false, true] {
        assert_eq!(
            nav.avoid_hazards(pos, 0.0, body(), veered, jump, 0.2, &world.cursor()),
            (Vec3::ZERO, false)
        );
    }
    nav.path = vec![start, start + IVec3::new(1, 1, 0)];
    nav.index = 1;
    world.set_block_world(5, 66, 8, syrup());
    assert_eq!(
        nav.avoid_hazards(pos, 0.0, body(), Vec3::X, true, 0.2, &world.cursor()),
        (Vec3::ZERO, false),
        "a jump cannot enter a hazard above the current body"
    );
}

fn the_live_guard_allows_an_immersed_body_to_escape() {
    let mut world = world();
    for x in 4..=9 {
        world.set_block_world(x, 63, 8, syrup());
    }
    let mut nav = route(
        &world,
        IVec3::new(5, 64, 8),
        IVec3::new(13, 64, 8),
        0.3,
        1.8,
    );
    let pos = WorldPos::new(5.5, 63.5, 8.5);
    assert_eq!(
        nav.avoid_hazards(pos, 0.0, body(), Vec3::X, false, 0.2, &world.cursor()),
        (Vec3::X, false)
    );
}

fn a_persistent_live_refusal_repaths_once_then_waits_for_the_interval() {
    let mut world = world();
    let start = IVec3::new(5, 64, 8);
    let goal = IVec3::new(13, 64, 8);
    let mut nav = route(&world, start, goal, 0.3, 1.8);
    let pos = WorldPos::new(5.5, 64.0, 8.5);
    world.set_block_world(5, 64, 9, syrup());
    let veered = Vec3::new(0.5, 0.0, 0.8660254);
    let before = nav.recomputes();
    let ticks = 200;
    for tick in 0..ticks {
        assert_eq!(
            nav.avoid_hazards(pos, 0.0, body(), veered, false, 0.2, &world.cursor()),
            (Vec3::ZERO, false)
        );
        nav.update_goal_when_supported(Some(goal), start, &world, true, &NavObstacles::none());
        if tick == 0 {
            assert_eq!(
                nav.recomputes(),
                before + 1,
                "the first refusal repaths at once"
            );
        }
    }
    assert!(
        nav.recomputes() - before <= 1 + ticks / super::super::REPATH_TICKS,
        "a refusal the planner cannot see must not search every tick: {}",
        nav.recomputes() - before
    );
}

/// Routes that keep leading into two different refused waypoints in turn
/// must back off exactly like one refused waypoint.
fn alternating_refusals_wait_for_the_interval_too() {
    let mut world = world();
    let start = IVec3::new(5, 64, 8);
    let goal = IVec3::new(13, 64, 8);
    let mut nav = route(&world, start, goal, 0.3, 1.8);
    world.set_block_world(6, 63, 8, syrup());
    world.set_block_world(5, 63, 9, syrup());
    let pos = WorldPos::new(5.5, 64.0, 8.5);
    let refused = [
        (IVec3::new(6, 64, 8), Vec3::X),
        (IVec3::new(5, 64, 9), Vec3::Z),
    ];
    let before = nav.recomputes();
    let ticks = 200;
    for tick in 0..ticks {
        let (waypoint, wish) = refused[tick as usize % refused.len()];
        nav.path = vec![start, waypoint];
        nav.index = 1;
        assert_eq!(
            nav.avoid_hazards(pos, 0.0, body(), wish, false, 0.2, &world.cursor()),
            (Vec3::ZERO, false)
        );
        nav.update_goal_when_supported(Some(goal), start, &world, true, &NavObstacles::none());
    }
    assert!(
        nav.recomputes() - before <= 1 + ticks / super::super::REPATH_TICKS,
        "alternating refusals must not search every tick: {}",
        nav.recomputes() - before
    );
}

/// A long body lying along X steps sideways: only its end segment sweeps
/// over the hazard, which a check of the centre box alone would miss.
fn every_segment_of_a_long_body_is_guarded() {
    let mut world = world();
    world.set_block_world(9, 63, 9, syrup());
    let start = IVec3::new(8, 64, 8);
    let mut nav = route(&world, start, IVec3::new(8, 64, 12), 0.3, 1.0);
    nav.path = vec![start, IVec3::new(8, 64, 9)];
    nav.index = 1;
    let pos = WorldPos::new(8.5, 64.0, 8.5);
    let along_x = std::f32::consts::FRAC_PI_2;
    let short = MobSize {
        half_width: 0.3,
        height: 1.0,
        half_length: None,
    };
    let long = MobSize {
        half_length: Some(1.5),
        ..short
    };
    let cursor = world.cursor();
    assert_eq!(
        nav.avoid_hazards(pos, along_x, short, Vec3::Z, false, 0.2, &cursor),
        (Vec3::Z, false)
    );
    assert_eq!(
        nav.avoid_hazards(pos, along_x, long, Vec3::Z, false, 0.2, &cursor),
        (Vec3::ZERO, false)
    );
}

fn a_wide_body_beside_a_hazard_gets_no_escape_exemption() {
    let mut world = world();
    for x in 6..=9 {
        for z in 0..16 {
            world.set_block_world(x, 63, z, syrup());
        }
    }
    let start = IVec3::new(5, 64, 8);
    let goal = IVec3::new(13, 64, 8);
    let nav = route(&world, start, goal, 0.7, 1.8);
    assert_ne!(
        nav.path().last(),
        Some(&goal),
        "overhanging a pool it does not touch must not open a crossing"
    );
    assert_eq!(
        nav::destination_reachable(&world, start, goal, nav.params, 1.8, None),
        Some(false)
    );
}
