use super::*;

/// Slippery support (ice): a coasting body glides far beyond what ordinary
/// ground friction allows, and steering input redirects it sluggishly. Pins
/// the grounded friction/snap swap in `update_core_with_current`, not the
/// exact constants.
#[test]
fn ice_glides_far_beyond_ordinary_ground() {
    let solid = |_x: i32, y: i32, _z: i32| y < 64;
    let coast = |ice_floor: bool| {
        let mut pl = p(Vec3::new(0.5, 64.0, 0.5));
        pl.vel = Vec3::new(6.0, 0.0, 0.0); // launched at walk speed, then no input
        let slippery = move |_x: i32, y: i32, _z: i32| ice_floor && y == 63;
        let x0 = pl.pos.x;
        for _ in 0..120 {
            pl.update_core_slippery(1.0 / 60.0, &solid, &dry, &slippery, Input::default());
        }
        pl.pos.x - x0
    };
    let ground = coast(false);
    let ice = coast(true);
    assert!(
        ice > 3.0 * ground,
        "ice coast ({ice:.2}m) should far exceed ground coast ({ground:.2}m)"
    );

    // Steering: from a +X slide, full -X input for a quarter second reverses a
    // grounded body but only BRAKES an ice-borne one — the slide smears.
    let steer = |ice_floor: bool| {
        let mut pl = p(Vec3::new(0.5, 64.0, 0.5));
        pl.vel = Vec3::new(6.0, 0.0, 0.0);
        let slippery = move |_x: i32, y: i32, _z: i32| ice_floor && y == 63;
        let back = Input {
            wishdir: Vec3::new(-1.0, 0.0, 0.0),
            jump: false,
            sprint: false,
            sneak: false,
        };
        for _ in 0..15 {
            pl.update_core_slippery(1.0 / 60.0, &solid, &dry, &slippery, back);
        }
        pl.vel.x
    };
    assert!(steer(false) < 0.0, "ordinary ground reverses within the window");
    assert!(steer(true) > 0.0, "ice keeps sliding forward through the same input");
}
