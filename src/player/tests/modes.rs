use super::*;

#[test]
fn spectator_clips_through_solids_and_flies_in_3d() {
    let solid_wall_and_ceiling = |x: i32, y: i32, _z: i32| x >= 1 || y >= 65;
    let mut pl = p(WorldPos::new(0.0, 64.0, 0.0));
    pl.set_mode(PlayerMode::Spectator);

    let input = Input {
        wishdir: Vec3::new(1.0, 1.0, 0.0).normalize(),
        jump: false,
        sprint: false,
        sneak: false,
    };
    pl.update_core(1.0, &solid_wall_and_ceiling, input);

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
    let mut pl = p(WorldPos::new(0.0, 64.0, 0.0));
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
fn creative_flight_coasts_consistently_and_collides() {
    let moving = Input {
        wishdir: Vec3::X,
        jump: false,
        sprint: false,
        sneak: false,
    };
    let stopped = Input {
        wishdir: Vec3::ZERO,
        ..moving
    };
    let run = |steps: u32| {
        let mut pl = p(WorldPos::new(0.0, 64.0, 0.0));
        pl.toggle_creative();
        assert!(!pl.is_flying());
        pl.toggle_creative_flight();
        for _ in 0..steps {
            pl.update_core(1.0 / steps as f32, &|_, _, _| false, moving);
        }
        let release = pl.pos;
        let speed = pl.vel.length();
        pl.update_core(0.05, &|_, _, _| false, stopped);
        assert!(pl.vel.length() > 0.0 && pl.vel.length() < speed);
        assert!(pl.pos.x > release.x);
        (pl.pos, pl.vel)
    };
    let (a, av) = run(30);
    let (b, bv) = run(144);
    assert!((a - b).length() < 0.001);
    assert!((av - bv).length() < 0.001);
    let mut pl = p(WorldPos::new(0.0, 64.0, 0.0));
    pl.set_mode(PlayerMode::CreativeFlying);
    pl.update_core(0.5, &|x, _, _| x >= 1, moving);
    assert!(pl.pos.x < 1.0);
    assert_eq!(pl.vel.x, 0.0);
    let hp = pl.health();
    assert!(!pl.apply_damage(5, petramond_world::damage::Immunity::Exempt));
    assert_eq!(pl.health(), hp);
}
