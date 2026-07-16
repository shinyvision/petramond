use super::{
    collision::Axis,
    movement::{
        friction_retain, AIR_ACCEL, AIR_FRICTION, CLIMB_LATERAL_SPEED, CLIMB_SPEED,
        FRICTION_REF_DT, GRAVITY, GROUND_ACCEL, GROUND_FRICTION, SPECTATOR_SPEED, SPRINT,
        SWIM_CLIMB, WALK, WATER_PROBE_Y,
    },
    *,
};
use crate::block::Block;
use crate::mathh::{IVec3, SelectionShape, Vec3};

/// No water anywhere -- the dry-land predicate every physics test uses.
fn dry(_x: i32, _y: i32, _z: i32) -> bool {
    false
}

/// No ladders anywhere -- the climb predicate for tests off the ladder.
fn no_ladder(_x: i32, _y: i32, _z: i32) -> Option<crate::facing::Facing> {
    None
}

fn p(feet: Vec3) -> Player {
    Player::new(feet)
}

#[test]
fn falls_and_lands_on_floor() {
    // Solid everywhere y < 64 (a thick floor), air above.
    let solid = |_x: i32, y: i32, _z: i32| y < 64;
    let mut pl = p(Vec3::new(0.0, 70.0, 0.0));
    // Large downward sweep: must clamp feet to the top of cell 63 (y=64).
    let blocked = pl.sweep(Axis::Y, -20.0, &solid);
    assert!(blocked);
    assert_eq!(pl.pos.y, 64.0);
}

#[test]
fn does_not_tunnel_through_one_block_floor() {
    // Only y == 0 is solid (a 1-block-thick platform).
    let solid = |_x: i32, y: i32, _z: i32| y == 0;
    let mut pl = p(Vec3::new(0.0, 5.0, 0.0));
    let blocked = pl.sweep(Axis::Y, -20.0, &solid);
    assert!(blocked, "must not fall through a 1-thick floor");
    assert_eq!(pl.pos.y, 1.0, "feet rest on top of cell 0");
}

#[test]
fn stops_at_wall_moving_positive_x() {
    // Wall at x >= 5.
    let solid = |x: i32, _y: i32, _z: i32| x >= 5;
    let mut pl = p(Vec3::new(4.0, 64.0, 0.0)); // max.x = 4.3
    let blocked = pl.sweep(Axis::X, 2.0, &solid);
    assert!(blocked);
    // max.x clamped to 5.0 => centre at 4.7.
    assert!((pl.pos.x - 4.7).abs() < 1e-5, "pos.x = {}", pl.pos.x);
}

#[test]
fn stops_at_wall_moving_negative_x() {
    // Wall at x <= 1 (cells 1 and below solid).
    let solid = |x: i32, _y: i32, _z: i32| x <= 1;
    let mut pl = p(Vec3::new(4.0, 64.0, 0.0)); // min.x = 3.7
    let blocked = pl.sweep(Axis::X, -3.0, &solid);
    assert!(blocked);
    // min.x clamped to 2.0 (top of cell 1) => centre at 2.3.
    assert!((pl.pos.x - 2.3).abs() < 1e-5, "pos.x = {}", pl.pos.x);
}

#[test]
fn moves_freely_in_open_air() {
    let solid = |_x: i32, _y: i32, _z: i32| false;
    let mut pl = p(Vec3::new(0.0, 64.0, 0.0));
    assert!(!pl.sweep(Axis::Z, 3.0, &solid));
    assert_eq!(pl.pos.z, 3.0);
}

#[test]
fn grounded_player_auto_steps_up_a_half_block_but_not_a_full_one() {
    use crate::block::Aabb;
    let still = |_: Vec3| Vec3::ZERO;
    let walk_x = Input {
        wishdir: Vec3::new(1.0, 0.0, 0.0),
        jump: false,
        sprint: false,
        sneak: false,
    };

    // Floor at y=0 (full cubes) + a 0.5-tall ledge filling cells x>=1 at y=1 (world y∈[1,1.5]).
    let half_step = |x: i32, y: i32, _z: i32| -> &'static [Aabb] {
        if y == 0 {
            Block::Stone.collision_boxes()
        } else if y == 1 && x >= 1 {
            &[Aabb {
                min: [0.0, 0.0, 0.0],
                max: [1.0, 0.5, 1.0],
            }]
        } else {
            &[]
        }
    };
    let mut pl = p(Vec3::new(0.5, 1.0, 0.5)); // feet on the floor top, walking +X into the ledge
    for _ in 0..180 {
        pl.update_core_with_current(
            1.0 / 60.0,
            &half_step,
            &dry,
            &still,
            &no_ladder,
            walk_x,
            &[],
        );
    }
    assert!(
        pl.pos.x > 1.2,
        "grounded player steps onto the ledge: x={}",
        pl.pos.x
    );
    assert!(
        pl.pos.y > 1.4,
        "and rises onto the 0.5 ledge top (y≈1.5): y={}",
        pl.pos.y
    );
    assert!(pl.on_ground, "player is grounded on the ledge");

    // A FULL block (cells x>=1 at y=1 AND y=2) is NOT climbed — it's a wall.
    let full_block = |x: i32, y: i32, _z: i32| -> &'static [Aabb] {
        if y == 0 || ((y == 1 || y == 2) && x >= 1) {
            Block::Stone.collision_boxes()
        } else {
            &[]
        }
    };
    let mut pl2 = p(Vec3::new(0.5, 1.0, 0.5));
    for _ in 0..180 {
        pl2.update_core_with_current(
            1.0 / 60.0,
            &full_block,
            &dry,
            &still,
            &no_ladder,
            walk_x,
            &[],
        );
    }
    assert!(
        pl2.pos.y < 1.1,
        "player does NOT climb a full block: y={}",
        pl2.pos.y
    );
    assert!(
        pl2.pos.x < 1.0,
        "player is stopped by the full block: x={}",
        pl2.pos.x
    );
}

#[test]
fn fall_distance_measures_the_drop_height() {
    // Thick floor with its top at y=64; drop from feet y=68 is a clean 4-block fall.
    let solid = |_x: i32, y: i32, _z: i32| y < 64;
    let mut pl = p(Vec3::new(0.5, 68.0, 0.5));
    for _ in 0..240 {
        pl.update_core(1.0 / 60.0, &solid, &dry, Input::default());
    }
    assert!(pl.on_ground, "player has landed");
    assert!((pl.pos.y - 64.0).abs() < 1e-3, "feet on the floor top");
    let dist = pl.take_fall_distance();
    assert!(
        (dist - 4.0).abs() < 0.05,
        "a 4-block drop measures ~4 blocks, got {dist}"
    );
    // Draining it leaves nothing for the next tick (a landing is counted once).
    assert_eq!(pl.take_fall_distance(), 0.0);
}

