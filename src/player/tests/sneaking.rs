use super::*;

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
            &no_slip,
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
        pl.update_core_with_current(1.0 / 60.0, &world, &dry, &still, &no_ladder, &no_slip, diag, &[]);
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
