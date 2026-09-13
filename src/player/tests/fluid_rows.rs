use super::*;
use crate::entity::fluid_fixture::{
    self, block, flowing_brine, pool, set_flow, BRINE, FLOOR_Y, SYRUP,
};
use petramond_math::world_pos::WorldPos;

#[test]
fn fluid_rows_drive_the_player_body() {
    let root = fluid_fixture::stage("player-fluids");
    crate::modding::tests::run_child_test(
        &root,
        "player::tests::fluid_rows::player_fluid_rows_inner",
    );
}

#[test]
#[ignore = "child of fluid_rows_drive_the_player_body with fixture content"]
fn player_fluid_rows_inner() {
    wet_locomotion_ignores_sprint_at_every_step_rate();
    entry_brakes_a_fast_fall();
    immersion_follows_the_probe_and_the_real_flow_height();
    a_current_carries_an_idle_swimmer();
}

fn wet_locomotion_ignores_sprint_at_every_step_rate() {
    for name in [BRINE, SYRUP] {
        let world = pool(block(name), 75);
        for hz in [20, 60, 144] {
            for jump in [false, true] {
                let swim = |sprint| {
                    let start = WorldPos::new(5.5, 70.0, 8.5);
                    let mut player = p(start);
                    let input = Input {
                        wishdir: Vec3::X,
                        jump,
                        sprint,
                        ..Default::default()
                    };
                    for _ in 0..hz * 2 {
                        player.update(1.0 / hz as f32, &world, input);
                    }
                    player.pos - start
                };
                let walked = swim(false);
                assert!(walked.x > 0.0, "{name}: the swimmer steers");
                assert_eq!(walked.y > 0.0, jump, "{name}: jump rises, idle sinks");
                assert!(
                    walked.distance(swim(true)) < 1e-4,
                    "{name}: sprint does not change wet locomotion"
                );
            }
        }
    }
}

fn entry_brakes_a_fast_fall() {
    for name in [BRINE, SYRUP] {
        let world = pool(block(name), 75);
        for hz in [20, 60, 144] {
            let mut player = p(WorldPos::new(5.5, 79.0, 8.5));
            player.vel = Vec3::new(SPRINT, -super::super::movement::TERMINAL, 0.0);
            for _ in 0..hz {
                player.update(1.0 / hz as f32, &world, Input::default());
            }
            assert!(
                player.pos.y > (FLOOR_Y + 2) as f64,
                "{name}: entry catches the fall: {:?}",
                player.pos
            );
            assert!(
                player.vel.x.abs() < SWIM_SPEED,
                "{name}: entry brakes momentum"
            );
            assert!(!player.on_ground);
            assert_eq!(player.take_fall_distance(), 0.0);
        }
    }
}

fn immersion_follows_the_probe_and_the_real_flow_height() {
    let feet = WorldPos::new(8.5, FLOOR_Y as f64, 8.5);
    for (name, probe_reaches_a_thin_flow) in [(SYRUP, true), (BRINE, false)] {
        let fluid = block(name);
        let mut world = pool(Block::Air, FLOOR_Y - 1);
        set_flow(&mut world, 8, 8, fluid, 6);
        assert_eq!(
            world.body_fluid(feet, HEIGHT, Buoyancy::Swim).is_some(),
            probe_reaches_a_thin_flow,
            "{name}: a thin flow immerses exactly the probes below its surface"
        );
        assert!(
            world
                .body_fluid(feet + Vec3::Y * 0.5, HEIGHT, Buoyancy::Swim)
                .is_none(),
            "{name}: air above a thin flow has no drag"
        );
        if probe_reaches_a_thin_flow {
            let wade = Input {
                wishdir: Vec3::X,
                sprint: true,
                ..Default::default()
            };
            let mut player = p(feet);
            for _ in 0..5 {
                player.update(0.05, &world, wade);
            }
            let wet_speed = SWIM_SPEED * fluid.fluid_def().unwrap().motion.speed_scale;
            assert!(player.on_ground, "{name}: wading still stands on the floor");
            assert!(
                player.vel.x > 0.0 && player.vel.x <= wet_speed + 1e-4,
                "{name}: wading takes the fluid's locomotion, not the land sprint"
            );
        }
        set_flow(&mut world, 8, 8, fluid, crate::world::fluid::FALLING);
        assert!(
            world
                .body_fluid(feet + Vec3::Y * 0.1, HEIGHT, Buoyancy::Swim)
                .is_some(),
            "{name}: a falling stream fills its cell"
        );
    }
}

fn a_current_carries_an_idle_swimmer() {
    let world = flowing_brine();
    let start = WorldPos::new(7.5, FLOOR_Y as f64, 8.5);
    let mut player = p(start);
    for _ in 0..20 {
        player.update(0.05, &world, Input::default());
    }
    assert!(
        player.pos.x > start.x + 0.1,
        "the current carries the body downstream: {:?}",
        player.pos
    );
}