#[test]
fn walking_off_a_low_ledge_measures_a_short_fall() {
    // Feet start on a floor top at y=64, walking +X off into open air with the floor
    // dropping to y=62 (a 2-block step-down) — under the 3-block safe distance.
    let solid = |x: i32, y: i32, _z: i32| if x <= 0 { y < 64 } else { y < 62 };
    let walk = Input {
        wishdir: Vec3::new(1.0, 0.0, 0.0),
        jump: false,
        sprint: false,
        sneak: false,
    };
    let mut pl = p(Vec3::new(0.5, 64.0, 0.5));
    for _ in 0..240 {
        pl.update_core(1.0 / 60.0, &solid, &dry, walk);
    }
    assert!(pl.on_ground);
    let dist = pl.take_fall_distance();
    assert!(
        dist < 3.0,
        "a 2-block step-down stays under the safe distance, got {dist}"
    );
}

#[test]
fn water_cancels_the_fall() {
    // Deep water column (y in 60..=71) over a floor top at y=60. A 10-block plunge that
    // would badly hurt on land measures ~0 because water breaks the fall.
    let solid = |_x: i32, y: i32, _z: i32| y < 60;
    let water = |_x: i32, y: i32, _z: i32| (60..=71).contains(&y);
    let mut pl = p(Vec3::new(0.5, 70.0, 0.5));
    for _ in 0..1200 {
        pl.update_core(1.0 / 60.0, &solid, &water, Input::default());
    }
    let dist = pl.take_fall_distance();
    assert!(
        dist < 3.0,
        "water should break the fall (no damage), measured {dist}"
    );
}

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

#[test]
fn mode_switch_drops_a_pending_fall() {
    // A pending fall must not survive a spectator toggle (you were flying, not falling).
    let solid = |_x: i32, y: i32, _z: i32| y < 64;
    let mut pl = p(Vec3::new(0.5, 70.0, 0.5));
    for _ in 0..240 {
        pl.update_core(1.0 / 60.0, &solid, &dry, Input::default());
    }
    pl.toggle_mode(); // -> spectator, re-anchors the fall
    assert_eq!(
        pl.take_fall_distance(),
        0.0,
        "mode switch clears the landing"
    );
}

#[test]
fn health_damage_and_restore_clamp_to_the_valid_range() {
    let mut pl = p(Vec3::new(0.0, 64.0, 0.0));
    assert_eq!(pl.health(), MAX_HEALTH, "starts at full health");
    assert!(pl.apply_damage(3));
    assert_eq!(pl.health(), MAX_HEALTH - 3);
    assert!(!pl.apply_damage(0)); // non-positive is a no-op
    assert_eq!(pl.health(), MAX_HEALTH - 3);
    assert!(
        !pl.apply_damage(1000),
        "the active i-frame window rejects damage"
    );
    for _ in 0..crate::damage::PLAYER_DAMAGE_IFRAME_TICKS {
        pl.tick_damage_immunity();
    }
    assert!(pl.apply_damage(1000)); // never below zero
    assert_eq!(pl.health(), 0);
    pl.set_health(1000); // restore clamps to the max
    assert_eq!(pl.health(), MAX_HEALTH);
    pl.set_health(-5);
    assert_eq!(pl.health(), 0);
}

#[test]
fn status_effects_fire_on_interval_boundaries_and_expire() {
    use crate::effect::{Effect, EffectBehavior};
    // Derive the cadence from the loaded row — the contract under test is the
    // boundary/expiry behavior, never the freely-editable interval/amount.
    let EffectBehavior::Regen { interval, .. } = Effect::Regeneration.def().behavior else {
        panic!("regeneration is an interval-heal behavior");
    };

    // The player owns WHEN a behavior fires (Game applies the consequences,
    // so damage can route through its funnel): boundaries land every
    // `interval` ticks, including one at expiry.
    let mut pl = p(Vec3::new(0.0, 64.0, 0.0));
    pl.apply_effect(Effect::Regeneration, interval * 2);
    let mut fired = 0;
    for _ in 0..interval {
        fired += pl.tick_effects().len();
    }
    assert_eq!(fired, 1, "the first boundary fires exactly once");
    for _ in 0..interval {
        fired += pl.tick_effects().len();
    }
    assert_eq!(fired, 2, "the expiry tick is itself a boundary");
    assert!(pl.effects().is_empty(), "the effect expired");

    // Re-applying overwrites the duration (in place); zero removes.
    pl.apply_effect(Effect::Regeneration, 10);
    pl.apply_effect(Effect::Regeneration, interval * 5);
    assert_eq!(pl.effects()[0].remaining, interval * 5);
    pl.apply_effect(Effect::Regeneration, 0);
    assert!(pl.effects().is_empty(), "zero ticks removes the effect");

    // The heal primitive the regen consequence lands through clamps at full
    // and never resurrects — respawn owns that transition.
    pl.set_health(MAX_HEALTH);
    pl.heal(5);
    assert_eq!(pl.health(), MAX_HEALTH, "healing clamps at full");
    pl.set_health(0);
    pl.heal(5);
    assert_eq!(pl.health(), 0, "healing never resurrects");
}

#[test]
fn spectator_clips_through_solids_and_flies_in_3d() {
    let solid_wall_and_ceiling = |x: i32, y: i32, _z: i32| x >= 1 || y >= 65;
    let mut pl = p(Vec3::new(0.0, 64.0, 0.0));
    pl.set_mode(PlayerMode::Spectator);

    let input = Input {
        wishdir: Vec3::new(1.0, 1.0, 0.0).normalize(),
        jump: false,
        sprint: false,
        sneak: false,
    };
    pl.update_core(1.0, &solid_wall_and_ceiling, &dry, input);

    assert_eq!(pl.mode(), PlayerMode::Spectator);
    assert!(pl.pos.x > 1.0, "spectator should pass through wall");
    assert!(pl.pos.y > 65.0, "spectator should fly through ceiling");
    assert!(!pl.on_ground);
    assert!(
        (pl.vel.length() - SPECTATOR_SPEED).abs() < 1e-5,
        "spectator velocity should be fixed fly speed, got {}",
        pl.vel.length()
    );
}

#[test]
fn switching_modes_resets_motion_state() {
    let mut pl = p(Vec3::new(0.0, 64.0, 0.0));
    pl.vel = Vec3::new(3.0, -7.0, 1.0);
    pl.on_ground = true;
    pl.jumping = true;

    pl.toggle_mode();
    assert_eq!(pl.mode(), PlayerMode::Spectator);
    assert_eq!(pl.vel, Vec3::ZERO);
    assert!(!pl.on_ground);
    assert!(!pl.jumping);

    pl.vel = Vec3::new(0.0, 9.0, 0.0);
    pl.toggle_mode();
    assert_eq!(pl.mode(), PlayerMode::Survival);
    assert_eq!(pl.vel, Vec3::ZERO);
    assert!(!pl.on_ground);
    assert!(!pl.jumping);
}

