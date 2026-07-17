use super::*;

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
