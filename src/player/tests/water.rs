use super::*;

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
        &no_slip,
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