#[test]
fn air_decays_slower_than_ground() {
    // No input: both decay gradually toward zero, but air friction is far
    // weaker than ground friction, so in a single frame the airborne body
    // sheds only a sliver of its speed while the grounded body sheds a larger
    // share — both a slide, ground just firmer.
    let dt = FRICTION_REF_DT; // at the reference frame, retain == 1 - friction
    let open = |_x: i32, _y: i32, _z: i32| false;
    let mut air = p(Vec3::new(0.0, 128.0, 0.0));
    air.vel = Vec3::new(WALK, 5.0, 0.0); // gliding +x, rising
    air.on_ground = false;
    air.update_core(dt, &open, &dry, Input::default());

    let floor = |_x: i32, y: i32, _z: i32| y < 64;
    let mut gnd = p(Vec3::new(0.0, 64.0, 0.0));
    gnd.vel = Vec3::new(WALK, 0.0, 0.0);
    gnd.on_ground = true;
    gnd.update_core(dt, &floor, &dry, Input::default());

    // Air retains 1 - AIR_FRICTION of its speed; ground retains less per frame.
    assert!(
        (air.vel.x - WALK * (1.0 - AIR_FRICTION)).abs() < 1e-5,
        "air vx = {}",
        air.vel.x
    );
    assert!(
        (gnd.vel.x - WALK * (1.0 - GROUND_FRICTION)).abs() < 1e-5,
        "gnd vx = {}",
        gnd.vel.x
    );
    assert!(
        air.vel.x > gnd.vel.x,
        "air should keep more momentum than ground"
    );
    assert!(air.vel.y < 5.0, "gravity should bleed upward speed");
}

#[test]
fn ground_accelerates_faster_than_air() {
    let input = Input {
        wishdir: Vec3::new(1.0, 0.0, 0.0),
        jump: false,
        sprint: false,
        sneak: false,
    };
    let dt = FRICTION_REF_DT;

    // On the ground from rest, one step ramps toward walk speed at the high
    // ground acceleration (GROUND_ACCEL·dt, still well below WALK so it is not
    // yet clamped) — a few frames to top speed, so the ground feels snappy.
    let floor = |_x: i32, y: i32, _z: i32| y < 64;
    let mut g = p(Vec3::new(0.0, 64.0, 0.0));
    g.on_ground = true;
    g.update_core(dt, &floor, &dry, input);
    assert!(
        (g.vel.x - GROUND_ACCEL * dt).abs() < 1e-5,
        "ground vx = {}",
        g.vel.x
    );

    // In the air from rest, the same input ramps far more slowly — gentle
    // steering, not a snap to speed.
    let open = |_x: i32, _y: i32, _z: i32| false;
    let mut a = p(Vec3::new(0.0, 128.0, 0.0));
    a.on_ground = false;
    a.update_core(dt, &open, &dry, input);
    assert!(
        (a.vel.x - AIR_ACCEL * dt).abs() < 1e-5,
        "air vx = {}",
        a.vel.x
    );
    assert!(
        g.vel.x > a.vel.x * 2.0,
        "ground acceleration much stronger than air"
    );
}

#[test]
fn air_input_does_not_brake_momentum() {
    // Airborne at sprint speed, then holding plain forward (a *slower* walk
    // wish). Air acceleration is additive — it only adds toward the wish
    // direction, never brakes — so the launched momentum is kept, not bled
    // down to walk speed. (Releasing input instead lets air friction coast it
    // down very gradually; that path is covered elsewhere.)
    let open = |_x: i32, _y: i32, _z: i32| false;
    let input = Input {
        wishdir: Vec3::new(1.0, 0.0, 0.0),
        jump: false,
        sprint: false,
        sneak: false,
    };
    let mut a = p(Vec3::new(0.0, 128.0, 0.0));
    a.on_ground = false;
    a.vel = Vec3::new(SPRINT, 0.0, 0.0); // gliding +x faster than WALK
    a.update_core(FRICTION_REF_DT, &open, &dry, input);
    assert!(
        (a.vel.x - SPRINT).abs() < 1e-5,
        "air input must not brake momentum, vx = {}",
        a.vel.x
    );
}

#[test]
fn air_steering_redirects_without_inflating_speed() {
    // Airborne moving +x at walk speed; steering +z rotates the velocity
    // toward +z at constant total speed — momentum is redirected, not pumped
    // (forward bleeds a hair as lateral is added). This speed cap is what stops
    // wall-scraping from building crazy sideways speed.
    let open = |_x: i32, _y: i32, _z: i32| false;
    let input = Input {
        wishdir: Vec3::new(0.0, 0.0, 1.0),
        jump: false,
        sprint: false,
        sneak: false,
    };
    let mut a = p(Vec3::new(0.0, 128.0, 0.0));
    a.on_ground = false;
    a.vel = Vec3::new(WALK, 0.0, 0.0);
    a.update_core(FRICTION_REF_DT, &open, &dry, input);
    let speed = (a.vel.x * a.vel.x + a.vel.z * a.vel.z).sqrt();
    assert!(
        (speed - WALK).abs() < 1e-4,
        "total speed preserved, not inflated, got {speed}"
    );
    assert!(a.vel.z > 0.0, "lateral input adds +z, vz = {}", a.vel.z);
    assert!(
        a.vel.x < WALK,
        "forward bleeds slightly as speed redirects, vx = {}",
        a.vel.x
    );
}

#[test]
fn jumping_into_wall_does_not_pump_sideways_speed() {
    // Wall just ahead in +x; hold a wish mostly *into* the wall, slightly along
    // it. The into-wall velocity is killed by the wall every step, which used to
    // let the perpendicular (+z) speed climb without bound. With the air speed
    // cap, total horizontal speed stays bounded by walk speed no matter how
    // long you scrape the wall.
    let wall_x = 6;
    let solid = move |x: i32, _y: i32, _z: i32| x >= wall_x;
    let mut a = p(Vec3::new(wall_x as f32 - 1.0, 128.0, 0.0));
    a.on_ground = false; // open below: stays airborne the whole run
    let wishdir = Vec3::new(0.98, 0.0, 0.2).normalize();
    let input = Input {
        wishdir,
        jump: false,
        sprint: false,
        sneak: false,
    };
    for _ in 0..600 {
        a.update_core(0.02, &solid, &dry, input);
    }
    let speed = (a.vel.x * a.vel.x + a.vel.z * a.vel.z).sqrt();
    assert!(
        speed <= WALK + 1e-3,
        "wall-scrape pumped speed to {speed} (cap is WALK = {WALK})"
    );
}

#[test]
fn air_out_coasts_ground() {
    // No input: both decay by friction alone. Air friction is far weaker than
    // ground friction, so after the same coast the airborne body retains
    // strictly — and, with the tuned values, far — more speed. Expectations are
    // derived from the constants, so this survives retuning either friction (it
    // only assumes the design invariant AIR_FRICTION < GROUND_FRICTION).
    let open = |_x: i32, _y: i32, _z: i32| false;
    let floor = |_x: i32, y: i32, _z: i32| y < 64;
    let mut air = p(Vec3::new(0.0, 1024.0, 0.0)); // open below: airborne the whole run
    air.on_ground = false;
    air.vel = Vec3::new(WALK, 0.0, 0.0);
    let mut gnd = p(Vec3::new(0.0, 64.0, 0.0));
    gnd.on_ground = true;
    gnd.vel = Vec3::new(WALK, 0.0, 0.0);
    let steps = 30; // ~half a second at the reference step
    for _ in 0..steps {
        air.update_core(FRICTION_REF_DT, &open, &dry, Input::default());
        gnd.update_core(FRICTION_REF_DT, &floor, &dry, Input::default());
    }
    // Pure-decay speeds implied by the friction constants (one ref step retains
    // exactly 1 - friction).
    let air_expected = WALK * (1.0 - AIR_FRICTION).powi(steps);
    let gnd_expected = WALK * (1.0 - GROUND_FRICTION).powi(steps);
    assert!(
        (air.vel.x - air_expected).abs() < 1e-3,
        "air vx = {} (want {air_expected})",
        air.vel.x
    );
    assert!(
        (gnd.vel.x - gnd_expected).abs() < 1e-3,
        "gnd vx = {} (want {gnd_expected})",
        gnd.vel.x
    );
    assert!(
        air.vel.x > gnd.vel.x,
        "air must out-coast ground: air {} vs gnd {}",
        air.vel.x,
        gnd.vel.x
    );
}

