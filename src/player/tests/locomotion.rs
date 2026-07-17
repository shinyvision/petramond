use super::*;

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
