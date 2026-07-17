use super::*;

/// The ladder-test fixture: a floor top at y=64, a solid wall filling x>=2, and
/// west-facing ladders (panel toward -X, hanging on that wall) covering the
/// whole wall — every cell (1, 64..=70, *) — so strafing along the wall stays
/// on the ladder. The player stands in front of the wall and walks +X into the
/// ladder's face.
mod ladder_fixture {
    use crate::facing::Facing;

    pub fn solid(x: i32, y: i32, _z: i32) -> bool {
        y < 64 || (x >= 2 && (64..=70).contains(&y))
    }

    pub fn ladder(x: i32, y: i32, _z: i32) -> Option<Facing> {
        (x == 1 && (64..=70).contains(&y)).then_some(Facing::West)
    }
}

#[test]
fn walking_into_a_ladder_climbs_at_the_climb_speed_sprint_or_not() {
    use ladder_fixture::{ladder, solid};
    let climb_run = |sprint: bool| {
        let input = Input {
            wishdir: Vec3::new(1.0, 0.0, 0.0), // toward the wall the ladder hangs on
            jump: false,
            sprint,
            sneak: false,
        };
        let mut pl = p(Vec3::new(1.5, 64.0, 0.5));
        pl.on_ground = true;
        for _ in 0..120 {
            pl.update_core_climb(1.0 / 60.0, &solid, &dry, &ladder, input);
        }
        pl
    };
    let walked = climb_run(false);
    assert!(
        walked.pos.y > 65.5,
        "two seconds of walking into the ladder climbs it: y={}",
        walked.pos.y
    );
    assert!(
        (walked.vel.y - CLIMB_SPEED).abs() < 1e-3,
        "steady climb runs at exactly the climb speed, got {}",
        walked.vel.y
    );
    // Sprinting must not climb any faster (the climb speed derives from base WALK).
    let sprinted = climb_run(true);
    assert!(
        (sprinted.pos.y - walked.pos.y).abs() < 1e-3,
        "sprint climbs no faster: {} vs {}",
        sprinted.pos.y,
        walked.pos.y
    );
}

#[test]
fn jump_on_a_ladder_climbs_instead_of_jumping() {
    use ladder_fixture::{ladder, solid};
    let input = Input {
        wishdir: Vec3::ZERO,
        jump: true,
        sprint: false,
        sneak: false,
    };
    let mut pl = p(Vec3::new(1.5, 64.0, 0.5));
    pl.on_ground = true;
    let mut max_vy = f32::NEG_INFINITY;
    for _ in 0..120 {
        pl.update_core_climb(1.0 / 60.0, &solid, &dry, &ladder, input);
        max_vy = max_vy.max(pl.vel.y);
    }
    assert!(
        pl.pos.y > 65.5,
        "holding jump on the ladder climbs it: y={}",
        pl.pos.y
    );
    assert!(
        max_vy < CLIMB_SPEED + 1e-3,
        "no jump impulse fires on a ladder — vertical speed stays at the climb speed, saw {max_vy}"
    );
}

#[test]
fn sideways_movement_on_a_ladder_is_halved_and_grips() {
    use ladder_fixture::{ladder, solid};
    // Climb (walking into the wall) while strafing along it: lateral speed must
    // cap at the halved climb-lateral speed, never full walk.
    let strafe_climb = Input {
        wishdir: Vec3::new(1.0, 0.0, 1.0).normalize(),
        jump: false,
        sprint: false,
        sneak: false,
    };
    let mut pl = p(Vec3::new(1.5, 64.0, 0.5));
    pl.on_ground = true;
    let mut max_lateral = 0.0f32;
    for _ in 0..120 {
        pl.update_core_climb(1.0 / 60.0, &solid, &dry, &ladder, strafe_climb);
        max_lateral = max_lateral.max(pl.vel.z.abs());
    }
    assert!(
        max_lateral <= CLIMB_LATERAL_SPEED * strafe_climb.wishdir.z + 1e-3,
        "lateral speed on the ladder caps at the halved speed, saw {max_lateral}"
    );
    assert!(pl.pos.y > 65.0, "still climbing while strafing");

    // Release input mid-climb: the grip stops sideways drift almost at once
    // (well inside a quarter second), instead of the airy coast.
    for _ in 0..15 {
        pl.update_core_climb(1.0 / 60.0, &solid, &dry, &ladder, Input::default());
    }
    assert!(
        pl.vel.z.abs() < 0.2,
        "releasing input on a ladder brakes sideways drift, still moving at {}",
        pl.vel.z
    );
}

#[test]
fn a_ladder_catches_a_fall_and_lowers_it_gently() {
    use ladder_fixture::{ladder, solid};
    // Free-fall from above the ladder column: the grab clamps the descent to the
    // climb speed and the landing measures no meaningful fall.
    let mut pl = p(Vec3::new(1.5, 74.0, 0.5));
    let mut min_vy_on_ladder = f32::INFINITY;
    for _ in 0..600 {
        // The grab clamps on frames that START on the ladder (the probe runs
        // before the vertical step), so measure those frames' resulting speed.
        let on_ladder = ladder(1, pl.pos.y.floor() as i32, 0).is_some();
        pl.update_core_climb(1.0 / 60.0, &solid, &dry, &ladder, Input::default());
        if on_ladder {
            min_vy_on_ladder = min_vy_on_ladder.min(pl.vel.y);
        }
    }
    assert!(pl.on_ground, "reached the floor");
    assert!(
        min_vy_on_ladder >= -CLIMB_SPEED - 1e-3,
        "descent on the ladder is clamped to the climb speed, saw {min_vy_on_ladder}"
    );
    assert!(
        pl.take_fall_distance() < 1.0,
        "a ladder breaks the fall like water"
    );
}