#[test]
fn friction_endpoints_hold_at_any_dt() {
    for &dt in &[0.005f32, FRICTION_REF_DT, 0.05] {
        // friction 0: nothing shed, motion continues indefinitely.
        assert_eq!(
            friction_retain(0.0, dt),
            1.0,
            "friction 0 must not decay (dt={dt})"
        );
        // friction 1: everything shed, an immediate stop.
        assert_eq!(
            friction_retain(1.0, dt),
            0.0,
            "friction 1 must snap to a stop (dt={dt})"
        );
    }
    // At the reference frame the retained fraction is exactly 1 - friction.
    assert!(
        (friction_retain(GROUND_FRICTION, FRICTION_REF_DT) - (1.0 - GROUND_FRICTION)).abs() < 1e-6
    );
    assert!((friction_retain(AIR_FRICTION, FRICTION_REF_DT) - (1.0 - AIR_FRICTION)).abs() < 1e-6);
}

#[test]
fn friction_is_framerate_independent() {
    // One big decay step must retain the same fraction as several small steps
    // spanning the same wall-clock time (the property the sub-step loop relies on).
    let total = 0.05f32;
    let one = friction_retain(GROUND_FRICTION, total);
    let n = 5;
    let many = friction_retain(GROUND_FRICTION, total / n as f32).powi(n);
    assert!(
        (one - many).abs() < 1e-6,
        "retained {one} (1 step) vs {many} ({n} steps)"
    );
}

#[test]
fn gravity_eases_near_apex() {
    let open = |_x: i32, _y: i32, _z: i32| false;
    // In a jump, inside the apex band: reduced gravity loses less speed.
    let mut near = p(Vec3::new(0.0, 128.0, 0.0));
    near.vel = Vec3::new(0.0, 1.0, 0.0);
    near.on_ground = false;
    near.jumping = true;
    near.update_core(0.05, &open, &dry, Input::default());
    let near_drop = 1.0 - near.vel.y;

    // In a jump, outside the band: full gravity.
    let mut fast = p(Vec3::new(0.0, 128.0, 0.0));
    fast.vel = Vec3::new(0.0, 20.0, 0.0);
    fast.on_ground = false;
    fast.jumping = true;
    fast.update_core(0.05, &open, &dry, Input::default());
    let fast_drop = 20.0 - fast.vel.y;

    assert!(
        near_drop < fast_drop,
        "apex should ease gravity: {near_drop} vs {fast_drop}"
    );
    assert!(
        (fast_drop - GRAVITY * 0.05).abs() < 1e-5,
        "outside band is full gravity"
    );
}

#[test]
fn no_apex_easing_when_not_jumping() {
    // Walking off a ledge / stepping down (jumping == false) must fall at
    // full gravity even though vel.y is briefly inside the apex band — the
    // easing is reserved for real jump arcs, so the world never feels floaty.
    let open = |_x: i32, _y: i32, _z: i32| false;
    let mut pl = p(Vec3::new(0.0, 128.0, 0.0));
    pl.vel = Vec3::new(0.0, 1.0, 0.0); // small downward-bound speed, no jump
    pl.on_ground = false;
    pl.jumping = false;
    pl.update_core(0.05, &open, &dry, Input::default());
    let drop = 1.0 - pl.vel.y;
    assert!(
        (drop - GRAVITY * 0.05).abs() < 1e-5,
        "not jumping → full gravity, got {drop}"
    );
}

/// Trusted, slow reference: does the player AABB centred at `pos` overlap any
/// solid cell? Shrinks the box by a symmetric tol on every side (so it is
/// direction-agnostic by construction — any asymmetry in `sweep` shows up as
/// a disagreement with this).
fn ref_overlaps<F: Fn(i32, i32, i32) -> bool>(pos: Vec3, solid: &F) -> bool {
    let t = 1e-4;
    let x0 = (pos.x - HALF_W + t).floor() as i32;
    let x1 = (pos.x + HALF_W - t).floor() as i32;
    let y0 = (pos.y + t).floor() as i32;
    let y1 = (pos.y + HEIGHT - t).floor() as i32;
    let z0 = (pos.z - HALF_W + t).floor() as i32;
    let z1 = (pos.z + HALF_W - t).floor() as i32;
    for x in x0..=x1 {
        for y in y0..=y1 {
            for z in z0..=z1 {
                if solid(x, y, z) {
                    return true;
                }
            }
        }
    }
    false
}

/// Reference separated-axis move (X then Z, like `sweep`) advancing in
/// ~0.5 mm micro-steps and stopping before the first overlap. Moves *exactly*
/// `disp` in open space (the final sub-step takes up the remainder, so there
/// is no rounding drift). Obviously correct; the slow oracle for `sweep`.
fn ref_move<F: Fn(i32, i32, i32) -> bool>(mut pos: Vec3, disp: Vec3, solid: &F) -> Vec3 {
    let step = 5e-4f32;
    for axis in [0, 1] {
        let d = if axis == 0 { disp.x } else { disp.z };
        let mut moved = 0.0f32;
        while moved < d.abs() {
            let this = step.min(d.abs() - moved) * d.signum();
            let mut next = pos;
            if axis == 0 {
                next.x += this;
            } else {
                next.z += this;
            }
            if ref_overlaps(next, solid) {
                break;
            }
            pos = next;
            moved += this.abs();
        }
    }
    pos
}

