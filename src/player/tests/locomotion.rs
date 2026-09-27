use super::*;

#[test]
fn mode_keys_select_nothing_without_a_wish() {
    let pl = p(WorldPos::ZERO);
    let still = |sneak: bool, sprint: bool| Input {
        wishdir: Vec3::ZERO,
        jump: false,
        sprint,
        sneak,
    };
    assert_eq!(pl.wish_speed(still(false, true)), WALK, "held sprint");
    assert_eq!(pl.wish_speed(still(true, false)), WALK, "held sneak");
    let moving = |sneak: bool, sprint: bool| Input {
        wishdir: Vec3::new(1.0, 0.0, 0.0),
        ..still(sneak, sprint)
    };
    assert_eq!(pl.wish_speed(moving(false, true)), SPRINT);
    assert_eq!(pl.wish_speed(moving(false, false)), WALK);
}

#[test]
fn the_mod_body_scale_multiplies_the_wished_land_speed() {
    let mut pl = p(WorldPos::ZERO);
    let walk = Input {
        wishdir: Vec3::new(1.0, 0.0, 0.0),
        jump: false,
        sprint: false,
        sneak: false,
    };
    assert_eq!(pl.wish_speed(walk), WALK);

    pl.claims
        .set_attribute("combat", mod_api::PlayerAttribute::MoveSpeed, 0.5);
    assert_eq!(pl.wish_speed(walk), WALK * 0.5);

    pl.claims
        .set_attribute("armour", mod_api::PlayerAttribute::MoveSpeed, 0.5);
    assert_eq!(pl.wish_speed(walk), WALK * 0.25);

    use crate::player::DeniedActions;
    use mod_api::BodyAction;
    pl.claims
        .set_attribute("armour", mod_api::PlayerAttribute::MoveSpeed, 1.0);
    pl.claims
        .set_attribute("combat", mod_api::PlayerAttribute::MoveSpeed, 1.0);
    pl.claims.set_attribute(
        crate::player::ENGINE_CLAIMANT,
        mod_api::PlayerAttribute::MoveSpeed,
        0.5,
    );
    pl.adopt_resolved_body(0.5, 1.0, DeniedActions::of([BodyAction::Mine]));
    assert_eq!(
        pl.wish_speed(walk),
        WALK * 0.25,
        "the adopted answer composes with the half the mirror derives"
    );
    assert!(pl.denied_actions().denies(BodyAction::Mine));
    assert!(!pl.denied_actions().denies(BodyAction::Attack));

    pl.adopt_resolved_body(1.0, 1.0, DeniedActions::NONE);
    assert_eq!(pl.wish_speed(walk), WALK * 0.5, "and releases only its own");
    assert!(pl.denied_actions().is_empty(), "the mirror releases too");
}

#[test]
fn air_decays_slower_than_ground() {
    let dt = FRICTION_REF_DT;
    let open = |_x: i32, _y: i32, _z: i32| false;
    let mut air = p(WorldPos::new(0.0, 128.0, 0.0));
    air.vel = Vec3::new(WALK, 5.0, 0.0);
    air.on_ground = false;
    air.update_core(dt, &open, Input::default());

    let floor = |_x: i32, y: i32, _z: i32| y < 64;
    let mut gnd = p(WorldPos::new(0.0, 64.0, 0.0));
    gnd.vel = Vec3::new(WALK, 0.0, 0.0);
    gnd.on_ground = true;
    gnd.update_core(dt, &floor, Input::default());

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

    let floor = |_x: i32, y: i32, _z: i32| y < 64;
    let mut g = p(WorldPos::new(0.0, 64.0, 0.0));
    g.on_ground = true;
    g.update_core(dt, &floor, input);
    assert!(
        (g.vel.x - GROUND_ACCEL * dt).abs() < 1e-5,
        "ground vx = {}",
        g.vel.x
    );

    let open = |_x: i32, _y: i32, _z: i32| false;
    let mut a = p(WorldPos::new(0.0, 128.0, 0.0));
    a.on_ground = false;
    a.update_core(dt, &open, input);
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
    let open = |_x: i32, _y: i32, _z: i32| false;
    let input = Input {
        wishdir: Vec3::new(1.0, 0.0, 0.0),
        jump: false,
        sprint: false,
        sneak: false,
    };
    let mut a = p(WorldPos::new(0.0, 128.0, 0.0));
    a.on_ground = false;
    a.vel = Vec3::new(SPRINT, 0.0, 0.0);
    a.update_core(FRICTION_REF_DT, &open, input);
    assert!(
        (a.vel.x - SPRINT).abs() < 1e-5,
        "air input must not brake momentum, vx = {}",
        a.vel.x
    );
}