#[test]
fn sweep_matches_reference_from_all_directions() {
    let configs: [(&str, &[IVec3]); 6] = [
        ("single", &[IVec3::new(10, 64, 10)]),
        (
            "wall_x",
            &[
                IVec3::new(10, 64, 8),
                IVec3::new(10, 64, 9),
                IVec3::new(10, 64, 10),
                IVec3::new(10, 64, 11),
                IVec3::new(10, 64, 12),
            ],
        ),
        (
            "wall_z",
            &[
                IVec3::new(8, 64, 10),
                IVec3::new(9, 64, 10),
                IVec3::new(10, 64, 10),
                IVec3::new(11, 64, 10),
                IVec3::new(12, 64, 10),
            ],
        ),
        ("pillar2", &[IVec3::new(10, 64, 10), IVec3::new(10, 65, 10)]),
        ("head", &[IVec3::new(10, 65, 10)]),
        (
            "Lcorner",
            &[
                IVec3::new(10, 64, 10),
                IVec3::new(11, 64, 10),
                IVec3::new(10, 64, 11),
            ],
        ),
    ];
    let dirs: [(f32, f32, &str); 8] = [
        (1.0, 0.0, "+X"),
        (-1.0, 0.0, "-X"),
        (0.0, 1.0, "+Z"),
        (0.0, -1.0, "-Z"),
        (1.0, 1.0, "+X+Z"),
        (1.0, -1.0, "+X-Z"),
        (-1.0, 1.0, "-X+Z"),
        (-1.0, -1.0, "-X-Z"),
    ];
    // Translate the whole scene to probe positive, origin-crossing, and
    // negative coordinates (floor()/cast/>>4 behave differently around 0).
    let bases: [(i32, i32, &str); 3] = [(0, 0, "pos"), (-10, -10, "origin"), (-21, -21, "neg")];
    let mut failures = Vec::new();
    for (bx, bz, bname) in bases {
        for (cname0, cells0) in configs {
            let cells_v: Vec<IVec3> = cells0
                .iter()
                .map(|c| IVec3::new(c.x + bx, c.y, c.z + bz))
                .collect();
            let cname = format!("{bname}/{cname0}");
            let solid = {
                let cells_v = cells_v.clone();
                move |x: i32, y: i32, z: i32| {
                    y < 64 || cells_v.iter().any(|c| c.x == x && c.y == y && c.z == z)
                }
            };
            let centre = Vec3::new(10.5 + bx as f32, 64.0, 10.5 + bz as f32);
            for (dx, dz, name) in dirs {
                let len = (dx * dx + dz * dz).sqrt();
                let wishdir = Vec3::new(dx / len, 0.0, dz / len);
                let lateral = Vec3::new(-wishdir.z, 0.0, wishdir.x);
                for k in -19..=19 {
                    let off = k as f32 * 0.05;
                    let start = centre - wishdir * 3.5 + lateral * off;
                    let (dt, speed) = (0.02f32, WALK);
                    // sweep path. Start at full walk speed so the friction
                    // ramp-up doesn't lag the reference mover (which moves at
                    // exactly speed·dt from step one); this test probes the
                    // collision sweep, not the acceleration curve.
                    let mut pl = p(start);
                    pl.on_ground = true;
                    pl.vel = wishdir * WALK;
                    let input = Input {
                        wishdir,
                        jump: false,
                        sprint: false,
                        sneak: false,
                    };
                    // reference path (kept at floor height, like the grounded body)
                    let mut rpos = start;
                    for _ in 0..150 {
                        pl.update_core(dt, &solid, &dry, input);
                        rpos = ref_move(rpos, wishdir * (speed * dt), &solid);
                    }
                    let d = ((pl.pos.x - rpos.x).powi(2) + (pl.pos.z - rpos.z).powi(2)).sqrt();
                    // Cardinals must track the reference tightly (the property the
                    // float-boundary bug broke: phantom/pass-through collisions).
                    // Diagonals slide along walls, where the two integrators round
                    // a corner up to one sub-step apart — allow that discretisation.
                    let tol = if dx == 0.0 || dz == 0.0 { 0.02 } else { 0.12 };
                    if d > tol {
                        failures.push(format!(
                            "{cname} {name} off={off:+.2}: sweep=({:.3},{:.3}) ref=({:.3},{:.3}) d={d:.3}",
                            pl.pos.x, pl.pos.z, rpos.x, rpos.z
                        ));
                    }
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} mismatches:\n{}",
        failures.len(),
        failures
            .iter()
            .take(40)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn sweep_does_not_skip_flush_wall_at_far_coordinates() {
    let bases = [2048, -2048, 8192, -8192];
    for base in bases {
        let wx = base + 10;
        let wz = base + 20;

        let wall_x = |x: i32, y: i32, z: i32| x == wx && (64..=65).contains(&y) && z == wz;
        let mut plus_x = p(Vec3::new(wx as f32 - HALF_W, 64.0, wz as f32 + 0.5));
        assert!(
            plus_x.sweep(Axis::X, 0.1, &wall_x),
            "+X should still hit a flush wall at base {base}"
        );
        assert!(
            (plus_x.pos.x - (wx as f32 - HALF_W)).abs() <= 0.001,
            "+X moved through flush wall at base {base}: x={}",
            plus_x.pos.x
        );

        let mut minus_x = p(Vec3::new((wx + 1) as f32 + HALF_W, 64.0, wz as f32 + 0.5));
        assert!(
            minus_x.sweep(Axis::X, -0.1, &wall_x),
            "-X should still hit a flush wall at base {base}"
        );
        assert!(
            (minus_x.pos.x - ((wx + 1) as f32 + HALF_W)).abs() <= 0.001,
            "-X moved through flush wall at base {base}: x={}",
            minus_x.pos.x
        );

        let wall_z = |x: i32, y: i32, z: i32| x == wx && (64..=65).contains(&y) && z == wz;
        let mut plus_z = p(Vec3::new(wx as f32 + 0.5, 64.0, wz as f32 - HALF_W));
        assert!(
            plus_z.sweep(Axis::Z, 0.1, &wall_z),
            "+Z should still hit a flush wall at base {base}"
        );
        assert!(
            (plus_z.pos.z - (wz as f32 - HALF_W)).abs() <= 0.001,
            "+Z moved through flush wall at base {base}: z={}",
            plus_z.pos.z
        );

        let mut minus_z = p(Vec3::new(wx as f32 + 0.5, 64.0, (wz + 1) as f32 + HALF_W));
        assert!(
            minus_z.sweep(Axis::Z, -0.1, &wall_z),
            "-Z should still hit a flush wall at base {base}"
        );
        assert!(
            (minus_z.pos.z - ((wz + 1) as f32 + HALF_W)).abs() <= 0.001,
            "-Z moved through flush wall at base {base}: z={}",
            minus_z.pos.z
        );
    }
}

#[test]
fn raycast_hits_block_ahead_with_back_normal() {
    // Single solid block at (4, 64, 0): entered at x=4.0, i.e. 3.5 from the
    // eye — within REACH (4.0). (A block at x=5 would be 4.5 away → a miss.)
    let solid = |x: i32, y: i32, z: i32| x == 4 && y == 64 && z == 0;
    // Eye centred in cell (0,64,0) looking +x.
    let eye = Vec3::new(0.5, 64.5, 0.5);
    let hit = Player::raycast_core(eye, Vec3::new(1.0, 0.0, 0.0), &solid).unwrap();
    assert_eq!(hit.block, IVec3::new(4, 64, 0));
    assert_eq!(hit.normal, IVec3::new(-1, 0, 0)); // face toward the eye
}

#[test]
fn raycast_out_of_reach_misses() {
    let solid = |x: i32, _y: i32, _z: i32| x == 100;
    let eye = Vec3::new(0.5, 64.5, 0.5);
    assert!(Player::raycast_core(eye, Vec3::new(1.0, 0.0, 0.0), &solid).is_none());
}

#[test]
fn raycast_eye_inside_solid_returns_zero_normal() {
    let solid = |_x: i32, _y: i32, _z: i32| true;
    let eye = Vec3::new(0.5, 64.5, 0.5);
    let hit = Player::raycast_core(eye, Vec3::new(1.0, 0.0, 0.0), &solid).unwrap();
    assert_eq!(hit.normal, IVec3::ZERO);
}

#[test]
fn raycast_precise_shape_uses_the_shape_surface_normal() {
    let blocks = |x: i32, y: i32, z: i32| {
        if (x, y, z) == (2, 64, 0) {
            Block::DirtSlab
        } else {
            Block::Air
        }
    };
    let eye = Vec3::new(1.9, 64.75, 0.5);
    let dir = Vec3::new(1.0, -0.5, 0.0).normalize();
    let (hit, _) = Player::raycast_blocks_core(eye, dir, &blocks, &|e, d, pos, block| {
        if block != Block::DirtSlab {
            return None;
        }
        let base = Vec3::new(pos.x as f32, pos.y as f32, pos.z as f32);
        interaction::ray_vs_aabb_hit(e, d, base, base + Vec3::new(1.0, 0.5, 1.0))
    })
    .unwrap();

    assert_eq!(hit.block, IVec3::new(2, 64, 0));
    assert_eq!(
        hit.normal,
        IVec3::Y,
        "placement should use the slab top face, not the full voxel side"
    );
}

#[test]
fn raycast_hits_a_plants_selection_box_without_pixel_precision() {
    let blocks = |x: i32, y: i32, z: i32| {
        if (x, y, z) == (2, 64, 0) {
            Block::Poppy
        } else {
            Block::Air
        }
    };
    // z = 0.5 crosses the cell centre where the sparse poppy art may well be
    // transparent — a BOX hitbox must select it anyway.
    let eye = Vec3::new(0.5, 64.25, 0.5);
    let (hit, _) =
        Player::raycast_blocks_core(eye, Vec3::new(1.0, 0.0, 0.0), &blocks, &|_, _, _, _| None)
            .unwrap();
    assert_eq!(hit.block, IVec3::new(2, 64, 0));
    assert_eq!(hit.normal, IVec3::new(-1, 0, 0));
    // The outline is the SAME square box the ray hit, trimmed to the art —
    // shorter than the cell (a ray can pass above it) and pulled in from the
    // cell walls.
    let SelectionShape::Box { min, max } = hit.outline else {
        panic!("plant outlines are square, got {:?}", hit.outline);
    };
    assert!(max.y < 65.0, "trims to the sprite's height, got {}", max.y);
    assert!(
        min.x > 2.0 && max.x < 3.0,
        "pulls in from the cell walls, got {}..{}",
        min.x,
        max.x
    );
}

#[test]
fn raycast_over_a_short_plants_box_misses_it() {
    let blocks = |x: i32, y: i32, z: i32| {
        if (x, y, z) == (2, 64, 0) {
            Block::Poppy
        } else {
            Block::Air
        }
    };
    let eye = Vec3::new(0.5, 64.95, 0.5);
    assert!(
        Player::raycast_blocks_core(eye, Vec3::new(1.0, 0.0, 0.0), &blocks, &|_, _, _, _| None)
            .is_none()
    );
}

#[test]
fn raycast_over_a_short_plants_box_hits_the_block_behind() {
    let blocks = |x: i32, y: i32, z: i32| match (x, y, z) {
        (2, 64, 0) => Block::Poppy,
        (3, 64, 0) => Block::Stone,
        _ => Block::Air,
    };
    let eye = Vec3::new(0.5, 64.95, 0.5);
    let (hit, _) =
        Player::raycast_blocks_core(eye, Vec3::new(1.0, 0.0, 0.0), &blocks, &|_, _, _, _| None)
            .unwrap();
    assert_eq!(hit.block, IVec3::new(3, 64, 0));
    assert_eq!(hit.normal, IVec3::new(-1, 0, 0));
}

#[test]
fn intersects_block_consistent_with_sweep_when_flush() {
    // Standing flush against a wall on the -X side: a -X resolve leaves the
    // min edge on the integer boundary, which float renders as 0.99999994.
    // `sweep` (lo = floor(min+EPS)) treats the cell beside you as free; the
    // place-gate must agree, or you can't build into a cell you clearly fit
    // next to.
    let pl = p(Vec3::new(1.3, 64.0, 0.5)); // min.x = 1.3 - 0.3 = 0.99999994
    assert!(
        pl.aabb_min().x < 1.0,
        "precondition: float pulls min.x below 1.0"
    );
    assert!(
        !pl.intersects_block(IVec3::new(0, 64, 0)),
        "flush-beside cell must read as free, matching sweep"
    );
    // And the cell the body actually stands in still counts.
    assert!(pl.intersects_block(IVec3::new(1, 64, 0)));
}

#[test]
fn intersects_block_strict_faces() {
    let pl = p(Vec3::new(0.5, 64.0, 0.5));
    // The cell the feet stand in overlaps.
    assert!(pl.intersects_block(IVec3::new(0, 64, 0)));
    // A block flush against +x face (player max.x = 0.8 < 1.0) does not.
    assert!(!pl.intersects_block(IVec3::new(1, 64, 0)));
    // A block at head height overlaps (player spans y in [64, 65.8]).
    assert!(pl.intersects_block(IVec3::new(0, 65, 0)));
    // Above the head does not.
    assert!(!pl.intersects_block(IVec3::new(0, 66, 0)));
}

/// A pool against a 1-block-high land ledge, for the swim-out tests. Pool floor
/// is solid up to y=6 (top 7) for x<2; the land plateau is solid up to y=9
/// (top 10) for x>=2; water fills the pool cells y=7..=9 (surface ~10, level
/// with the plateau top). So a swimmer in the pool must climb ~1 block of land
/// to get out onto the plateau.
fn pool_solid(x: i32, y: i32, _z: i32) -> bool {
    (x >= 2 && y <= 9) || (x < 2 && y <= 6)
}

fn pool_water(x: i32, y: i32, _z: i32) -> bool {
    x < 2 && (7..=9).contains(&y)
}

/// Explicitly jumping (Space) while swimming toward a climbable ledge gives a
/// firm upward boost (>= SWIM_CLIMB) so the player rises to crest it.
#[test]
fn swim_toward_ledge_boosts_up() {
    // Near the surface (feet=8: water probe at 8.6 -> cell 8 is water) and one
    // step back from the wall so the look-ahead probe reaches the plateau.
    let mut pl = p(Vec3::new(1.6, 8.0, 0.5));
    let input = Input {
        wishdir: Vec3::new(1.0, 0.0, 0.0),
        jump: true,
        sprint: false,
        sneak: false,
    };
    pl.update_core(
        0.02,
        &(pool_solid as fn(i32, i32, i32) -> bool),
        &(pool_water as fn(i32, i32, i32) -> bool),
        input,
    );
    assert!(
        pl.vel.y >= SWIM_CLIMB - 1e-3,
        "expected climb boost >= {SWIM_CLIMB}, got vel.y = {}",
        pl.vel.y
    );
}

/// Climbing out is an EXPLICIT action: moving toward a ledge WITHOUT pressing
/// jump must not boost (regression for the "1-deep / edge water hops you out on
/// its own" bug). Same scene as the boost test, jump released.
#[test]
fn swim_toward_ledge_requires_jump() {
    let mut pl = p(Vec3::new(1.6, 8.0, 0.5));
    let input = Input {
        wishdir: Vec3::new(1.0, 0.0, 0.0),
        jump: false,
        sprint: false,
        sneak: false,
    };
    pl.update_core(
        0.02,
        &(pool_solid as fn(i32, i32, i32) -> bool),
        &(pool_water as fn(i32, i32, i32) -> bool),
        input,
    );
    assert!(
        pl.vel.y < SWIM_CLIMB - 1.0,
        "no jump -> no climb boost; vel.y = {}",
        pl.vel.y
    );
}

/// Swimming toward open water (no ledge ahead) does NOT trigger the climb
/// boost — vertical stays the gentle buoyant motion, so surface bobbing is
/// preserved.
#[test]
fn swim_open_water_no_boost() {
    // Deep open water, no solid anywhere near, moving horizontally.
    let open_water = |_x: i32, _y: i32, _z: i32| true;
    let no_solid = |_x: i32, _y: i32, _z: i32| false;
    let mut pl = p(Vec3::new(0.0, 64.0, 0.0));
    let input = Input {
        wishdir: Vec3::new(1.0, 0.0, 0.0),
        jump: false,
        sprint: false,
        sneak: false,
    };
    pl.update_core(0.02, &no_solid, &open_water, input);
    assert!(
        pl.vel.y < SWIM_CLIMB - 1.0,
        "open water must not boost; vel.y = {}",
        pl.vel.y
    );
}

#[test]
fn flowing_water_pushes_idle_player_along_current() {
    let water = |_x: i32, y: i32, _z: i32| y == 64;
    let flow = |p: Vec3| {
        if p.y.floor() as i32 == 64 {
            Vec3::X
        } else {
            Vec3::ZERO
        }
    };
    let no_boxes = |_x: i32, _y: i32, _z: i32| Block::Air.collision_boxes();
    let mut pl = p(Vec3::new(0.5, 64.0, 0.5));

    pl.update_core_with_current(
        0.05,
        &no_boxes,
        &water,
        &flow,
        &no_ladder,
        Input::default(),
        &[],
    );

    assert!(
        pl.vel.x > 0.0,
        "current should add +X velocity: {}",
        pl.vel.x
    );
    assert!(
        pl.pos.x > 0.5,
        "current should move the player: {}",
        pl.pos.x
    );
}

#[test]
fn chest_collides_as_its_inset_box() {
    // A single chest at cell (0, 64, 0); every other cell is empty. Exercises the
    // general collision-box sweep with a non-full-cube shape.
    let boxes = |x: i32, y: i32, z: i32| {
        if (x, y, z) == (0, 64, 0) {
            Block::Chest
        } else {
            Block::Air
        }
        .collision_boxes()
    };
    let top = 64.0 + 14.0 / 16.0; // the chest's 14/16 collision top

    // Falling onto the chest lands the feet on its 14/16 top, not the full cell top.
    let mut faller = p(Vec3::new(0.5, 66.0, 0.5));
    assert!(
        faller.sweep_boxes(Axis::Y, -5.0, &boxes),
        "lands on the chest"
    );
    assert!(
        (faller.pos.y - top).abs() < 1e-3,
        "feet rest on the 14/16 top, got {}",
        faller.pos.y
    );

    // Standing on the chest top, you can walk off it (no full-cell wall blocking the
    // body just because its feet share the chest's cell).
    let mut on_top = p(Vec3::new(0.5, top, 0.5));
    assert!(
        !on_top.sweep_boxes(Axis::X, 1.0, &boxes),
        "walking off the top is not blocked"
    );
    assert!(
        (on_top.pos.x - 1.5).abs() < 1e-3,
        "walked the full step, got {}",
        on_top.pos.x
    );

    // At ground level beside the chest, walking into it stops at the 1/16 inset face,
    // not the cell boundary.
    let mut walker = p(Vec3::new(-1.0, 64.0, 0.5));
    assert!(
        walker.sweep_boxes(Axis::X, 2.0, &boxes),
        "hits the chest side"
    );
    assert!(
        (walker.aabb_max().x - 1.0 / 16.0).abs() < 1e-3,
        "stops at the inset -X face (1/16), got {}",
        walker.aabb_max().x
    );
}

/// Falling back into the water against the ledge wall (Space still held, still
/// facing the ledge) must NOT relaunch: the downward fall velocity is preserved
/// so the player sinks, instead of the boost discarding it and firing again
/// immediately. They sink once before another attempt is allowed.
#[test]
fn swim_falling_back_does_not_relaunch() {
    let solid = pool_solid as fn(i32, i32, i32) -> bool;
    let water = pool_water as fn(i32, i32, i32) -> bool;
    // At the wall, in water, but moving DOWN (just fell back in) at -5 m/s.
    let mut pl = p(Vec3::new(1.6, 8.0, 0.5));
    pl.vel.y = -5.0;
    let input = Input {
        wishdir: Vec3::new(1.0, 0.0, 0.0),
        jump: true,
        sprint: false,
        sneak: false,
    };
    pl.update_core(0.02, &solid, &water, input);
    assert!(
        pl.vel.y < 0.0,
        "falling back in must keep sinking, not relaunch; vel.y = {}",
        pl.vel.y
    );
}

/// Swimming into a tall (2+ block) wall must NOT boost — the assist is only for
/// 1-block ledges you can actually climb onto, so you can't scale a cliff face
/// just by holding into it underwater.
#[test]
fn swim_into_tall_wall_no_boost() {
    let tall_wall = |x: i32, _y: i32, _z: i32| x >= 2; // solid at every height
    let all_water = |x: i32, _y: i32, _z: i32| x < 2;
    let mut pl = p(Vec3::new(1.6, 8.0, 0.5));
    let input = Input {
        wishdir: Vec3::new(1.0, 0.0, 0.0),
        jump: false,
        sprint: false,
        sneak: false,
    };
    pl.update_core(0.02, &tall_wall, &all_water, input);
    assert!(
        pl.vel.y < SWIM_CLIMB - 1.0,
        "a tall wall is not a climbable ledge; vel.y = {}",
        pl.vel.y
    );
}

/// End-to-end: a swimmer holding "forward + swim up" against a 1-block ledge
/// climbs out of the pool and ends up standing on the plateau (the reported
/// "can't get out of the water onto a block" case).
#[test]
fn swims_out_onto_ledge() {
    let solid = pool_solid as fn(i32, i32, i32) -> bool;
    let water = pool_water as fn(i32, i32, i32) -> bool;
    // Start floating in the pool a little back from the wall, swimming up+toward.
    let mut pl = p(Vec3::new(1.0, 8.0, 0.5));
    let input = Input {
        wishdir: Vec3::new(1.0, 0.0, 0.0),
        jump: true,
        sprint: false,
        sneak: false,
    };
    for _ in 0..250 {
        pl.update_core(0.02, &solid, &water, input);
    }
    // Made it up and over onto the plateau (x>=2), out of the water, near its
    // top (y~10). Failure mode would leave the player stuck at the wall (x<2,
    // bobbing at y~8-9 in the pool).
    assert!(
        pl.pos.x > 2.0 && pl.pos.y > 9.5,
        "expected to climb onto the plateau, ended at ({:.2}, {:.2})",
        pl.pos.x,
        pl.pos.y
    );
    assert!(
        !water(
            pl.pos.x.floor() as i32,
            (pl.pos.y + WATER_PROBE_Y).floor() as i32,
            pl.pos.z.floor() as i32
        ),
        "expected to be out of the water after climbing out"
    );
}

#[test]
fn sneaking_halves_land_speed_and_overrides_sprint() {
    let solid = |_x: i32, y: i32, _z: i32| y == 0;
    let sneak_walk = Input {
        wishdir: Vec3::new(1.0, 0.0, 0.0),
        jump: false,
        sprint: false,
        sneak: true,
    };
    let mut pl = p(Vec3::new(0.5, 1.0, 0.5));
    for _ in 0..120 {
        pl.update_core(1.0 / 60.0, &solid, &dry, sneak_walk);
    }
    assert!(
        (pl.vel.x - WALK * 0.5).abs() < 0.01,
        "sneak top speed is half walk: {}",
        pl.vel.x
    );

    // Sneak + sprint held together: sneak wins.
    let both = Input {
        sprint: true,
        ..sneak_walk
    };
    let mut pl = p(Vec3::new(0.5, 1.0, 0.5));
    for _ in 0..120 {
        pl.update_core(1.0 / 60.0, &solid, &dry, both);
    }
    assert!(
        (pl.vel.x - WALK * 0.5).abs() < 0.01,
        "sneak overrides sprint: {}",
        pl.vel.x
    );
}

#[test]
fn sneaking_never_walks_off_a_ledge_but_jumping_escapes() {
    // A plateau ending at x=1: floor cells x<=0 at y=0, a deep drop beyond.
    let solid = |x: i32, y: i32, _z: i32| y == 0 && x <= 0;
    let sneak_walk = Input {
        wishdir: Vec3::new(1.0, 0.0, 0.0),
        jump: false,
        sprint: false,
        sneak: true,
    };
    let mut pl = p(Vec3::new(0.5, 1.0, 0.5));
    for _ in 0..300 {
        pl.update_core(1.0 / 60.0, &solid, &dry, sneak_walk);
    }
    assert!(pl.on_ground, "the sneaker never leaves the plateau");
    assert_eq!(pl.pos.y, 1.0, "feet stay on the plateau top");
    assert!(
        pl.pos.x < 1.0 + HALF_W,
        "stopped hanging at the lip, not past it: x={}",
        pl.pos.x
    );

    // The same walk WITHOUT sneak drops off.
    let plain = Input {
        sneak: false,
        ..sneak_walk
    };
    let mut pl = p(Vec3::new(0.5, 1.0, 0.5));
    for _ in 0..300 {
        pl.update_core(1.0 / 60.0, &solid, &dry, plain);
    }
    assert!(pl.pos.y < 1.0, "an ordinary walk falls off the ledge");

    // Jumping while sneaking is an explicit action: it clears the edge.
    let hop = Input {
        jump: true,
        ..sneak_walk
    };
    let mut pl = p(Vec3::new(0.5, 1.0, 0.5));
    for _ in 0..300 {
        pl.update_core(1.0 / 60.0, &solid, &dry, hop);
    }
    assert!(
        pl.pos.x > 1.0 + HALF_W,
        "a sneak jump still leaves the plateau: x={}",
        pl.pos.x
    );
}

#[test]
fn sneaking_still_steps_down_a_half_block() {
    use crate::block::Aabb;
    let still = |_: Vec3| Vec3::ZERO;
    // A full floor for x<=0, a half-height slab (top y=0.5) for x>=1: a
    // step DOWN of exactly the step height, which sneaking must allow.
    const SLAB: &[Aabb] = &[Aabb {
        min: [0.0, 0.0, 0.0],
        max: [1.0, 0.5, 1.0],
    }];
    let step_down = |x: i32, y: i32, _z: i32| -> &'static [Aabb] {
        if y != 0 {
            &[]
        } else if x <= 0 {
            Block::Stone.collision_boxes()
        } else {
            SLAB
        }
    };
    let sneak_walk = Input {
        wishdir: Vec3::new(1.0, 0.0, 0.0),
        jump: false,
        sprint: false,
        sneak: true,
    };
    let mut pl = p(Vec3::new(0.5, 1.0, 0.5));
    for _ in 0..300 {
        pl.update_core_with_current(
            1.0 / 60.0,
            &step_down,
            &dry,
            &still,
            &no_ladder,
            sneak_walk,
            &[],
        );
    }
    assert!(
        pl.pos.x > 1.5,
        "the sneaker walks onto the lower step: x={}",
        pl.pos.x
    );
    assert!(pl.on_ground, "grounded on the slab");
    assert!(
        (pl.pos.y - 0.5).abs() < 1e-3,
        "feet settled on the slab top: y={}",
        pl.pos.y
    );
}

#[test]
fn sneak_step_down_is_instant_so_diagonal_descent_cannot_fall_off() {
    use crate::block::Aabb;
    let still = |_: Vec3| Vec3::ZERO;
    // A plateau (x<=0, top y=1), a ONE-block-wide slab strip beside it (x==1,
    // top y=0.5 — a legal step-down), and void beyond and below. Sneaking
    // diagonally (+X+Z) must step onto the strip and then slide along its far
    // lip forever. The old airborne step-down failed exactly here: ~10 frames
    // of gravity with the guard off let the diagonal momentum carry the body
    // across the 1-wide strip and off its far edge.
    const SLAB: &[Aabb] = &[Aabb {
        min: [0.0, 0.0, 0.0],
        max: [1.0, 0.5, 1.0],
    }];
    let world = |x: i32, y: i32, _z: i32| -> &'static [Aabb] {
        if y != 0 {
            &[]
        } else if x <= 0 {
            Block::Stone.collision_boxes()
        } else if x == 1 {
            SLAB
        } else {
            &[]
        }
    };
    let diag = Input {
        wishdir: Vec3::new(
            std::f32::consts::FRAC_1_SQRT_2,
            0.0,
            std::f32::consts::FRAC_1_SQRT_2,
        ),
        jump: false,
        sprint: false,
        sneak: true,
    };
    let mut pl = p(Vec3::new(0.5, 1.0, 0.5));
    let mut min_y = f32::MAX;
    let mut airborne_frames = 0;
    for i in 0..600 {
        pl.update_core_with_current(1.0 / 60.0, &world, &dry, &still, &no_ladder, diag, &[]);
        min_y = min_y.min(pl.pos.y);
        // Skip the first frames: a fresh Player spawns with on_ground unset.
        if i > 2 && !pl.on_ground {
            airborne_frames += 1;
        }
    }
    assert_eq!(
        airborne_frames, 0,
        "the sneak step-down is instant — the guard never disengages mid-descent"
    );
    assert!(
        min_y >= 0.5 - 1e-3,
        "never dipped below the slab top: {min_y}"
    );
    assert!(
        pl.on_ground && (pl.pos.y - 0.5).abs() < 1e-3,
        "settled on the strip: y={}",
        pl.pos.y
    );
    assert!(
        pl.pos.x < 2.0 + HALF_W,
        "hangs at the strip's far lip, never past it: x={}",
        pl.pos.x
    );
    assert!(
        pl.pos.z > 4.0,
        "still slides along the lip meanwhile: z={}",
        pl.pos.z
    );
}