#[test]
fn air_steering_redirects_without_inflating_speed() {
    let open = |_x: i32, _y: i32, _z: i32| false;
    let input = Input {
        wishdir: Vec3::new(0.0, 0.0, 1.0),
        jump: false,
        sprint: false,
        sneak: false,
    };
    let mut a = p(WorldPos::new(0.0, 128.0, 0.0));
    a.on_ground = false;
    a.vel = Vec3::new(WALK, 0.0, 0.0);
    a.update_core(FRICTION_REF_DT, &open, input);
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
    let wall_x = 6;
    let solid = move |x: i32, _y: i32, _z: i32| x >= wall_x;
    let mut a = p(WorldPos::new(wall_x as f64 - 1.0, 128.0, 0.0));
    a.on_ground = false;
    let wishdir = Vec3::new(0.98, 0.0, 0.2).normalize();
    let input = Input {
        wishdir,
        jump: false,
        sprint: false,
        sneak: false,
    };
    for _ in 0..600 {
        a.update_core(0.02, &solid, input);
    }
    let speed = (a.vel.x * a.vel.x + a.vel.z * a.vel.z).sqrt();
    assert!(
        speed <= WALK + 1e-3,
        "wall-scrape pumped speed to {speed} (cap is WALK = {WALK})"
    );
}

#[test]
fn air_out_coasts_ground() {
    let open = |_x: i32, _y: i32, _z: i32| false;
    let floor = |_x: i32, y: i32, _z: i32| y < 64;
    let mut air = p(WorldPos::new(0.0, 1024.0, 0.0));
    air.on_ground = false;
    air.vel = Vec3::new(WALK, 0.0, 0.0);
    let mut gnd = p(WorldPos::new(0.0, 64.0, 0.0));
    gnd.on_ground = true;
    gnd.vel = Vec3::new(WALK, 0.0, 0.0);
    let steps = 30;
    for _ in 0..steps {
        air.update_core(FRICTION_REF_DT, &open, Input::default());
        gnd.update_core(FRICTION_REF_DT, &floor, Input::default());
    }
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
        assert_eq!(
            friction_retain(0.0, dt),
            1.0,
            "friction 0 must not decay (dt={dt})"
        );
        assert_eq!(
            friction_retain(1.0, dt),
            0.0,
            "friction 1 must snap to a stop (dt={dt})"
        );
    }
    assert!(
        (friction_retain(GROUND_FRICTION, FRICTION_REF_DT) - (1.0 - GROUND_FRICTION)).abs() < 1e-6
    );
    assert!((friction_retain(AIR_FRICTION, FRICTION_REF_DT) - (1.0 - AIR_FRICTION)).abs() < 1e-6);
}

#[test]
fn friction_is_framerate_independent() {
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
    let mut near = p(WorldPos::new(0.0, 128.0, 0.0));
    near.vel = Vec3::new(0.0, 1.0, 0.0);
    near.on_ground = false;
    near.jumping = true;
    near.update_core(0.05, &open, Input::default());
    let near_drop = 1.0 - near.vel.y;

    let mut fast = p(WorldPos::new(0.0, 128.0, 0.0));
    fast.vel = Vec3::new(0.0, 20.0, 0.0);
    fast.on_ground = false;
    fast.jumping = true;
    fast.update_core(0.05, &open, Input::default());
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
    let open = |_x: i32, _y: i32, _z: i32| false;
    let mut pl = p(WorldPos::new(0.0, 128.0, 0.0));
    pl.vel = Vec3::new(0.0, 1.0, 0.0);
    pl.on_ground = false;
    pl.jumping = false;
    pl.update_core(0.05, &open, Input::default());
    let drop = 1.0 - pl.vel.y;
    assert!(
        (drop - GRAVITY * 0.05).abs() < 1e-5,
        "not jumping → full gravity, got {drop}"
    );
}
